//! Tauri-powered webview backend (WebView2 on Windows, WebKit on macOS/Linux). See `src/AGENTS.md`.

use std::cell::RefCell;
use std::rc::Rc;

use raw_window_handle::RawWindowHandle;
use url::Url;
use wry::{PageLoadEvent, WebView, WebViewBuilder};

use crate::backend::{BackendEvent, WebViewBackend, WindowGeometry};

use super::win32_popup::webview_fill_rect;
#[cfg(not(target_os = "windows"))]
use super::win32_popup::ParentHandle;
#[cfg(target_os = "windows")]
use super::win32_popup::{win32, PopupHandle};

/// Tauri-powered webview backend; on Windows it lives in a borderless popup owned by the Bevy window.
pub struct TauriBackend {
    webview: WebView,
    events: Rc<RefCell<Vec<BackendEvent>>>,
    visible: bool,
    alive: bool,
    /// Last URL we navigated to — skip duplicates to prevent feedback loops.
    current_url: Option<Url>,
    #[cfg(target_os = "windows")]
    popup_hwnd: isize,
}

/// Navigation-handler decision: validates via `security::is_url_allowed`, records
/// `UrlChanged`/`Error` on `events`, returns whether to allow the navigation.
fn decide_navigation(raw_url: &str, events: &Rc<RefCell<Vec<BackendEvent>>>) -> bool {
    match raw_url.parse::<Url>() {
        Ok(parsed) if crate::security::is_url_allowed(&parsed) => {
            events.borrow_mut().push(BackendEvent::UrlChanged(parsed));
            true
        }
        Ok(parsed) => {
            tracing::warn!("navigation_handler: blocked navigation to '{parsed}'");
            events.borrow_mut().push(BackendEvent::Error(format!(
                "Navigation blocked: URL not allowed: {parsed}"
            )));
            false
        }
        Err(e) => {
            tracing::warn!(
                "navigation_handler: blocked navigation to unparseable URL '{raw_url}': {e}"
            );
            events.borrow_mut().push(BackendEvent::Error(format!(
                "Navigation blocked: invalid URL '{raw_url}': {e}"
            )));
            false
        }
    }
}

impl WebViewBackend for TauriBackend {
    fn create(
        parent_handle: &RawWindowHandle,
        geometry: WindowGeometry,
        trust_bar_js: &str,
    ) -> anyhow::Result<Self> {
        tracing::info!(
            "TauriBackend::create — geometry: x={} y={} w={} h={}",
            geometry.x,
            geometry.y,
            geometry.width,
            geometry.height
        );

        let events: Rc<RefCell<Vec<BackendEvent>>> = Rc::new(RefCell::new(Vec::new()));
        let nav_events = events.clone();
        let load_events = events.clone();

        // --- build wry WebView -------------------------------------------

        // `mut` is consumed by the platform-specific blocks below (e.g. the
        // Windows child-window attach); those cfg out on Linux/macOS, so allow the
        // resulting unused_mut there rather than duplicate the whole builder chain.
        #[allow(unused_mut)]
        let mut builder = WebViewBuilder::new()
            .with_bounds(webview_fill_rect(&geometry))
            .with_visible(true)
            .with_autoplay(true)
            .with_initialization_script(trust_bar_js)
            .with_navigation_handler(move |url: String| decide_navigation(&url, &nav_events))
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
                RawWindowHandle::Win32(h) => h.hwnd.get(),
                _ => anyhow::bail!("TauriBackend on Windows requires a Win32 window handle"),
            };

            // Create visible popup BEFORE building webview — WebView2 needs
            // a visible parent HWND to initialize its rendering pipeline.
            let popup_hwnd = win32::create_popup(parent_hwnd, &geometry)?;
            tracing::debug!("TauriBackend: popup HWND = {popup_hwnd:#x}");

            let popup = PopupHandle(popup_hwnd);
            tracing::debug!("TauriBackend: calling build_as_child...");
            let webview = builder.build_as_child(&popup).map_err(|e| {
                tracing::error!("TauriBackend: build_as_child FAILED: {e}");
                win32::destroy(popup_hwnd);
                anyhow::anyhow!("TauriBackend: build_as_child failed: {e}")
            })?;

            tracing::info!("TauriBackend: webview built OK — hiding popup until navigate()");
            win32::hide(popup_hwnd);

            (webview, popup_hwnd)
        };

        #[cfg(not(target_os = "windows"))]
        let webview = {
            let parent = ParentHandle(*parent_handle);
            builder
                .build_as_child(&parent)
                .map_err(|e| anyhow::anyhow!("TauriBackend: build_as_child failed: {e}"))?
        };

        tracing::info!("TauriBackend: initialization complete");

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
        tracing::info!("TauriBackend::navigate — {url}");
        self.webview
            .load_url(url.as_str())
            .map_err(|e| anyhow::anyhow!("TauriBackend: load_url failed: {e}"))?;
        // Record only after load_url succeeds so a failed load stays retryable.
        self.current_url = Some(url.clone());
        self.show()?;
        Ok(())
    }

    fn go_back(&mut self) -> anyhow::Result<()> {
        tracing::info!("TauriBackend::go_back");
        self.webview
            .evaluate_script("history.back()")
            .map_err(|e| anyhow::anyhow!("TauriBackend: go_back failed: {e}"))?;
        Ok(())
    }

    fn show(&mut self) -> anyhow::Result<()> {
        if !self.visible {
            tracing::debug!("TauriBackend::show");
            #[cfg(target_os = "windows")]
            win32::show(self.popup_hwnd);

            self.webview
                .set_visible(true)
                .map_err(|e| anyhow::anyhow!("TauriBackend: set_visible(true) failed: {e}"))?;
            self.visible = true;
        }
        Ok(())
    }

    fn hide(&mut self) -> anyhow::Result<()> {
        if self.visible {
            tracing::debug!("TauriBackend::hide");
            self.webview
                .set_visible(false)
                .map_err(|e| anyhow::anyhow!("TauriBackend: set_visible(false) failed: {e}"))?;

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
            .map_err(|e| anyhow::anyhow!("TauriBackend: set_bounds failed: {e}"))?;
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

        let drained = std::mem::take(&mut *self.events.borrow_mut());
        // Track in-page navigations so the navigate() dedup matches reality.
        for evt in &drained {
            if let BackendEvent::UrlChanged(url) = evt {
                self.current_url = Some(url.clone());
            }
        }
        drained
    }

    fn is_alive(&self) -> bool {
        self.alive
    }
}

impl Drop for TauriBackend {
    fn drop(&mut self) {
        self.destroy();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verify TauriBackend implements WebViewBackend (compile-time check).
    #[test]
    fn tauri_backend_type_exists() {
        fn _check_trait<T: WebViewBackend>() {}
        fn _check_tauri_implements_trait() {
            _check_trait::<TauriBackend>();
        }
    }

    fn fresh_events() -> Rc<RefCell<Vec<BackendEvent>>> {
        Rc::new(RefCell::new(Vec::new()))
    }

    #[test]
    fn decide_navigation_allows_public_https_and_emits_url_changed() {
        let events = fresh_events();
        assert!(decide_navigation("https://example.com/", &events));
        let evts = events.borrow();
        assert_eq!(evts.len(), 1);
        assert!(matches!(
            &evts[0],
            BackendEvent::UrlChanged(u) if u.as_str() == "https://example.com/"
        ));
    }

    #[test]
    fn decide_navigation_blocks_private_ip_and_emits_error() {
        let events = fresh_events();
        assert!(!decide_navigation("http://192.168.1.1/admin", &events));
        let evts = events.borrow();
        assert_eq!(evts.len(), 1);
        assert!(matches!(
            &evts[0],
            BackendEvent::Error(msg) if msg.contains("Navigation blocked")
        ));
    }

    #[test]
    fn decide_navigation_blocks_localhost_and_emits_error() {
        let events = fresh_events();
        assert!(!decide_navigation("http://localhost:8080/", &events));
        let evts = events.borrow();
        assert_eq!(evts.len(), 1);
        assert!(matches!(&evts[0], BackendEvent::Error(_)));
    }

    #[test]
    fn decide_navigation_blocks_non_http_scheme_and_emits_error() {
        let events = fresh_events();
        assert!(!decide_navigation("ftp://example.com/", &events));
        let evts = events.borrow();
        assert_eq!(evts.len(), 1);
        assert!(matches!(&evts[0], BackendEvent::Error(_)));
    }

    #[test]
    fn decide_navigation_blocks_unparseable_url_and_emits_error() {
        let events = fresh_events();
        assert!(!decide_navigation("not a url at all", &events));
        let evts = events.borrow();
        assert_eq!(evts.len(), 1);
        assert!(matches!(
            &evts[0],
            BackendEvent::Error(msg) if msg.contains("invalid URL")
        ));
    }

    #[test]
    fn decide_navigation_does_not_emit_url_changed_when_blocked() {
        let events = fresh_events();
        decide_navigation("http://127.0.0.1/", &events);
        assert!(
            !events
                .borrow()
                .iter()
                .any(|e| matches!(e, BackendEvent::UrlChanged(_))),
            "blocked navigation must not record a UrlChanged event"
        );
    }
}
