//! Shared Win32 popup window helpers for embedding a webview above the wgpu swap chain. See `src/AGENTS.md`.

#[cfg(feature = "backend-tauri")]
use crate::backend::WindowGeometry;

#[cfg(target_os = "windows")]
pub(crate) mod win32 {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use std::ptr;

    use crate::backend::WindowGeometry;

    type HWND = isize;
    type HINSTANCE = isize;
    type HMENU = isize;
    type LPVOID = *mut std::ffi::c_void;
    type LPCWSTR = *const u16;
    type DWORD = u32;
    type BOOL = i32;
    type ATOM = u16;
    type UINT = u32;
    type WPARAM = usize;
    type LPARAM = isize;
    type LRESULT = isize;

    type WNDPROC = Option<unsafe extern "system" fn(HWND, UINT, WPARAM, LPARAM) -> LRESULT>;

    const WS_POPUP: DWORD = 0x8000_0000;
    const WS_VISIBLE: DWORD = 0x1000_0000;
    const WS_CLIPSIBLINGS: DWORD = 0x0400_0000;
    const WS_CLIPCHILDREN: DWORD = 0x0200_0000;
    const WS_EX_TOOLWINDOW: DWORD = 0x0000_0080;
    const SW_SHOW: i32 = 5;
    const SW_HIDE: i32 = 0;
    const CS_OWNDC: UINT = 0x0020;
    const SWP_NOACTIVATE: UINT = 0x0010;
    const SWP_SHOWWINDOW: UINT = 0x0040;
    const HWND_TOP: HWND = 0;
    const HWND_TOPMOST: HWND = -1;
    const HWND_NOTOPMOST: HWND = -2;
    const COLOR_WINDOW: isize = 5;

    #[repr(C)]
    struct WNDCLASSEXW {
        cb_size: UINT,
        style: UINT,
        lpfn_wnd_proc: WNDPROC,
        cb_cls_extra: i32,
        cb_wnd_extra: i32,
        h_instance: HINSTANCE,
        h_icon: isize,
        h_cursor: isize,
        hbr_background: isize,
        lpsz_menu_name: LPCWSTR,
        lpsz_class_name: LPCWSTR,
        h_icon_sm: isize,
    }

    extern "system" {
        fn CreateWindowExW(
            dw_ex_style: DWORD, lp_class_name: LPCWSTR, lp_window_name: LPCWSTR,
            dw_style: DWORD, x: i32, y: i32, n_width: i32, n_height: i32,
            h_wnd_parent: HWND, h_menu: HMENU, h_instance: HINSTANCE, lp_param: LPVOID,
        ) -> HWND;
        fn DestroyWindow(h_wnd: HWND) -> BOOL;
        fn ShowWindow(h_wnd: HWND, n_cmd_show: i32) -> BOOL;
        fn IsWindow(h_wnd: HWND) -> BOOL;
        fn SetWindowPos(
            h_wnd: HWND, h_wnd_insert_after: HWND, x: i32, y: i32, cx: i32, cy: i32,
            u_flags: UINT,
        ) -> BOOL;
        fn GetModuleHandleW(lp_module_name: LPCWSTR) -> HINSTANCE;
        fn RegisterClassExW(lpwcx: *const WNDCLASSEXW) -> ATOM;
        fn DefWindowProcW(h_wnd: HWND, msg: UINT, w_param: WPARAM, l_param: LPARAM) -> LRESULT;
        fn GetLastError() -> DWORD;
    }

    unsafe extern "system" fn wnd_proc(
        h_wnd: HWND, msg: UINT, w_param: WPARAM, l_param: LPARAM,
    ) -> LRESULT {
        unsafe { DefWindowProcW(h_wnd, msg, w_param, l_param) }
    }

    fn wide_string(s: &str) -> Vec<u16> {
        OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect()
    }

    /// Creates a borderless popup window owned by `parent`, created visible and fronted so WebView2 can init.
    pub(crate) fn create_popup(
        parent: HWND,
        geometry: &WindowGeometry,
    ) -> anyhow::Result<HWND> {
        let class_name = wide_string("FE_Portal");
        let h_instance = unsafe { GetModuleHandleW(ptr::null()) };

        let wc = WNDCLASSEXW {
            cb_size: std::mem::size_of::<WNDCLASSEXW>() as UINT,
            style: CS_OWNDC,
            lpfn_wnd_proc: Some(wnd_proc),
            cb_cls_extra: 0,
            cb_wnd_extra: 0,
            h_instance,
            h_icon: 0,
            h_cursor: 0,
            // Give the popup a background brush so it's not transparent.
            hbr_background: COLOR_WINDOW + 1,
            lpsz_menu_name: ptr::null(),
            lpsz_class_name: class_name.as_ptr(),
            h_icon_sm: 0,
        };

        // May "fail" if already registered — fine, CreateWindowExW still works.
        unsafe { RegisterClassExW(&wc) };

        let window_name = wide_string("FractalEngine Portal");

        tracing::info!(
            "win32::create_popup — parent={parent:#x} x={} y={} w={} h={}",
            geometry.x, geometry.y, geometry.width, geometry.height
        );

        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_TOOLWINDOW,
                class_name.as_ptr(),
                window_name.as_ptr(),
                // Start VISIBLE so WebView2 has a visible parent during init.
                WS_POPUP | WS_VISIBLE | WS_CLIPSIBLINGS | WS_CLIPCHILDREN,
                geometry.x,
                geometry.y,
                geometry.width as i32,
                geometry.height as i32,
                parent,
                0,
                h_instance,
                ptr::null_mut(),
            )
        };

        if hwnd == 0 {
            let err = unsafe { GetLastError() };
            anyhow::bail!(
                "CreateWindowExW failed for portal popup (GetLastError={err})"
            );
        }

        tracing::info!("win32::create_popup — hwnd={hwnd:#x} created and visible");

        // Briefly set TOPMOST to ensure it's above the Bevy window,
        // then drop back to NOTOPMOST so it doesn't stay always-on-top.
        unsafe {
            SetWindowPos(
                hwnd, HWND_TOPMOST,
                0, 0, 0, 0,
                SWP_NOACTIVATE | SWP_SHOWWINDOW | 0x0001 /*SWP_NOSIZE*/ | 0x0002 /*SWP_NOMOVE*/,
            );
            SetWindowPos(
                hwnd, HWND_NOTOPMOST,
                0, 0, 0, 0,
                SWP_NOACTIVATE | SWP_SHOWWINDOW | 0x0001 | 0x0002,
            );
        }

        Ok(hwnd)
    }

    pub(crate) fn show(hwnd: HWND) {
        unsafe {
            ShowWindow(hwnd, SW_SHOW);
            // Place at top of z-order (above Bevy window) without being
            // always-on-top above other applications.
            SetWindowPos(
                hwnd, HWND_TOP,
                0, 0, 0, 0,
                SWP_NOACTIVATE | 0x0001 /*SWP_NOSIZE*/ | 0x0002 /*SWP_NOMOVE*/,
            );
        }
    }

    pub(crate) fn hide(hwnd: HWND) {
        unsafe { ShowWindow(hwnd, SW_HIDE); }
    }

    pub(crate) fn move_window(hwnd: HWND, geometry: &WindowGeometry) {
        unsafe {
            SetWindowPos(
                hwnd, HWND_TOP,
                geometry.x, geometry.y,
                geometry.width as i32, geometry.height as i32,
                SWP_NOACTIVATE,
            );
        }
    }

    pub(crate) fn destroy(hwnd: HWND) {
        unsafe { DestroyWindow(hwnd); }
    }

    pub(crate) fn is_window(hwnd: HWND) -> bool {
        unsafe { IsWindow(hwnd) != 0 }
    }
}

// HasWindowHandle wrapper for the popup HWND.
#[cfg(target_os = "windows")]
pub(crate) struct PopupHandle(pub(crate) isize);

#[cfg(target_os = "windows")]
impl raw_window_handle::HasWindowHandle for PopupHandle {
    fn window_handle(
        &self,
    ) -> Result<raw_window_handle::WindowHandle<'_>, raw_window_handle::HandleError> {
        let h = raw_window_handle::Win32WindowHandle::new(
            std::num::NonZeroIsize::new(self.0)
                .expect("popup HWND must be non-zero"),
        );
        // SAFETY: the HWND is valid — we just created it.
        Ok(unsafe { raw_window_handle::WindowHandle::borrow_raw(h.into()) })
    }
}

// Non-Windows: wrap the raw parent handle directly (no popup indirection).
#[cfg(not(target_os = "windows"))]
pub(crate) struct ParentHandle(pub(crate) raw_window_handle::RawWindowHandle);

#[cfg(not(target_os = "windows"))]
impl raw_window_handle::HasWindowHandle for ParentHandle {
    fn window_handle(
        &self,
    ) -> Result<raw_window_handle::WindowHandle<'_>, raw_window_handle::HandleError> {
        Ok(unsafe { raw_window_handle::WindowHandle::borrow_raw(self.0) })
    }
}

/// The webview fills the entire popup window, so bounds are always at origin.
#[cfg(feature = "backend-tauri")]
pub(crate) fn webview_fill_rect(g: &WindowGeometry) -> wry::Rect {
    use wry::dpi::{PhysicalPosition, PhysicalSize};
    wry::Rect {
        position: PhysicalPosition::new(0, 0).into(),
        size: PhysicalSize::new(g.width, g.height).into(),
    }
}
