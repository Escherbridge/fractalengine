//! Native and queued file selection; see AGENTS.md §file-dialogs.

use fe_runtime::qa_debug::{DialogAnswer, DialogAnswerBatch, DialogKind};
use std::collections::VecDeque;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug)]
pub struct DialogFailure {
    pub code: &'static str,
    pub message: &'static str,
}
impl std::fmt::Display for DialogFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}
fn refused(code: &'static str, message: &'static str) -> DialogFailure {
    DialogFailure { code, message }
}

/// A selected path retaining its confinement handles through deferred writes.
#[derive(Clone, Debug)]
pub struct DialogPath {
    path: PathBuf,
    root: Option<PathBuf>,
    _pins: Arc<Vec<std::fs::File>>,
}
impl From<PathBuf> for DialogPath {
    fn from(path: PathBuf) -> Self {
        Self {
            path,
            root: None,
            _pins: Arc::new(vec![]),
        }
    }
}
impl AsRef<Path> for DialogPath {
    fn as_ref(&self) -> &Path {
        &self.path
    }
}
impl std::ops::Deref for DialogPath {
    type Target = Path;
    fn deref(&self) -> &Path {
        &self.path
    }
}
impl DialogPath {
    pub fn read_lease(&self) -> Option<fe_runtime::messages::FileReadLease> {
        self.root.as_ref().map(|_| self._pins.clone())
    }
    pub fn write_bytes(&self, bytes: &[u8]) -> Result<(), String> {
        self.validate_for_write().map_err(|e| e.to_string())?;
        if self.root.is_some() {
            fe_identity::file_fallback::replace_owner_only_file(&self.path, bytes)
                .map_err(|e| e.to_string())
        } else {
            std::fs::write(&self.path, bytes).map_err(|e| e.to_string())
        }
    }
    pub fn copy_from(&self, source: &Path) -> Result<(), String> {
        self.validate_for_write().map_err(|e| e.to_string())?;
        if self.root.is_some() {
            fe_identity::file_fallback::copy_owner_only_file(source, &self.path)
                .map_err(|e| e.to_string())
        } else {
            std::fs::copy(source, &self.path)
                .map(|_| ())
                .map_err(|e| e.to_string())
        }
    }
    pub fn validate_for_write(&self) -> Result<(), DialogFailure> {
        if let Some(root) = &self.root {
            confined_path(root, &self.path, DialogKind::Save)?;
        }
        Ok(())
    }
}

pub struct FileDialogRequest<'a> {
    pub kind: DialogKind,
    pub verse_id: Option<&'a str>,
    pub filters: &'a [(&'a str, &'a [&'a str])],
    pub suggested_name: Option<&'a str>,
}

/// Strategy for all user-facing native file dialogs.
pub trait FileDialogProvider: Send + Sync {
    fn choose(&self, request: FileDialogRequest<'_>) -> Result<Option<DialogPath>, DialogFailure>;
    fn lease_path(
        &self,
        path: &Path,
        kind: DialogKind,
        verse_id: Option<&str>,
    ) -> Result<DialogPath, DialogFailure>;
}

#[derive(Clone)]
pub struct FileDialogs(pub Arc<dyn FileDialogProvider>);
impl Default for FileDialogs {
    fn default() -> Self {
        Self(Arc::new(NativeFileDialogs))
    }
}
impl FileDialogs {
    pub fn disabled() -> Self {
        Self(Arc::new(DisabledFileDialogs))
    }
    pub fn choose(
        &self,
        request: FileDialogRequest<'_>,
    ) -> Result<Option<DialogPath>, DialogFailure> {
        let _surface = crate::native_accessibility::surface::NativeDialog::enter();
        self.0.choose(request)
    }
    pub fn lease_path(
        &self,
        path: &Path,
        kind: DialogKind,
        verse_id: Option<&str>,
    ) -> Result<DialogPath, DialogFailure> {
        self.0.lease_path(path, kind, verse_id)
    }
}
struct DisabledFileDialogs;
impl FileDialogProvider for DisabledFileDialogs {
    fn lease_path(
        &self,
        _: &Path,
        _: DialogKind,
        _: Option<&str>,
    ) -> Result<DialogPath, DialogFailure> {
        Err(refused(
            "qa_controls_disabled",
            "QA file access is unavailable without isolated storage and a loopback listener",
        ))
    }
    fn choose(&self, _: FileDialogRequest<'_>) -> Result<Option<DialogPath>, DialogFailure> {
        Err(refused("qa_controls_disabled", "Synthetic input requires an isolated QA root and loopback listener before file dialogs can be answered"))
    }
}

struct NativeFileDialogs;
impl FileDialogProvider for NativeFileDialogs {
    fn lease_path(
        &self,
        path: &Path,
        _: DialogKind,
        _: Option<&str>,
    ) -> Result<DialogPath, DialogFailure> {
        Ok(path.to_path_buf().into())
    }
    fn choose(&self, request: FileDialogRequest<'_>) -> Result<Option<DialogPath>, DialogFailure> {
        let mut dialog = rfd::FileDialog::new();
        for (name, extensions) in request.filters {
            dialog = dialog.add_filter(*name, extensions);
        }
        if let Some(name) = request.suggested_name {
            dialog = dialog.set_file_name(name);
        }
        if request.kind == DialogKind::Save {
            if let Some(directory) = dirs::download_dir() {
                dialog = dialog.set_directory(directory);
            }
        }
        let path = match request.kind {
            DialogKind::Open => dialog.pick_file(),
            DialogKind::Save => dialog.save_file(),
            DialogKind::Folder => dialog.pick_folder(),
        };
        Ok(path.map(|path| DialogPath {
            path,
            root: None,
            _pins: Arc::new(vec![]),
        }))
    }
}

struct LeasedAnswer {
    verse: String,
    answer: DialogAnswer,
}
#[derive(Default)]
struct Queue {
    loopback: bool,
    answers: VecDeque<LeasedAnswer>,
}
pub struct QaFileDialogs {
    root: PathBuf,
    queue: Mutex<Queue>,
}
impl QaFileDialogs {
    pub fn new(root: PathBuf) -> Result<Self, DialogFailure> {
        let root = local_path(
            root.canonicalize()
                .map_err(|_| refused("dialog_root_invalid", "QA root is unavailable"))?,
        );
        confined_path(&root, &root, DialogKind::Folder)?;
        Ok(Self {
            root,
            queue: Mutex::new(Queue::default()),
        })
    }
    pub fn set_listener(&self, bound: std::net::SocketAddr) {
        if let Ok(mut queue) = self.queue.lock() {
            queue.loopback = bound.ip().is_loopback();
        }
    }
    pub fn enabled(&self) -> bool {
        self.queue.lock().is_ok_and(|q| q.loopback)
    }
    pub fn enqueue(&self, verse: &str, batch: &DialogAnswerBatch) -> Result<usize, DialogFailure> {
        batch
            .validate()
            .map_err(|_| refused("invalid_dialog_answers", "invalid dialog answer batch"))?;
        let mut queue = self
            .queue
            .lock()
            .map_err(|_| refused("dialog_queue_unavailable", "QA dialog queue is unavailable"))?;
        if !queue.loopback {
            return Err(refused(
                "qa_controls_disabled",
                "QA controls require an actual loopback listener",
            ));
        }
        let prepared = batch
            .answers
            .iter()
            .map(|answer| {
                let mut answer = answer.clone();
                if let Some(path) = &answer.path {
                    answer.path = Some(
                        confined_path(&self.root, Path::new(path), answer.kind)?
                            .to_string_lossy()
                            .into_owned(),
                    );
                }
                Ok(LeasedAnswer {
                    verse: verse.to_owned(),
                    answer,
                })
            })
            .collect::<Result<Vec<_>, DialogFailure>>()?;
        if queue
            .answers
            .iter()
            .filter(|entry| entry.verse == verse)
            .count()
            + prepared.len()
            > 64
        {
            return Err(refused("dialog_queue_full", "QA dialog queue is full"));
        }
        queue.answers.retain(|entry| entry.verse == verse);
        queue.answers.extend(prepared);
        Ok(queue.answers.len())
    }
}
impl FileDialogProvider for QaFileDialogs {
    fn choose(&self, request: FileDialogRequest<'_>) -> Result<Option<DialogPath>, DialogFailure> {
        let mut queue = self
            .queue
            .lock()
            .map_err(|_| refused("dialog_queue_unavailable", "QA dialog queue is unavailable"))?;
        if !queue.loopback {
            return Err(refused(
                "qa_controls_disabled",
                "QA controls require an actual loopback listener",
            ));
        }
        let verse = request
            .verse_id
            .ok_or_else(|| refused("not_found_or_denied", "not found or not permitted"))?;
        let entry = queue.answers.pop_front().ok_or_else(|| {
            refused(
                "dialog_answer_missing",
                "Queue a QA dialog answer before choosing a file",
            )
        })?;
        if entry.verse != verse {
            return Err(refused(
                "dialog_answer_scope_changed",
                "Queued answer belongs to a different active verse",
            ));
        }
        if entry.answer.kind != request.kind {
            return Err(refused(
                "dialog_answer_kind_mismatch",
                "Queued answer has the wrong dialog kind",
            ));
        }
        if entry.answer.cancel {
            return Ok(None);
        }
        let supplied = entry
            .answer
            .path
            .ok_or_else(|| refused("invalid_dialog_answer", "Dialog answer path is missing"))?;
        drop(queue);
        self.lease_path(Path::new(&supplied), request.kind, Some(verse))
            .map(Some)
    }

    fn lease_path(
        &self,
        path: &Path,
        kind: DialogKind,
        verse_id: Option<&str>,
    ) -> Result<DialogPath, DialogFailure> {
        if !self.enabled() || verse_id.is_none() {
            return Err(refused(
                "qa_controls_disabled",
                "QA file access requires an active verse and loopback listener",
            ));
        }
        let path = confined_path(&self.root, path, kind)?;
        let mut pins = fe_identity::file_fallback::pin_secret_parents(&path).map_err(|_| {
            refused(
                "dialog_path_invalid",
                "Dialog path could not be pinned safely",
            )
        })?;
        confined_path(&self.root, &path, kind)?;
        if kind != DialogKind::Save {
            #[cfg(windows)]
            {
                use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
                let file = std::fs::OpenOptions::new()
                    .access_mode(0x80)
                    .share_mode(0x1)
                    .custom_flags(0x0220_0000)
                    .open(&path)
                    .map_err(|_| {
                        refused("dialog_path_invalid", "Selected input could not be pinned")
                    })?;
                if file
                    .metadata()
                    .map_err(|_| {
                        refused(
                            "dialog_path_invalid",
                            "Selected input metadata is unavailable",
                        )
                    })?
                    .file_attributes()
                    & 0x400
                    != 0
                {
                    return Err(refused(
                        "dialog_path_not_confined",
                        "Selected input is a reparse point",
                    ));
                }
                pins.push(file);
            }
            #[cfg(not(windows))]
            pins.push(std::fs::File::open(&path).map_err(|_| {
                refused("dialog_path_invalid", "Selected input could not be pinned")
            })?);
        }
        let pins = Arc::new(pins);
        Ok(DialogPath {
            path,
            root: Some(self.root.clone()),
            _pins: pins,
        })
    }
}

fn local_path(path: PathBuf) -> PathBuf {
    #[cfg(windows)]
    if let Some(text) = path.to_str().and_then(|s| s.strip_prefix("\\\\?\\")) {
        if text.as_bytes().get(1) == Some(&b':') {
            return PathBuf::from(text);
        }
    }
    path
}

fn confined_path(root: &Path, supplied: &Path, kind: DialogKind) -> Result<PathBuf, DialogFailure> {
    let invalid = || {
        refused(
            "dialog_path_not_confined",
            "Dialog path must stay inside the QA run without traversal, streams or reparse points",
        )
    };
    let text = supplied.to_str().ok_or_else(invalid)?;
    if text.contains('\0')
        || text.starts_with("\\\\")
        || text.starts_with("//")
        || supplied.components().any(|c| c == Component::ParentDir)
    {
        return Err(invalid());
    }
    #[cfg(windows)]
    {
        if supplied.has_root() && !supplied.is_absolute() {
            return Err(invalid());
        }
        if text.as_bytes().get(1) == Some(&b':') && !supplied.is_absolute() {
            return Err(invalid());
        }
        if text
            .char_indices()
            .any(|(i, c)| c == ':' && !(i == 1 && text.as_bytes()[0].is_ascii_alphabetic()))
        {
            return Err(invalid());
        }
        for component in supplied.components() {
            if let Component::Normal(value) = component {
                let value = value.to_str().ok_or_else(invalid)?;
                let base = value
                    .split('.')
                    .next()
                    .unwrap_or_default()
                    .to_ascii_uppercase();
                let port = base
                    .strip_prefix("COM")
                    .or_else(|| base.strip_prefix("LPT"));
                if value.ends_with(['.', ' '])
                    || value
                        .chars()
                        .any(|c| c.is_control() || matches!(c, '<' | '>' | '"' | '|' | '?' | '*'))
                    || matches!(
                        base.as_str(),
                        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
                    )
                    || port.is_some_and(|suffix| {
                        matches!(
                            suffix,
                            "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
                        )
                    })
                {
                    return Err(invalid());
                }
            }
        }
    }
    let path = if supplied.is_absolute() {
        supplied.to_path_buf()
    } else {
        root.join(supplied)
    };
    for ancestor in path.ancestors() {
        match std::fs::symlink_metadata(ancestor) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() {
                    return Err(invalid());
                }
                #[cfg(windows)]
                {
                    use std::os::windows::fs::MetadataExt;
                    if metadata.file_attributes() & 0x400 != 0 {
                        return Err(invalid());
                    }
                }
            }
            Err(error)
                if error.kind() == std::io::ErrorKind::NotFound
                    && ancestor == path
                    && kind == DialogKind::Save => {}
            Err(_) => return Err(invalid()),
        }
    }
    let resolved = if path.exists() {
        local_path(path.canonicalize().map_err(|_| invalid())?)
    } else {
        let parent = path
            .parent()
            .ok_or_else(invalid)?
            .canonicalize()
            .map_err(|_| invalid())?;
        local_path(parent.join(path.file_name().ok_or_else(invalid)?))
    };
    if !resolved.starts_with(root)
        || (kind == DialogKind::Open && !resolved.is_file())
        || (kind == DialogKind::Folder && !resolved.is_dir())
        || (kind == DialogKind::Save && resolved.is_dir())
    {
        return Err(invalid());
    }
    Ok(resolved)
}
