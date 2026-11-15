//! Package storage seam (`PackageStore`) + the filesystem implementation — see `AGENTS.md` §store.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use axum::body::Bytes;

use crate::index::{self, HexonIndexEntry};

/// Prefix of in-flight publish temp files (never ends in `.hexon`, so `scan_dir` ignores them).
const TEMP_PREFIX: &str = ".fe-registry-tmp-";

/// Largest catalogue document the filesystem store will read (matches a client's document cap).
const MAX_CATALOG_DOC_BYTES: u64 = 16 << 20;

/// Why a store operation failed.
#[derive(Debug)]
pub enum StoreError {
    /// `put` found the `{id}@{version}` slot already taken (the create-new rule).
    AlreadyExists,
    /// The id, version or catalogue path is outside the contract grammar.
    Invalid(String),
    /// Local I/O failure.
    Io(std::io::Error),
    /// Any other backend failure (object store, database).
    Backend(String),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AlreadyExists => write!(f, "package already exists"),
            Self::Invalid(msg) => write!(f, "invalid: {msg}"),
            Self::Io(e) => write!(f, "io: {e}"),
            Self::Backend(msg) => write!(f, "backend: {msg}"),
        }
    }
}

impl std::error::Error for StoreError {}

impl From<std::io::Error> for StoreError {
    fn from(e: std::io::Error) -> Self {
        if e.kind() == std::io::ErrorKind::AlreadyExists {
            Self::AlreadyExists
        } else {
            Self::Io(e)
        }
    }
}

/// Where sealed `.hexon` archives and catalogue documents live; dyn-safe so a hosted registry swaps the backend.
#[async_trait]
pub trait PackageStore: Send + Sync {
    /// Every stored package; `HexonIndexEntry::path` must name a locally readable copy (remote stores materialise one).
    async fn list(&self) -> Result<Vec<HexonIndexEntry>, StoreError>;

    /// Store one archive at `{id}@{version}`; `AlreadyExists` when the slot is taken (never overwrite).
    async fn put(&self, id: &str, version: &str, bytes: Bytes) -> Result<(), StoreError>;

    /// One catalogue document at `path` under `api/v1/catalog/`, byte-for-byte; `None` when absent.
    async fn catalog_doc(&self, path: &str) -> Result<Option<Bytes>, StoreError>;
}

/// One path segment of an id, version or catalogue document: `[A-Za-z0-9._-]`, never starting with `.`.
fn portable_segment(s: &str) -> bool {
    !s.is_empty()
        && !s.starts_with('.')
        && s
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// A catalogue path a store may be asked for: portable segments only, at most the
/// `packages/{id}/{version}/attestation.json` depth, and none of the reserved v1 paths.
pub fn valid_catalog_path(path: &str) -> bool {
    if path.is_empty() || path.len() > 512 {
        return false;
    }
    let segments: Vec<&str> = path.split('/').collect();
    if segments.len() > 4 || !segments.iter().all(|s| portable_segment(s)) {
        return false;
    }
    // Reserved: featured.json, the publishers/ prefix, packages/{id}/ratings.json.
    let reserved = path == "featured.json"
        || segments[0] == "publishers"
        || (segments.len() == 3 && segments[0] == "packages" && segments[2] == "ratings.json");
    !reserved
}

/// Process-unique temp file name inside the store directory.
fn unique_temp_name() -> String {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    format!("{TEMP_PREFIX}{}-{nanos}-{n}", std::process::id())
}

/// Archives as `{dir}/{id}@{version}.hexon`; catalogue documents as plain files under `{dir}/catalog/`.
pub struct FsPackageStore {
    dir: PathBuf,
}

impl FsPackageStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// Delete leftover publish temp files from a crash; returns how many were removed.
    pub fn remove_stale_temp_files(&self) -> usize {
        let Ok(read_dir) = std::fs::read_dir(&self.dir) else {
            return 0;
        };
        read_dir
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with(TEMP_PREFIX))
            .filter(|e| std::fs::remove_file(e.path()).is_ok())
            .count()
    }

    /// [`PackageStore::put`] with the temp-file writer injectable (tests drive a failing disk through this).
    pub async fn put_with_writer<W>(
        &self,
        id: &str,
        version: &str,
        bytes: Bytes,
        write: W,
    ) -> Result<(), StoreError>
    where
        W: FnOnce(&mut std::fs::File, &[u8]) -> std::io::Result<()> + Send + 'static,
    {
        if !portable_segment(id) || !portable_segment(version) {
            return Err(StoreError::Invalid(format!("{id}@{version}")));
        }
        let dest = self.dir.join(format!("{id}@{version}.hexon"));
        let temp = self.dir.join(unique_temp_name());
        tokio::task::spawn_blocking(move || {
            let staged = (|| {
                let mut file = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&temp)?;
                write(&mut file, &bytes)?;
                file.sync_all()
            })();
            // hard_link fails when `dest` exists, so the slot is never overwritten.
            let published = staged.and_then(|()| std::fs::hard_link(&temp, &dest));
            let _ = std::fs::remove_file(&temp);
            published
        })
        .await
        .map_err(|e| StoreError::Backend(e.to_string()))?
        .map_err(StoreError::from)
    }

    /// Directory holding the archives.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Directory a static catalogue tree (`fe-ext index build` output) is dropped into.
    pub fn catalog_dir(&self) -> PathBuf {
        self.dir.join("catalog")
    }
}

#[async_trait]
impl PackageStore for FsPackageStore {
    async fn list(&self) -> Result<Vec<HexonIndexEntry>, StoreError> {
        let dir = self.dir.clone();
        tokio::task::spawn_blocking(move || index::scan_dir(&dir))
            .await
            .map_err(|e| StoreError::Backend(e.to_string()))
    }

    async fn put(&self, id: &str, version: &str, bytes: Bytes) -> Result<(), StoreError> {
        self.put_with_writer(id, version, bytes, |file, bytes| {
            std::io::Write::write_all(file, bytes)
        })
        .await
    }

    async fn catalog_doc(&self, path: &str) -> Result<Option<Bytes>, StoreError> {
        if !valid_catalog_path(path) {
            return Ok(None);
        }
        let file = self.catalog_dir().join(path);
        tokio::task::spawn_blocking(move || {
            let meta = match std::fs::metadata(&file) {
                Ok(m) if m.is_file() => m,
                Ok(_) => return Ok(None),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                Err(e) => return Err(StoreError::Io(e)),
            };
            if meta.len() > MAX_CATALOG_DOC_BYTES {
                return Err(StoreError::Backend(format!(
                    "catalogue document over {MAX_CATALOG_DOC_BYTES} bytes"
                )));
            }
            std::fs::read(&file)
                .map(|b| Some(Bytes::from(b)))
                .map_err(StoreError::Io)
        })
        .await
        .map_err(|e| StoreError::Backend(e.to_string()))?
    }
}
