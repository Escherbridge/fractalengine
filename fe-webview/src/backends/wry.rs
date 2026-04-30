use std::cell::RefCell;
use std::rc::Rc;

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use url::Url;
use wry::{PageLoadEvent, Rect, WebView, WebViewBuilder};

use crate::backend::{BackendEvent, WebViewBackend, WindowGeometry};

// ---------------------------------------------------------------------------
// Win32 popup window — sits above the wgpu swap chain
// ---------------------------------------------------------------------------
//
// WebView2 as a direct child of the wgpu-owned HWND gets rendered *behind*
// the swap chain (black rectangle). Fix: create a borderless popup window
// owned by the main window and embed the webview in that instead.

#[cfg(target_os = "windows")]
mod win32 {
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

    /// Creates a borderless popup window owned by `parent` at the given
    /// screen-space geometry. The window is created VISIBLE and brought
    /// to the front so WebView2 can initialize with a visible parent.
    pub(super) fn create_popup(
        parent: HWND,
        geometry: &WindowGeometry,
    ) -> anyhow::Result<HWND> {
        let class_name = wide_string("FE_WryPortal");
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

    pub(super) fn show(hwnd: HWND) {
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

    pub(super) fn hide(hwnd: HWND) {
        unsafe { ShowWindow(hwnd, SW_HIDE); }
    }

    pub(super) fn move_window(hwnd: HWND, geometry: &WindowGeometry) {
        unsafe {
            SetWindowPos(
                hwnd, HWND_TOP,
                geometry.x, geometry.y,
                geometry.width as i32, geometry.height as i32,
                SWP_NOACTIVATE,
            );
        }
    }

    pub(super) fn destroy(hwnd: HWND) {
        unsafe { DestroyWindow(hwnd); }
    }

    pub(super) fn is_window(hwnd: HWND) -> bool {
        unsafe { IsWindow(hwnd) != 0 }
    }
}

// ---------------------------------------------------------------------------
// HasWindowHandle wrapper for the popup HWND
// ---------------------------------------------------------------------------

#[cfg(target_os = "windows")]
struct PopupHandle(isize);

#[cfg(target_os = "windows")]
impl HasWindowHandle for PopupHandle {
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
struct ParentHandle(RawWindowHandle);

#[cfg(not(target_os = "windows"))]
impl HasWindowHandle for ParentHandle {
    fn window_handle(
        &self,
    ) -> Result<raw_window_handle::WindowHandle<'_>, raw_window_handle::HandleError> {
        Ok(unsafe { raw_window_handle::WindowHandle::borrow_raw(self.0) })
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// The webview fills the entire popup window, so bounds are always at origin.
fn webview_fill_rect(g: &WindowGeometry) -> Rect {
    use wry::dpi::{PhysicalPosition, PhysicalSize};
    Rect {
        position: PhysicalPosition::new(0, 0).into(),
        size: PhysicalSize::new(g.width, g.height).into(),
    }
}

// ---------------------------------------------------------------------------
// WryBackend
// ---------------------------------------------------------------------------

/// Wry-based webview backend (WebView2 on Windows, WebKit on macOS/Linux).
///
/// On Windows the webview lives in a separate popup window (owned by the main
/// Bevy window) to avoid z-order conflicts with the wgpu swap chain. The popup
/// is positioned at screen coordinates matching the right-panel portal area.
///
/// No address bar or browser chrome — navigation happens only through in-page
/// links and programmatic `navigate()` calls.
pub struct WryBackend {
    webview: WebView,
    events: Rc<RefCell<Vec<BackendEvent>>>,
    visible: bool,
    alive: bool,
    /// Last URL we navigated to — skip duplicates to prevent feedback loops.
    current_url: Option<Url>,
    #[cfg(target_os = "windows")]
    popup_hwnd: isize,
}

impl WebViewBackend for WryBackend {
    fn create(
        parent_handle: &RawWindowHandle,
        geometry: WindowGeometry,
        trust_bar_js: &str,
    ) -> anyhow::Result<Self> {
        eprintln!(
            "[PORTAL] WryBackend::create — geometry: x={} y={} w={} h={}",
            geometry.x, geometry.y, geometry.width, geometry.height
        );

        let events: Rc<RefCell<Vec<BackendEvent>>> = Rc::new(RefCell::new(Vec::new()));
        let nav_events = events.clone();
        let load_events = events.clone();

        // --- build wry WebView -------------------------------------------

        let mut builder = WebViewBuilder::new()
            .with_bounds(webview_fill_rect(&geometry))
            .with_visible(true)
            .with_autoplay(true)
            .with_initialization_script(trust_bar_js)
            .with_navigation_handler(move |url: String| {
                match url.parse::<Url>() {
                    Ok(parsed) if crate::security::is_url_allowed(&parsed) => {
                        nav_events
                            .borrow_mut()
                            .push(BackendEvent::UrlChanged(parsed));
                        true
                    }
                    Ok(parsed) => {
                        tracing::warn!(
                            "navigation_handler: blocked navigation to '{parsed}'"
                        );
                        nav_events
                            .borrow_mut()
                            .push(BackendEvent::Error(format!(
                                "Navigation blocked: URL not allowed: {parsed}"
                            )));
                        false
                    }
                    Err(e) => {
                        tracing::warn!(
                            "navigation_handler: blocked navigation to unparseable URL '{url}': {e}"
                        );
                        nav_events
                            .borrow_mut()
                            .push(BackendEvent::Error(format!(
                                "Navigation blocked: invalid URL '{url}': {e}"
                            )));
                        false
                    }
                }
            })
            .with_on_page_load_handler(move |event, _url| {
                if matches!(event, PageLoadEvent::Finished) {
                    load_events.borrow_mut().push(BackendEvent::LoadComplete);
                }
            });

        #[cfg(target_os = "windows")]
        {
            use wry::WebViewBuilderExtWindows;
            builder = builder
                .with_default_context_menus(false)
                .with_browser_accelerator_keys(false);
        }

        // --- platform-specific window strategy ----------------------------

        #[cfg(target_os = "windows")]
        let (webview, popup_hwnd) = {
            let parent_hwnd = match parent_handle {
                RawWindowHandle::Win32(h) => h.hwnd.get() as isize,
                _ => anyhow::bail!("WryBackend on Windows requires a Win32 window handle"),
            };

            // Create visible popup BEFORE building webview — WebView2 needs
            // a visible parent HWND to initialize its rendering pipeline.
            let popup_hwnd = win32::create_popup(parent_hwnd, &geometry)?;
            eprintln!("[PORTAL] popup HWND = {popup_hwnd:#x}");

            let popup = PopupHandle(popup_hwnd);
            eprintln!("[PORTAL] calling build_as_child...");
            let webview = builder
                .build_as_child(&popup)
                .map_err(|e| {
                    eprintln!("[PORTAL] build_as_child FAILED: {e}");
                    win32::destroy(popup_hwnd);
                    anyhow::anyhow!("WryBackend: build_as_child failed: {e}")
                })?;

            eprintln!("[PORTAL] webview built OK — hiding popup until navigate()");
            win32::hide(popup_hwnd);

            (webview, popup_hwnd)
        };

        #[cfg(not(target_os = "windows"))]
        let webview = {
            let parent = ParentHandle(*parent_handle);
            builder
                .build_as_child(&parent)
                .map_err(|e| anyhow::anyhow!("WryBackend: build_as_child failed: {e}"))?
        };

        tracing::info!("WryBackend: initialization complete");

        Ok(Self {
            webview,
            events,
            visible: false,
            alive: true,
            current_url: None,
            #[cfg(target_os = "windows")]
            popup_hwnd,
        })
    }

    fn navigate(&mut self, url: &Url) -> anyhow::Result<()> {
        // Skip if we're already at this URL (prevents feedback loops from
        // the guard/flush command re-write in petal_portal).
        if self.current_url.as_ref() == Some(url) {
            return Ok(());
        }
        eprintln!("[PORTAL] WryBackend::navigate — {url}");
        self.current_url = Some(url.clone());
        self.webview
            .load_url(url.as_str())
            .map_err(|e| anyhow::anyhow!("WryBackend: load_url failed: {e}"))?;
        self.show()?;
        Ok(())
    }

    fn go_back(&mut self) -> anyhow::Result<()> {
        tracing::info!("WryBackend::go_back");
        self.webview
            .evaluate_script("history.back()")
            .map_err(|e| anyhow::anyhow!("WryBackend: go_back failed: {e}"))?;
        Ok(())
    }

    fn show(&mut self) -> anyhow::Result<()> {
        if !self.visible {
            eprintln!("[PORTAL] WryBackend::show");
            #[cfg(target_os = "windows")]
            win32::show(self.popup_hwnd);

            self.webview
                .set_visible(true)
                .map_err(|e| anyhow::anyhow!("WryBackend: set_visible(true) failed: {e}"))?;
            self.visible = true;
        }
        Ok(())
    }

    fn hide(&mut self) -> anyhow::Result<()> {
        if self.visible {
            eprintln!("[PORTAL] WryBackend::hide");
            self.webview
                .set_visible(false)
                .map_err(|e| anyhow::anyhow!("WryBackend: set_visible(false) failed: {e}"))?;

            #[cfg(target_os = "windows")]
            win32::hide(self.popup_hwnd);

            self.visible = false;
            // Reset so the same URL can be re-opened after close.
            self.current_url = None;
        }
        Ok(())
    }

    fn reposition(&mut self, geometry: WindowGeometry) -> anyhow::Result<()> {
        #[cfg(target_os = "windows")]
        win32::move_window(self.popup_hwnd, &geometry);

        self.webview
            .set_bounds(webview_fill_rect(&geometry))
            .map_err(|e| anyhow::anyhow!("WryBackend: set_bounds failed: {e}"))?;
        Ok(())
    }

    fn destroy(&mut self) {
        if self.alive {
            self.alive = false;
            #[cfg(target_os = "windows")]
            win32::destroy(self.popup_hwnd);
        }
    }

    fn drain_events(&mut self) -> Vec<BackendEvent> {
        if !self.alive {
            return Vec::new();
        }

        #[cfg(target_os = "windows")]
        if !win32::is_window(self.popup_hwnd) {
            self.alive = false;
            return vec![BackendEvent::WindowClosed];
        }

        std::mem::take(&mut *self.events.borrow_mut())
    }

    fn is_alive(&self) -> bool {
        self.alive
    }
}

impl Drop for WryBackend {
    fn drop(&mut self) {
        self.destroy();
    }
}

#[cfg(test)]
mod tests {
    /// Task 2.3: Compile-time verification that platform #[cfg] gates are correct.
    /// This test compiling on each platform proves the gates work.
    #[test]
    fn wry_backend_platform_types_compile() {
        #[cfg(target_os = "windows")]
        {
            let _ = std::mem::size_of::<super::PopupHandle>();
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = std::mem::size_of::<super::ParentHandle>();
        }
    }
}
