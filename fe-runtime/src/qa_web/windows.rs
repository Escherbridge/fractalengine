//! Retained process identity and exact listener ownership; see ../AGENTS.md.

use std::ffi::{c_void, OsString};
use std::os::windows::ffi::OsStringExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{ensure, Context};

const DOTNET_FILETIME_OFFSET: u64 = 504_911_232_000_000_000;
const DOTNET_UNIX_OFFSET: u64 = 621_355_968_000_000_000;

#[repr(C)]
#[derive(Default)]
struct FileTime {
    low: u32,
    high: u32,
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut c_void;
    fn WaitForSingleObject(handle: *mut c_void, milliseconds: u32) -> u32;
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
        path: *mut u16,
        length: *mut u32,
    ) -> i32;
}

#[link(name = "iphlpapi")]
unsafe extern "system" {
    fn GetExtendedTcpTable(
        table: *mut c_void,
        size: *mut u32,
        order: i32,
        family: u32,
        table_class: u32,
        reserved: u32,
    ) -> u32;
}

pub(super) struct Helper {
    handle: OwnedHandle,
    pid: u32,
    start_ticks: u64,
    executable: PathBuf,
    port: u16,
}

impl Helper {
    pub(super) fn capture(
        pid: u32,
        start_ticks: u64,
        executable: &Path,
        port: u16,
    ) -> anyhow::Result<(Self, SystemTime)> {
        let raw = unsafe { OpenProcess(0x0010_1000, 0, pid) };
        ensure!(!raw.is_null(), "cannot retain QA helper process");
        let helper = Self {
            handle: unsafe { OwnedHandle::from_raw_handle(raw) },
            pid,
            start_ticks,
            executable: executable.canonicalize()?,
            port,
        };
        ensure!(
            helper.identity_matches(),
            "QA helper lifetime/executable mismatch"
        );
        let unix_ticks = start_ticks
            .checked_sub(DOTNET_UNIX_OFFSET)
            .context("invalid helper start time")?;
        let created = UNIX_EPOCH
            .checked_add(Duration::from_nanos(
                unix_ticks
                    .checked_mul(100)
                    .context("helper start time overflow")?,
            ))
            .context("helper start time overflow")?;
        Ok((helper, created))
    }

    pub(super) fn is_live_listener(&self) -> bool {
        self.identity_matches() && listener_owned_by(self.pid, self.port) && self.identity_matches()
    }

    fn identity_matches(&self) -> bool {
        let handle = self.handle.as_raw_handle();
        if unsafe { WaitForSingleObject(handle, 0) } != 258 {
            return false;
        }
        let (mut creation, mut exit, mut kernel, mut user) = (
            FileTime::default(),
            FileTime::default(),
            FileTime::default(),
            FileTime::default(),
        );
        if unsafe { GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user) } == 0
        {
            return false;
        }
        let ticks = (u64::from(creation.high) << 32) | u64::from(creation.low);
        if ticks.checked_add(DOTNET_FILETIME_OFFSET) != Some(self.start_ticks) {
            return false;
        }
        let mut path = vec![0u16; 32_768];
        let mut length = path.len() as u32;
        if unsafe { QueryFullProcessImageNameW(handle, 0, path.as_mut_ptr(), &mut length) } == 0 {
            return false;
        }
        let actual = PathBuf::from(OsString::from_wide(&path[..length as usize]));
        actual
            .canonicalize()
            .is_ok_and(|actual| actual == self.executable)
    }
}

fn listener_owned_by(pid: u32, port: u16) -> bool {
    let mut size = 0u32;
    if unsafe { GetExtendedTcpTable(std::ptr::null_mut(), &mut size, 0, 2, 3, 0) } != 122 {
        return false;
    }
    for _ in 0..2 {
        if !(4..=1024 * 1024).contains(&size) {
            return false;
        }
        let mut words = vec![0u32; (size as usize).div_ceil(4)];
        let status =
            unsafe { GetExtendedTcpTable(words.as_mut_ptr().cast(), &mut size, 0, 2, 3, 0) };
        if status == 122 {
            continue;
        }
        if status != 0 || size < 4 || size as usize > words.len() * 4 {
            return false;
        }
        let count = words[0] as usize;
        if count > ((size as usize - 4) / 24) {
            return false;
        }
        return words[1..1 + count * 6].chunks_exact(6).any(|row| {
            row[0] == 2
                && row[1] == u32::from_ne_bytes([127, 0, 0, 1])
                && u16::from_be(row[2] as u16) == port
                && row[5] == pid
        });
    }
    false
}
