//! Narrow HWND property ABI; see AGENTS.md for the cross-thread cleanup inference.

use bevy::window::RawHandleWrapper;

#[derive(Clone, Copy)]
pub(crate) struct Process {
    pub pid: u32,
    pub birth: u64,
}

#[cfg(any(all(windows, target_pointer_width = "64"), test))]
pub(super) fn owns(current: u64, expected: u64) -> Result<bool, ()> {
    if current == 0 {
        Ok(false)
    } else if current == expected && expected > 0 && expected <= isize::MAX as u64 {
        Ok(true)
    } else {
        Err(())
    }
}

#[cfg(all(windows, target_pointer_width = "64"))]
mod platform {
    use super::*;
    use std::ffi::c_void;
    type Handle = *mut c_void;
    #[repr(C)]
    #[derive(Default)]
    struct FileTime {
        low: u32,
        high: u32,
    }
    const KEY: &[u16] = &[
        70, 114, 97, 99, 116, 97, 108, 69, 110, 103, 105, 110, 101, 46, 78, 97, 116, 105, 118, 101,
        67, 111, 110, 116, 101, 120, 116, 46, 118, 49, 0,
    ];
    #[link(name = "user32")]
    unsafe extern "system" {
        fn GetPropW(hwnd: Handle, key: *const u16) -> Handle;
        fn SetPropW(hwnd: Handle, key: *const u16, data: Handle) -> i32;
        fn RemovePropW(hwnd: Handle, key: *const u16) -> Handle;
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentProcessId() -> u32;
        fn GetCurrentProcess() -> Handle;
        fn GetProcessTimes(
            process: Handle,
            birth: *mut FileTime,
            exit: *mut FileTime,
            kernel: *mut FileTime,
            user: *mut FileTime,
        ) -> i32;
        fn GetTickCount64() -> u64;
    }
    pub(crate) fn handle(wrapper: &RawHandleWrapper) -> Option<isize> {
        match wrapper.get_window_handle() {
            winit::raw_window_handle::RawWindowHandle::Win32(handle) => Some(handle.hwnd.get()),
            _ => None,
        }
    }
    pub(crate) fn process() -> Option<Process> {
        let (mut birth, mut exit, mut kernel, mut user) = (
            FileTime::default(),
            FileTime::default(),
            FileTime::default(),
            FileTime::default(),
        );
        // Windows SDK ABI; only the current-process pseudo-handle and owned output buffers.
        let ok = unsafe {
            GetProcessTimes(
                GetCurrentProcess(),
                &mut birth,
                &mut exit,
                &mut kernel,
                &mut user,
            )
        };
        let pid = unsafe { GetCurrentProcessId() };
        let birth = (u64::from(birth.high) << 32) | u64::from(birth.low);
        (ok != 0 && pid > 0 && birth > 0).then_some(Process { pid, birth })
    }
    pub(crate) fn tick() -> u64 {
        unsafe { GetTickCount64() }
    }
    pub(crate) fn clear(hwnd: isize, epoch: u64) -> Result<(), ()> {
        // Caller serializes property operations and retains the actual native window.
        unsafe {
            let current = GetPropW(hwnd as Handle, KEY.as_ptr()) as usize as u64;
            if !owns(current, epoch)? {
                return Ok(());
            }
            if RemovePropW(hwnd as Handle, KEY.as_ptr()) as usize as u64 != epoch
                || !GetPropW(hwnd as Handle, KEY.as_ptr()).is_null()
            {
                return Err(());
            }
        }
        Ok(())
    }
    pub(crate) fn publish(hwnd: isize, epoch: u64) -> Result<(), ()> {
        if epoch == 0 || epoch > isize::MAX as u64 {
            return Err(());
        }
        // This stores integer bits, never a dereferenced pointer or a transferred resource.
        unsafe {
            if !GetPropW(hwnd as Handle, KEY.as_ptr()).is_null()
                || SetPropW(hwnd as Handle, KEY.as_ptr(), epoch as usize as Handle) == 0
                || GetPropW(hwnd as Handle, KEY.as_ptr()) as usize as u64 != epoch
            {
                return Err(());
            }
        }
        Ok(())
    }
}
#[cfg(not(all(windows, target_pointer_width = "64")))]
mod platform {
    use super::*;
    pub(crate) fn handle(_: &RawHandleWrapper) -> Option<isize> {
        None
    }
    pub(crate) fn process() -> Option<Process> {
        None
    }
    pub(crate) fn tick() -> u64 {
        0
    }
    pub(crate) fn clear(_: isize, _: u64) -> Result<(), ()> {
        Err(())
    }
    pub(crate) fn publish(_: isize, _: u64) -> Result<(), ()> {
        Err(())
    }
}
pub(super) use platform::{clear, handle, process, publish, tick};
