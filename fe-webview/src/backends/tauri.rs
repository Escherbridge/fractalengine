//! Tauri-powered webview backend (WebView2 on Windows, WebKit on macOS/Linux). See `src/AGENTS.md`.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use raw_window_handle::RawWindowHandle;
use url::Url;
use wry::{PageLoadEvent, WebView, WebViewBuilder};

use crate::backend::{BackendEvent, WebViewBackend, WindowGeometry};
use crate::portal_bundle::{
    self, BridgeWebview, BundleParts, BundleWebview, Decision, NavigationGate, PortalBridge, PortalMounts,
};

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
    fixture_session: Rc<Cell<bool>>,
    guard_unusable: Rc<Cell<bool>>,
    /// Bundle mount table, shared with the `fe-portal` protocol workers.
    mounts: PortalMounts,
    /// Bundle navigation guard state, consulted first by both navigation layers.
    gate: Rc<NavigationGate>,
    /// Windows with `ICoreWebView2_22` (spike P-1) and a registered IPC handler; otherwise bundles are Blocked.
    bundles_supported: bool,
    /// `fe.portal.v1` bridge, shared with the native message handler and the protocol workers.
    bridge: PortalBridge,
    #[cfg(target_os = "windows")]
    popup_hwnd: isize,
    #[cfg(target_os = "windows")]
    fixture_surface: super::fixture_surface::Surface,
}

/// Navigation-handler decision: the bundle gate first (`portal_bundle/AGENTS.md` §navigation), then
/// `security::embedded_allowed` for what it defers; records `UrlChanged`/`Error` on `events`.
fn decide_navigation(
    raw_url: &str,
    events: &Rc<RefCell<Vec<BackendEvent>>>,
    fixture_session: bool,
    gate: &NavigationGate,
) -> bool {
    let attempt = portal_bundle::Attempt {
        raw_url,
        is_redirect: false,
        is_main_frame: true,
    };
    match gate.decide(&attempt) {
        Decision::DeferToRawPolicy => {}
        Decision::Refuse(reason) => {
            tracing::warn!("navigation_handler: refused bundle navigation ({})", reason.as_str());
            events
                .borrow_mut()
                .push(BackendEvent::Error(format!("Navigation blocked: {}", reason.as_str())));
            return false;
        }
        Decision::AllowHostUnload => return true,
        Decision::AllowSame | Decision::AllowEnter | Decision::AllowLeave => {
            if let Ok(url) = raw_url.parse() {
                events.borrow_mut().push(BackendEvent::UrlChanged(url));
            }
            return true;
        }
    }
    let allowed = if fixture_session {
        fe_runtime::qa_web::allows(raw_url)
    } else {
        crate::security::embedded_allowed(raw_url)
    };
    match raw_url.parse::<Url>() {
        Ok(parsed) if allowed => {
            gate.external_committed();
            if !fe_runtime::qa_web::preview_allows(raw_url) {
                events.borrow_mut().push(BackendEvent::UrlChanged(parsed));
            }
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
        let fixture_session = Rc::new(Cell::new(false));
        let guard_unusable = Rc::new(Cell::new(false));
        let load_guard_unusable = guard_unusable.clone();
        let mounts = PortalMounts::new();
        let gate = Rc::new(NavigationGate::new(mounts.clone()));
        let nav_gate = gate.clone();
        let download_gate = gate.clone();
        let protocol_mounts = mounts.clone();
        let protocol_workers = portal_bundle::protocol::ProtocolWorkers::start();
        let bridge = PortalBridge::new();
        let protocol_bridge = bridge.clone();
        // The fixed `ipc-stats` QA operation reads host counters only, never page state (protocol.md §9).
        #[cfg(target_os = "windows")]
        let fixture_surface = {
            let mut surface = super::fixture_surface::Surface::default();
            let stats_bridge = bridge.clone();
            surface.bind_portal(
                mounts.clone(),
                std::sync::Arc::new(move || stats_bridge.stats().to_json()),
            );
            surface
        };

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
            .with_on_page_load_handler(move |event, url| {
                if !load_guard_unusable.get() && matches!(event, PageLoadEvent::Finished) {
                    if let Ok(url) = url.parse() {
                        load_events.borrow_mut().push(BackendEvent::PageLoaded(url));
                    }
                }
            })
            // Registered once: requests resolve against the shared mount table, off the main thread;
            // `ipc::serve` adds the page API for Admitted mounts.
            .with_asynchronous_custom_protocol(
                portal_bundle::SCHEME.to_owned(),
                move |_webview_id, request, responder| {
                    let mounts = protocol_mounts.clone();
                    let bridge = protocol_bridge.clone();
                    protocol_workers.submit(Box::new(move || {
                        responder.respond(portal_bundle::ipc::serve(&mounts, &bridge, &request));
                    }));
                },
            )
            .with_download_started_handler(move |url, _destination| download_gate.allows_download(&url));

        if fe_runtime::qa_web::captured() {
            builder = builder.with_new_window_req_handler(|_, _| wry::NewWindowResponse::Deny);
        } else {
            builder = builder.with_navigation_handler(move |url: String| {
                decide_navigation(&url, &nav_events, false, &nav_gate)
            });
        }

        #[cfg(target_os = "windows")]
        {
            use wry::WebViewBuilderExtWindows;
            builder = builder
                .with_default_context_menus(false)
                .with_browser_accelerator_keys(false)
                .with_https_scheme(true);
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
            if fe_runtime::qa_web::captured()
                && !win32::set_qa_cookie(popup_hwnd, fixture_surface.cookie())
            {
                win32::destroy(popup_hwnd);
                anyhow::bail!("could not bind QA Portal popup lifetime");
            }
            tracing::debug!("TauriBackend: popup HWND = {popup_hwnd:#x}");

            let popup = PopupHandle(popup_hwnd);
            tracing::debug!("TauriBackend: calling build_as_child...");
            let webview = builder.build_as_child(&popup).map_err(|e| {
                tracing::error!("TauriBackend: build_as_child FAILED: {e}");
                win32::destroy(popup_hwnd);
                anyhow::anyhow!("TauriBackend: build_as_child failed: {e}")
            })?;
            if fe_runtime::qa_web::captured() {
                if let Err(error) = super::qa_navigation::install(
                    &webview,
                    fixture_session.clone(),
                    guard_unusable.clone(),
                    events.clone(),
                    fixture_surface.clock.clone(),
                    gate.clone(),
                ) {
                    drop(webview);
                    win32::destroy(popup_hwnd);
                    return Err(error.context("could not install fixture redirect guard"));
                }
            }

            tracing::info!("TauriBackend: webview built OK — hiding popup until navigate()");
            win32::hide(popup_hwnd);

            (webview, popup_hwnd)
        };

        // Spike P-1: without ICoreWebView2_22 wry falls back to a filter that misses worker
        // requests, which would then dial loopback; bundles stay Blocked on such runtimes.
        // P-2: without the native IPC handler an Admitted bundle would render bridgeless, so Block too.
        #[cfg(target_os = "windows")]
        let bundles_supported = {
            use windows::core::Interface;
            use wry::WebViewExtWindows;
            webview
                .webview()
                .cast::<webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2_22>()
                .is_ok()
        } && match install_ipc_handler(&webview, mounts.clone(), bridge.clone()) {
            Ok(()) => true,
            Err(error) => {
                tracing::error!("TauriBackend: {error:#}; portal bundles are disabled");
                false
            }
        };

        #[cfg(not(target_os = "windows"))]
        let webview = {
            let parent = ParentHandle(*parent_handle);
            builder
                .build_as_child(&parent)
                .map_err(|e| anyhow::anyhow!("TauriBackend: build_as_child failed: {e}"))?
        };
        // OD-10 / SP-7: the IPC sender URL is not origin-correct off Windows.
        #[cfg(not(target_os = "windows"))]
        let bundles_supported = false;

        tracing::info!("TauriBackend: initialization complete");

        Ok(Self {
            webview,
            events,
            visible: false,
            alive: true,
            current_url: None,
            fixture_session,
            guard_unusable,
            mounts,
            gate,
            bundles_supported,
            bridge,
            #[cfg(target_os = "windows")]
            popup_hwnd,
            #[cfg(target_os = "windows")]
            fixture_surface,
        })
    }

    fn navigate(&mut self, url: &Url) -> anyhow::Result<()> {
        self.ensure_usable()?;
        anyhow::ensure!(
            crate::security::embedded_allowed(url.as_str()),
            "Navigation blocked by URL policy or unavailable fixture helper"
        );
        self.fixture_session
            .set(fe_runtime::qa_web::preview_allows(url.as_str()));
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
        self.ensure_usable()?;
        #[cfg(target_os = "windows")]
        self.fixture_surface.clock.bump();
        tracing::info!("TauriBackend::go_back");
        self.webview
            .evaluate_script("history.back()")
            .map_err(|e| anyhow::anyhow!("TauriBackend: go_back failed: {e}"))?;
        Ok(())
    }

    fn go_forward(&mut self) -> anyhow::Result<()> {
        self.ensure_usable()?;
        #[cfg(target_os = "windows")]
        self.fixture_surface.clock.bump();
        self.webview.evaluate_script("history.forward()")?;
        Ok(())
    }

    fn reload(&mut self) -> anyhow::Result<()> {
        self.ensure_usable()?;
        #[cfg(target_os = "windows")]
        self.fixture_surface.clock.bump();
        self.webview.evaluate_script("location.reload()")?;
        Ok(())
    }

    fn show(&mut self) -> anyhow::Result<()> {
        self.ensure_usable()?;
        if !self.visible {
            tracing::debug!("TauriBackend::show");
            #[cfg(target_os = "windows")]
            win32::show(self.popup_hwnd);

            self.webview
                .set_visible(true)
                .map_err(|e| anyhow::anyhow!("TauriBackend: set_visible(true) failed: {e}"))?;
            self.visible = true;
        }
        self.bridge.set_visible(true);
        Ok(())
    }

    fn hide(&mut self) -> anyhow::Result<()> {
        self.ensure_usable()?;
        // IPC check 2 fails from here on, before the surface goes.
        self.bridge.set_visible(false);
        #[cfg(target_os = "windows")]
        self.fixture_surface.clock.bump();
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
        self.ensure_usable()?;
        #[cfg(target_os = "windows")]
        self.fixture_surface.clock.bump();
        #[cfg(target_os = "windows")]
        win32::move_window(self.popup_hwnd, &geometry);

        self.webview
            .set_bounds(webview_fill_rect(&geometry))
            .map_err(|e| anyhow::anyhow!("TauriBackend: set_bounds failed: {e}"))?;
        Ok(())
    }

    fn destroy(&mut self) {
        #[cfg(target_os = "windows")]
        self.fixture_surface.clock.bump();
        // No byte of a bundle is served once its webview is gone; unmount also bumps the generation.
        self.mounts.unmount();
        // The broker client closes now, not on some later frame that may never run.
        self.bridge.shutdown();
        if self.alive {
            self.alive = false;
            #[cfg(target_os = "windows")]
            win32::destroy(self.popup_hwnd);
        }
    }

    fn drain_events(&mut self) -> Vec<BackendEvent> {
        if self.guard_unusable.get() {
            self.current_url = None;
            self.visible = false;
            self.bridge.set_visible(false);
            #[cfg(target_os = "windows")]
            win32::hide(self.popup_hwnd);
            return std::mem::take(&mut *self.events.borrow_mut())
                .into_iter()
                .filter(|event| matches!(event, BackendEvent::Error(_)))
                .collect();
        }
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
        self.alive && !self.guard_unusable.get()
    }

    fn fixture_start(
        &mut self,
        command: &fe_runtime::qa_portal::FixtureCommand,
    ) -> Result<(), fe_runtime::qa_debug::QaFailure> {
        #[cfg(target_os = "windows")]
        {
            let visible = self.fixture_visible();
            let result = self.fixture_surface.start(
                &self.webview,
                visible,
                self.guard_unusable.get(),
                command,
            );
            if matches!(&result, Err(error) if error.code == "portal_script_unavailable") {
                super::qa_navigation::abort(&self.webview, &self.guard_unusable, &self.events);
            }
            result
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = command;
            Err(fe_runtime::qa_portal::unavailable())
        }
    }

    fn fixture_poll(&mut self) -> Option<fe_runtime::qa_portal::FixtureOutcome> {
        #[cfg(target_os = "windows")]
        {
            let visible = self.fixture_visible();
            let mut result =
                self.fixture_surface
                    .poll(&self.webview, visible, self.guard_unusable.get());
            if let Some(Ok(completed)) = &result {
                if !self.fixture_surface.receipt_is_current(
                    &self.webview,
                    self.fixture_visible(),
                    self.guard_unusable.get(),
                    &completed.receipt,
                ) {
                    result = Some(Err(fe_runtime::qa_portal::stale()));
                }
            }
            if matches!(&result, Some(Err(error)) if matches!(error.code, "portal_fixture_changed" | "portal_capture_unavailable" | "portal_script_unavailable"))
            {
                super::qa_navigation::abort(&self.webview, &self.guard_unusable, &self.events);
            }
            result
        }
        #[cfg(not(target_os = "windows"))]
        None
    }

    fn fixture_cancel(&mut self) {
        #[cfg(target_os = "windows")]
        if self.fixture_surface.cancel() {
            super::qa_navigation::abort(&self.webview, &self.guard_unusable, &self.events);
        }
    }
}

impl TauriBackend {
    #[cfg(target_os = "windows")]
    fn fixture_visible(&self) -> bool {
        self.alive
            && self.visible
            && win32::is_owned_visible_popup(self.popup_hwnd, self.fixture_surface.cookie())
    }
    fn ensure_usable(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            !self.guard_unusable.get(),
            "Browser disabled after native navigation enforcement failure; restart the application"
        );
        Ok(())
    }
}

impl BundleWebview for TauriBackend {
    fn bundle_parts(&self) -> Option<BundleParts> {
        Some(BundleParts {
            mounts: self.mounts.clone(),
            gate: Rc::clone(&self.gate),
            supported: self.bundles_supported,
        })
    }

    /// Host-minted URLs only (an entry or the unload document); the gate admits them by token, not policy.
    fn bundle_load(&mut self, url: &str) -> anyhow::Result<()> {
        self.ensure_usable()?;
        // Opening a bundle is an explicit host navigation: it ends any fixture session.
        self.fixture_session.set(false);
        self.current_url = None;
        self.webview
            .load_url(url)
            .map_err(|e| anyhow::anyhow!("TauriBackend: bundle load failed: {e}"))
    }

    fn bundle_show(&mut self) -> anyhow::Result<()> {
        WebViewBackend::show(self)
    }

    fn bundle_hide(&mut self) -> anyhow::Result<()> {
        WebViewBackend::hide(self)
    }
}

impl BridgeWebview for TauriBackend {
    fn portal_bridge(&self) -> Option<&PortalBridge> {
        Some(&self.bridge)
    }

    /// Host → page (OD-2): `PostWebMessageAsJson` to the top-level document; the caller already ran the send-time gate.
    fn post_page_message(&self, json: &str) -> anyhow::Result<()> {
        anyhow::ensure!(self.is_alive(), "the portal webview is gone");
        #[cfg(target_os = "windows")]
        {
            use wry::WebViewExtWindows;
            // SAFETY: called on the UI thread that owns the webview; the HSTRING outlives the call.
            unsafe {
                self.webview
                    .webview()
                    .PostWebMessageAsJson(&windows::core::HSTRING::from(json))
            }
            .map_err(|error| anyhow::anyhow!("PostWebMessageAsJson failed: {error}"))
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = json;
            anyhow::bail!("host-to-page delivery is Windows-only in wave 1 (OD-10)")
        }
    }
}

/// Page → host (P-2): a native `WebMessageReceived` handler; wry's `with_ipc_handler` is never registered (SP-9).
#[cfg(target_os = "windows")]
fn install_ipc_handler(webview: &WebView, mounts: PortalMounts, bridge: PortalBridge) -> anyhow::Result<()> {
    use webview2_com::WebMessageReceivedEventHandler;
    use windows::core::PWSTR;
    use wry::WebViewExtWindows;

    let native = webview.webview();
    let handler = WebMessageReceivedEventHandler::create(Box::new(move |_, args| {
        let Some(args) = args else { return Ok(()) };
        let mut source = PWSTR::null();
        // SAFETY: WebView2 fills `source` with a CoTaskMem string; take_pwstr frees it.
        if unsafe { args.Source(&mut source) }.is_err() {
            bridge.ingress(&mounts, "", "", std::time::Instant::now());
            return Ok(());
        }
        let source = webview2_com::take_pwstr(source);
        let mut raw_body = PWSTR::null();
        // A non-string message (postMessage of an object) is never the page API's shape: judge it as empty.
        // SAFETY: as above.
        let body = if unsafe { args.TryGetWebMessageAsString(&mut raw_body) }.is_ok() {
            webview2_com::take_pwstr(raw_body)
        } else {
            String::new()
        };
        bridge.ingress(&mounts, &source, &body, std::time::Instant::now());
        Ok(())
    }));
    let mut token = 0i64;
    // SAFETY: registered on the UI thread that owns the webview; the handler lives as long as it does.
    unsafe { native.add_WebMessageReceived(&handler, &mut token) }
        .map_err(|error| anyhow::anyhow!("portal IPC handler registration failed: {error}"))
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

    /// A gate with nothing mounted: every non-bundle destination defers to the raw policy.
    fn fresh_gate() -> NavigationGate {
        NavigationGate::new(PortalMounts::new())
    }

    #[test]
    fn decide_navigation_allows_public_https_and_emits_url_changed() {
        let events = fresh_events();
        assert!(decide_navigation("https://example.com/", &events, false, &fresh_gate()));
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
        assert!(!decide_navigation(
            "http://192.168.1.1/admin",
            &events,
            false,
            &fresh_gate()
        ));
        let evts = events.borrow();
        assert_eq!(evts.len(), 1);
        assert!(matches!(
            &evts[0],
            BackendEvent::Error(msg) if msg.contains("Navigation blocked")
        ));
    }

    #[test]
    fn decide_navigation_blocks_localhost_and_emits_error() {
        let foreign = crate::portal_bundle::origin::admitted_label("did:key:other", "org.example.other")
            .document_url("web/index.html")
            .expect("foreign url");
        let events = fresh_events();
        assert!(!decide_navigation("http://localhost:8080/", &events, false, &fresh_gate()));
        let evts = events.borrow();
        assert_eq!(evts.len(), 1);
        assert!(matches!(&evts[0], BackendEvent::Error(_)));
        drop(evts);
        assert!(
            !decide_navigation("https://example.com/escape", &events, true, &fresh_gate()),
            "fixture session cannot escape to an otherwise public destination"
        );
        assert!(
            !decide_navigation("http://127.0.0.1:49152/web/one", &events, true, &fresh_gate()),
            "fixture session requires live host capability"
        );
        #[cfg(windows)]
        {
            use super::super::qa_navigation::{
                enforce, NavigationAttempt, TERMINAL_ERROR, UNCONFIRMED_ERROR,
            };
            struct Attempt {
                metadata: Option<(String, bool)>,
                fail_cancel: Option<bool>,
                fail_stop: bool,
                fail_close: bool,
                calls: RefCell<Vec<&'static str>>,
            }
            impl NavigationAttempt for Attempt {
                fn metadata(&self) -> Result<(String, bool), ()> {
                    self.calls.borrow_mut().push("metadata");
                    self.metadata.clone().ok_or(())
                }
                fn set_cancel(&self, cancel: bool) -> Result<(), ()> {
                    self.calls
                        .borrow_mut()
                        .push(if cancel { "cancel" } else { "allow" });
                    if self.fail_cancel == Some(cancel) {
                        Err(())
                    } else {
                        Ok(())
                    }
                }
                fn stop(&self) -> Result<(), ()> {
                    self.calls.borrow_mut().push("stop");
                    if self.fail_stop {
                        Err(())
                    } else {
                        Ok(())
                    }
                }
                fn close(&self) -> Result<(), ()> {
                    self.calls.borrow_mut().push("close");
                    if self.fail_close {
                        Err(())
                    } else {
                        Ok(())
                    }
                }
            }
            for (metadata, fail_cancel, fail_stop, fail_close, expected) in [
                (None, None, true, true, TERMINAL_ERROR),
                (None, Some(true), false, false, TERMINAL_ERROR),
                (
                    Some(("https://example.com/".into(), false)),
                    Some(false),
                    false,
                    false,
                    TERMINAL_ERROR,
                ),
                (None, Some(true), true, false, TERMINAL_ERROR),
                (None, Some(true), false, true, TERMINAL_ERROR),
                (None, Some(true), true, true, UNCONFIRMED_ERROR),
                (
                    Some(("https://example.com/".into(), false)),
                    Some(false),
                    true,
                    true,
                    UNCONFIRMED_ERROR,
                ),
            ] {
                let attempt = Attempt {
                    metadata,
                    fail_cancel,
                    fail_stop,
                    fail_close,
                    calls: RefCell::new(Vec::new()),
                };
                let unusable = Cell::new(false);
                let events = RefCell::new(vec![BackendEvent::UrlChanged(
                    "https://stale.example/".parse().unwrap(),
                )]);
                enforce(
                    &attempt,
                    &Cell::new(false),
                    &unusable,
                    &events,
                    &super::super::fixture_surface::NavigationClock::default(),
                    &fresh_gate(),
                );
                assert!(unusable.get());
                let calls = attempt.calls.borrow();
                assert_eq!(calls.first(), Some(&"cancel"));
                assert_eq!(&calls[calls.len() - 2..], &["stop", "close"]);
                assert!(
                    matches!(&events.borrow()[..], [BackendEvent::Error(message)] if message == expected)
                );
            }
            let one = "https://example.com/web/one";
            let two = "https://example.com/web/two";
            let clock = super::super::fixture_surface::NavigationClock::default();
            let attempt = Attempt {
                metadata: Some((two.into(), false)),
                fail_cancel: None,
                fail_stop: false,
                fail_close: false,
                calls: RefCell::new(Vec::new()),
            };
            assert!(enforce(
                &attempt,
                &Cell::new(false),
                &Cell::new(false),
                &RefCell::new(Vec::new()),
                &clock,
                &fresh_gate(),
            ));
            assert_eq!(&attempt.calls.borrow()[..], &["cancel", "metadata", "allow"]);
            assert_eq!(
                clock.page_state(one, 0, one, true),
                Some(super::super::fixture_surface::PageState::StartingSecond),
                "the policy owner records the same metadata it actually admitted"
            );

            // The same bundle rows on the native owner, which also sees IsRedirected (rows 10-11).
            let (_bundle, mount) =
                crate::portal_bundle::test_fixture::static_mount(&["https://docs.example.org"]);
            let mounts = PortalMounts::new();
            let mount = mounts.install(mount);
            let gate = NavigationGate::new(mounts.clone());
            let entry = mount.entry_url.clone();
            let page = mount.label.document_url("web/app.js").expect("member url");
            let clock = super::super::fixture_surface::NavigationClock::default();
            let run = |url: &str, redirected: bool| {
                let attempt = Attempt {
                    metadata: Some((url.to_owned(), redirected)),
                    fail_cancel: None,
                    fail_stop: false,
                    fail_close: false,
                    calls: RefCell::new(Vec::new()),
                };
                enforce(
                    &attempt,
                    &Cell::new(false),
                    &Cell::new(false),
                    &RefCell::new(Vec::new()),
                    &clock,
                    &gate,
                )
            };
            assert!(!run(&entry, false), "a page cannot enter a bundle without the host's Open");
            gate.expect_entry(entry.clone());
            assert!(!run(&entry, true), "a redirect into a bundle origin is refused even with a token");
            assert!(run(&entry, false), "the host's Open enters once");
            assert!(run(&page, false), "same mounted origin");
            assert!(!run(&foreign, false), "a foreign bundle origin");
            assert!(run("https://docs.example.org/", false), "declared leave");
            assert!(mounts.current().is_none(), "leaving unmounts");
            assert!(
                !run("https://undeclared.example.org/", true),
                "every hop of a leave is judged against the declared list"
            );
            assert!(
                !run("https://example.com/", false),
                "a detached bundle document cannot navigate itself"
            );
            gate.expect_unload();
            assert!(run(crate::portal_bundle::UNLOAD_URL, false), "the host unload, by one-shot token");
            assert!(!run(&page, false), "history back into an unmounted bundle");
        }

        // Bundle origins (protocol.md §6.3): admitted only by exact match against the live mount.
        let (_bundle, mount) =
            crate::portal_bundle::test_fixture::static_mount(&["https://docs.example.org"]);
        let mounts = PortalMounts::new();
        let mount = mounts.install(mount);
        let gate = NavigationGate::new(mounts.clone());
        let entry = mount.entry_url.clone();
        let page = mount.label.document_url("web/app.js").expect("member url");
        let events = fresh_events();
        assert!(!decide_navigation(&entry, &events, false, &gate), "page-initiated entry");
        gate.expect_entry(entry.clone());
        assert!(decide_navigation(&entry, &events, false, &gate), "the host's Open enters once");
        assert!(decide_navigation(&page, &events, false, &gate), "same mounted origin");
        assert!(!decide_navigation(&foreign, &events, false, &gate), "a foreign bundle origin");
        assert!(!decide_navigation("https://undeclared.example.org/", &events, false, &gate));
        assert!(!decide_navigation("http://127.0.0.1/", &events, false, &gate));
        assert!(decide_navigation("https://docs.example.org/guide", &events, false, &gate), "declared leave");
        assert!(mounts.current().is_none(), "leaving unmounts");
        assert!(!decide_navigation(&page, &events, false, &gate), "history back into an unmounted bundle");
        assert!(
            decide_navigation("https://example.com/", &events, false, &gate),
            "after leaving, external behaviour is unchanged"
        );
    }

    #[test]
    fn decide_navigation_blocks_non_http_scheme_and_emits_error() {
        let events = fresh_events();
        assert!(!decide_navigation("ftp://example.com/", &events, false, &fresh_gate()));
        let evts = events.borrow();
        assert_eq!(evts.len(), 1);
        assert!(matches!(&evts[0], BackendEvent::Error(_)));
    }

    #[test]
    fn decide_navigation_blocks_unparseable_url_and_emits_error() {
        let events = fresh_events();
        assert!(!decide_navigation("not a url at all", &events, false, &fresh_gate()));
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
        decide_navigation("http://127.0.0.1/", &events, false, &fresh_gate());
        assert!(
            !events
                .borrow()
                .iter()
                .any(|e| matches!(e, BackendEvent::UrlChanged(_))),
            "blocked navigation must not record a UrlChanged event"
        );
        #[cfg(windows)]
        {
            use super::super::fixture_surface::{
                page_state, png_dimensions, NavigationClock, PageState,
            };
            use fe_runtime::qa_portal::MAX_PNG_BYTES;
            use windows::Win32::System::Com::STREAM_SEEK_SET;
            let one = "http://127.0.0.1:49152/web/one";
            let two = "http://127.0.0.1:49152/web/two";
            assert_eq!(page_state(one, 7, one, 7, None), Some(PageState::Original));
            assert!(
                page_state(one, 7, one, 8, None).is_none(),
                "same URL after reload is a different page generation"
            );
            assert!(
                page_state(one, 7, two, 8, None).is_none(),
                "capture cannot follow navigation"
            );
            assert!(
                page_state(one, 7, two, 8, Some(two)) == Some(PageState::SecondDocument),
                "only the fixed second-link job admits its one expected navigation"
            );
            assert!(page_state(one, 7, two, 9, Some(two)).is_none());
            assert!(
                page_state(one, 7, "https://foreign.example/web/two", 8, Some(two)).is_none()
            );
            let clock = NavigationClock::default();
            assert_eq!(clock.page_state(one, 0, one, true), Some(PageState::Original));
            clock.admit(two.into());
            assert_eq!(
                clock.page_state(one, 0, one, true),
                Some(PageState::StartingSecond),
                "NavigationStarting can precede the Source change and initial script callback"
            );
            assert!(clock.page_state(one, 0, one, false).is_none());
            assert_eq!(
                clock.page_state(one, 0, two, true),
                Some(PageState::SecondDocument)
            );
            assert!(
                clock.page_state(two, 0, two, true).is_none(),
                "a same-page reload cannot prove a new second document"
            );
            clock.bump();
            assert!(
                clock.page_state(one, 0, two, true).is_none(),
                "geometry/history invalidation cancels the admitted transition"
            );
            assert!(page_state(one, 7, one, 8, Some(one)).is_none());
            assert!(
                page_state(one, 7, two, 8, Some("https://foreign.example/web/two")).is_none()
            );
            assert!(png_dimensions(b"\x89PNG\r\n\x1a\ntruncated native callback").is_none());
            let (stream, buffer) = super::super::fixture_stream::create();
            unsafe {
                assert!(stream.SetSize(MAX_PNG_BYTES as u64 + 1).is_err());
                stream
                    .Seek(MAX_PNG_BYTES as i64, STREAM_SEEK_SET, None)
                    .unwrap();
                assert!(stream.Write(b"x".as_ptr().cast(), 1, None).is_err());
                assert!(
                    buffer.lock().unwrap().bytes.is_empty(),
                    "a rejected native write retains no out-of-bound bytes"
                );
                stream.Seek(0, STREAM_SEEK_SET, None).unwrap();
                let mut written = 0;
                assert!(stream
                    .Write(b"PNG".as_ptr().cast(), 3, Some(&mut written))
                    .is_ok());
                assert_eq!(written, 3);
                assert_eq!(buffer.lock().unwrap().bytes, b"PNG");
            }
        }
    }
}
