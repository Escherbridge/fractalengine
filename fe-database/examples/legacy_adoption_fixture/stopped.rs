use super::files::Result;
#[cfg(windows)]
use super::files::text;
use serde_json::Value;

pub fn verify(current: &Value, request: &Value) -> Result<()> {
    for key in [
        "process_identity",
        "descendant_identities",
        "started_at",
        "bootstrap_verse_id",
    ] {
        if current.get(key) != request.get(key) {
            return Err("fixture_lifetime_changed");
        }
    }
    let descendants = current["descendant_identities"]
        .as_array()
        .filter(|a| a.len() <= 32)
        .ok_or("invalid_fixture_lifetime")?;
    for identity in std::iter::once(&current["process_identity"]).chain(descendants) {
        absent(identity)?;
    }
    Ok(())
}

#[cfg(windows)]
fn absent(identity: &Value) -> Result<()> {
    use std::ffi::c_void;
    #[repr(C)]
    struct FileTime {
        low: u32,
        high: u32,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut c_void;
        fn GetLastError() -> u32;
        fn CloseHandle(handle: *mut c_void) -> i32;
        fn GetProcessTimes(
            handle: *mut c_void,
            creation: *mut FileTime,
            exit: *mut FileTime,
            kernel: *mut FileTime,
            user: *mut FileTime,
        ) -> i32;
        fn QueryFullProcessImageNameW(
            handle: *mut c_void,
            flags: u32,
            name: *mut u16,
            length: *mut u32,
        ) -> i32;
        fn GetExitCodeProcess(handle: *mut c_void, code: *mut u32) -> i32;
    }
    let pid = identity["pid"]
        .as_u64()
        .filter(|v| *v > 0 && *v <= u32::MAX as u64)
        .ok_or("invalid_fixture_lifetime")?;
    let ticks = text(identity, "start_ticks")?
        .parse::<u64>()
        .map_err(|_| "invalid_fixture_lifetime")?;
    let expected = text(identity, "exe_path")?;
    unsafe {
        let handle = OpenProcess(0x1000, 0, pid as u32);
        if handle.is_null() {
            return if GetLastError() == 87 {
                Ok(())
            } else {
                Err("fixture_lifetime_unverifiable")
            };
        }
        let result = (|| {
            let mut code = 0;
            if GetExitCodeProcess(handle, &mut code) == 0 {
                return Err("fixture_lifetime_unverifiable");
            }
            if code != 259 {
                return Ok(());
            }
            let blank = || FileTime { low: 0, high: 0 };
            let (mut creation, mut exit, mut kernel, mut user) =
                (blank(), blank(), blank(), blank());
            if GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user) == 0 {
                return Err("fixture_lifetime_unverifiable");
            }
            let actual = ((creation.high as u64) << 32 | creation.low as u64)
                .checked_add(504_911_232_000_000_000)
                .ok_or("invalid_fixture_lifetime")?;
            let mut image = [0_u16; 2048];
            let mut length = image.len() as u32;
            if QueryFullProcessImageNameW(handle, 0, image.as_mut_ptr(), &mut length) == 0 {
                return Err("fixture_lifetime_unverifiable");
            }
            let image = String::from_utf16(&image[..length as usize])
                .map_err(|_| "fixture_lifetime_unverifiable")?;
            if actual == ticks && image.eq_ignore_ascii_case(expected) {
                return Err("fixture_lifetime_still_running");
            }
            Ok(())
        })();
        CloseHandle(handle);
        result
    }
}

#[cfg(not(windows))]
fn absent(_: &Value) -> Result<()> {
    Err("fixture_lifecycle_requires_windows")
}
