use serde_json::{Value, json};
use std::{
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

pub type Result<T> = std::result::Result<T, &'static str>;
pub const INPUT_LIMIT: u64 = 64 * 1024;
pub const OUTPUT_LIMIT: usize = 4 * 1024 * 1024;

pub fn text<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty() && s.len() <= 2048)
        .ok_or("invalid_request_field")
}

pub fn read_json(path: &Path, limit: u64) -> Result<Value> {
    let file = read_file(path)?;
    let mut bytes = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "fixture_read_failed")?;
    if bytes.len() as u64 > limit {
        return Err("fixture_input_overflow");
    }
    let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(&bytes);
    serde_json::from_slice(bytes).map_err(|_| "invalid_fixture_json")
}

pub fn read_file(path: &Path) -> Result<File> {
    reject_redirected(path)?;
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(1).custom_flags(0x0020_0000);
    }
    let file = options.open(path).map_err(|_| "fixture_file_unavailable")?;
    let metadata = file.metadata().map_err(|_| "fixture_file_unavailable")?;
    if !metadata.is_file() {
        return Err("fixture_not_regular_file");
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return Err("redirected_fixture_path");
        }
    }
    Ok(file)
}

pub fn reject_redirected(path: &Path) -> Result<()> {
    let meta = std::fs::symlink_metadata(path).map_err(|_| "fixture_path_missing")?;
    if meta.file_type().is_symlink() {
        return Err("redirected_fixture_path");
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if meta.file_attributes() & 0x400 != 0 {
            return Err("redirected_fixture_path");
        }
    }
    Ok(())
}

/// Pin existing ancestors without creating any inspector path.
pub fn pin_existing(path: &Path) -> Result<Vec<File>> {
    let mut parents: Vec<_> = path
        .parent()
        .ok_or("invalid_fixture_path")?
        .ancestors()
        .collect();
    parents.reverse();
    let mut handles = Vec::new();
    for directory in parents {
        reject_redirected(directory)?;
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            options
                .access_mode(0x80)
                .share_mode(1)
                .custom_flags(0x0220_0000);
        }
        let handle = options
            .open(directory)
            .map_err(|_| "fixture_parent_unavailable")?;
        let metadata = handle
            .metadata()
            .map_err(|_| "fixture_parent_unavailable")?;
        if !metadata.is_dir() {
            return Err("fixture_parent_unavailable");
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if metadata.file_attributes() & 0x400 != 0 {
                return Err("redirected_fixture_path");
            }
        }
        handles.push(handle);
    }
    Ok(handles)
}

pub fn create_json(path: &Path, value: &Value) -> Result<()> {
    let _parents = pin_existing(path)?;
    let bytes = serde_json::to_vec(value).map_err(|_| "fixture_encoding_failed")?;
    if bytes.len() > OUTPUT_LIMIT {
        return Err("fixture_output_overflow");
    }
    create_bytes(path, &bytes)
}

pub fn create_bytes(path: &Path, bytes: &[u8]) -> Result<()> {
    let _parents = pin_existing(path)?;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(1).custom_flags(0x0020_0000);
    }
    let mut file = options
        .open(path)
        .map_err(|_| "fixture_output_exists_or_unavailable")?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|_| "fixture_write_failed")
}

pub struct Context {
    pub run: PathBuf,
    pub stage: Value,
    pub instance: String,
    pub _lease: File,
    pub _parents: Vec<File>,
    pub _stage_file: File,
}

impl Context {
    pub fn open(request: &Value, creating: bool) -> Result<Self> {
        let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .ok_or("invalid_build_root")?;
        let root = repo.join(".qa/run");
        let run = PathBuf::from(text(request, "run_dir")?);
        let name = run
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or("invalid_fixture_root")?;
        if !run.is_absolute()
            || run.parent() != Some(root.as_path())
            || name.len() != 19
            || !name.bytes().enumerate().all(|(i, b)| {
                if [8, 15].contains(&i) {
                    b == b'-'
                } else {
                    b.is_ascii_digit()
                }
            })
        {
            return Err("invalid_fixture_root");
        }
        let lease_path = repo.join(".qa/lifecycle.lock");
        let lease_parents = pin_existing(&lease_path)?;
        let lease = lease(request, &lease_path)?;
        let instance = text(request, "preparation_id")?.to_owned();
        if instance.len() != 32 || !instance.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err("invalid_preparation_identity");
        }
        let stage_path = PathBuf::from(text(request, "stage_dir")?);
        if stage_path.parent() != Some(repo.join(".qa/stage").as_path()) {
            return Err("invalid_fixture_stage");
        }
        let stage_parents = pin_existing(&stage_path.join("stage.json"))?;
        let stage_file = read_file(&stage_path.join("stage.json"))?;
        let stage = read_json(&stage_path.join("stage.json"), INPUT_LIMIT)?;
        if stage["kind"] != "fractalengine-qa-stage-v1" {
            return Err("invalid_fixture_stage");
        }
        for key in [
            "source_commit",
            "source_fingerprint",
            "exe_sha256",
            "legacy_fixture_sha256",
        ] {
            let expected_length = if key == "source_commit" { 40 } else { 64 };
            let expected = text(&stage, key)?;
            if expected.len() != expected_length || !expected.bytes().all(|b| b.is_ascii_hexdigit())
            {
                return Err("invalid_fixture_stage");
            }
            if text(request, key)? != text(&stage, key)? {
                return Err("fixture_stage_changed");
            }
        }
        if creating {
            if !root.exists() {
                let _pin = pin_existing(&root)?;
                std::fs::create_dir(&root).map_err(|_| "fixture_root_unavailable")?;
            }
            let _pin = pin_existing(&run)?;
            std::fs::create_dir(&run).map_err(|_| "fixture_root_not_fresh")?;
        }
        let mut parents = pin_existing(&run.join(".pin"))?;
        parents.extend(stage_parents);
        parents.extend(lease_parents);
        Ok(Self {
            run,
            stage,
            instance,
            _lease: lease,
            _parents: parents,
            _stage_file: stage_file,
        })
    }

    pub fn identity(&self) -> Value {
        json!({"schema":"fe-qa-legacy-preparation-v1","fixture":"legacy-adoption","preparation_id":self.instance,"run_dir":self.run,
            "source_commit":self.stage["source_commit"],"source_fingerprint":self.stage["source_fingerprint"],
            "exe_sha256":self.stage["exe_sha256"],"legacy_fixture_sha256":self.stage["legacy_fixture_sha256"]})
    }
}

#[cfg(windows)]
fn lease(request: &Value, expected: &Path) -> Result<File> {
    use std::{ffi::c_void, os::windows::io::FromRawHandle};
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut c_void;
        fn GetCurrentProcess() -> *mut c_void;
        fn DuplicateHandle(
            source: *mut c_void,
            handle: *mut c_void,
            target: *mut c_void,
            out: *mut *mut c_void,
            access: u32,
            inherit: i32,
            options: u32,
        ) -> i32;
        fn CloseHandle(handle: *mut c_void) -> i32;
        fn GetFinalPathNameByHandleW(
            handle: *mut c_void,
            name: *mut u16,
            size: u32,
            flags: u32,
        ) -> u32;
    }
    let pid = request["lease"]["process_id"]
        .as_u64()
        .filter(|v| *v > 0 && *v <= u32::MAX as u64)
        .ok_or("invalid_lifecycle_lease")?;
    let raw = request["lease"]["handle"]
        .as_i64()
        .filter(|v| *v > 0)
        .ok_or("invalid_lifecycle_lease")?;
    unsafe {
        let parent = OpenProcess(0x40, 0, pid as u32);
        if parent.is_null() {
            return Err("lifecycle_lease_unavailable");
        }
        let mut copied = std::ptr::null_mut();
        let ok = DuplicateHandle(
            parent,
            raw as usize as *mut c_void,
            GetCurrentProcess(),
            &mut copied,
            0,
            0,
            2,
        );
        CloseHandle(parent);
        if ok == 0 {
            return Err("lifecycle_lease_unavailable");
        }
        let file = File::from_raw_handle(copied);
        reject_redirected(expected)?;
        if !file
            .metadata()
            .map_err(|_| "invalid_lifecycle_lease")?
            .is_file()
        {
            return Err("invalid_lifecycle_lease");
        }
        let mut buffer = [0_u16; 4096];
        let length =
            GetFinalPathNameByHandleW(copied, buffer.as_mut_ptr(), buffer.len() as u32, 0) as usize;
        if length == 0 || length >= buffer.len() {
            return Err("invalid_lifecycle_lease");
        }
        let actual =
            String::from_utf16(&buffer[..length]).map_err(|_| "invalid_lifecycle_lease")?;
        let expected = expected
            .canonicalize()
            .map_err(|_| "invalid_lifecycle_lease")?;
        if !actual.eq_ignore_ascii_case(&expected.to_string_lossy()) {
            return Err("invalid_lifecycle_lease");
        }
        Ok(file)
    }
}

#[cfg(not(windows))]
fn lease(_: &Value, _: &Path) -> Result<File> {
    Err("fixture_lifecycle_requires_windows")
}
