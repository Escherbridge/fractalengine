//! Bounded native CapturePreview sink; see src/AGENTS.md section QA Portal evidence.

use fe_runtime::qa_portal::MAX_PNG_BYTES;
use std::ffi::c_void;
use std::sync::{Arc, Mutex};
use windows::core::{implement, Error, Ref, HRESULT};
use windows::Win32::System::Com::*;

#[derive(Default)]
pub(super) struct State {
    pub bytes: Vec<u8>,
    cursor: usize,
}
pub(super) type Buffer = Arc<Mutex<State>>;

#[implement(IStream, ISequentialStream)]
struct BoundedStream {
    state: Buffer,
}

pub(super) fn create() -> (IStream, Buffer) {
    let state = Arc::new(Mutex::new(State {
        bytes: Vec::with_capacity(MAX_PNG_BYTES),
        cursor: 0,
    }));
    (
        BoundedStream {
            state: state.clone(),
        }
        .into(),
        state,
    )
}

const INVALID: HRESULT = HRESULT(0x8003_0001u32 as i32);
const FULL: HRESULT = HRESULT(0x8003_0070u32 as i32);
const POINTER: HRESULT = HRESULT(0x8000_4003u32 as i32);
fn unsupported<T>() -> windows::core::Result<T> {
    Err(Error::from_hresult(INVALID))
}

impl ISequentialStream_Impl for BoundedStream_Impl {
    fn Read(&self, pv: *mut c_void, cb: u32, read: *mut u32) -> HRESULT {
        if !read.is_null() {
            unsafe {
                *read = 0;
            }
        }
        if cb > 0 && pv.is_null() {
            return POINTER;
        }
        let Ok(mut state) = self.state.lock() else {
            return INVALID;
        };
        let count = (cb as usize).min(state.bytes.len().saturating_sub(state.cursor));
        if count > 0 {
            unsafe {
                std::ptr::copy_nonoverlapping(
                    state.bytes.as_ptr().add(state.cursor),
                    pv.cast(),
                    count,
                );
            }
        }
        state.cursor += count;
        if !read.is_null() {
            unsafe {
                *read = count as u32;
            }
        }
        HRESULT(if count == cb as usize { 0 } else { 1 })
    }
    fn Write(&self, pv: *const c_void, cb: u32, written: *mut u32) -> HRESULT {
        if !written.is_null() {
            unsafe {
                *written = 0;
            }
        }
        if cb > 0 && pv.is_null() {
            return POINTER;
        }
        let Ok(mut state) = self.state.lock() else {
            return INVALID;
        };
        let Some(end) = state
            .cursor
            .checked_add(cb as usize)
            .filter(|end| *end <= MAX_PNG_BYTES)
        else {
            return FULL;
        };
        if end > state.bytes.len() {
            state.bytes.resize(end, 0);
        }
        if cb > 0 {
            unsafe {
                std::ptr::copy_nonoverlapping(
                    pv.cast(),
                    state.bytes.as_mut_ptr().add(state.cursor),
                    cb as usize,
                );
            }
        }
        state.cursor = end;
        if !written.is_null() {
            unsafe {
                *written = cb;
            }
        }
        HRESULT(0)
    }
}

impl IStream_Impl for BoundedStream_Impl {
    fn Seek(
        &self,
        distance: i64,
        origin: STREAM_SEEK,
        position: *mut u64,
    ) -> windows::core::Result<()> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| Error::from_hresult(INVALID))?;
        let base = match origin {
            STREAM_SEEK_SET => 0,
            STREAM_SEEK_CUR => state.cursor,
            STREAM_SEEK_END => state.bytes.len(),
            _ => return unsupported(),
        };
        let next = (base as i128) + i128::from(distance);
        if !(0..=MAX_PNG_BYTES as i128).contains(&next) {
            return unsupported();
        }
        state.cursor = next as usize;
        if !position.is_null() {
            unsafe {
                *position = next as u64;
            }
        }
        Ok(())
    }
    fn SetSize(&self, size: u64) -> windows::core::Result<()> {
        if size > MAX_PNG_BYTES as u64 {
            return Err(Error::from_hresult(FULL));
        }
        self.state
            .lock()
            .map_err(|_| Error::from_hresult(INVALID))?
            .bytes
            .resize(size as usize, 0);
        Ok(())
    }
    fn Stat(&self, stat: *mut STATSTG, _: &STATFLAG) -> windows::core::Result<()> {
        if stat.is_null() {
            return Err(Error::from_hresult(POINTER));
        }
        let state = self
            .state
            .lock()
            .map_err(|_| Error::from_hresult(INVALID))?;
        unsafe {
            *stat = STATSTG {
                r#type: STGTY_STREAM.0 as u32,
                cbSize: state.bytes.len() as u64,
                ..Default::default()
            };
        }
        Ok(())
    }
    fn Commit(&self, _: &STGC) -> windows::core::Result<()> {
        Ok(())
    }
    fn Revert(&self) -> windows::core::Result<()> {
        unsupported()
    }
    fn CopyTo(
        &self,
        _: Ref<'_, IStream>,
        _: u64,
        _: *mut u64,
        _: *mut u64,
    ) -> windows::core::Result<()> {
        unsupported()
    }
    fn LockRegion(&self, _: u64, _: u64, _: &LOCKTYPE) -> windows::core::Result<()> {
        unsupported()
    }
    fn UnlockRegion(&self, _: u64, _: u64, _: u32) -> windows::core::Result<()> {
        unsupported()
    }
    fn Clone(&self) -> windows::core::Result<IStream> {
        unsupported()
    }
}
