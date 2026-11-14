//! Portal WebView2 spike runner (protocol.md §10, §12); see `fe-webview/src/portal_bundle/AGENTS.md` §spikes.
//!
//! `cargo run -p fe-webview --example portal_spikes -- <SP-1|SP-2|...|all> [--out <file>]`

#[cfg(not(all(windows, feature = "backend-tauri")))]
fn main() {
    println!("WebView2 spikes run on Windows with backend-tauri");
}

#[cfg(all(windows, feature = "backend-tauri"))]
mod spikes_a;
// EXT-07 adds SP-5..SP-9: `mod spikes_b;` plus `spikes_b::SPIKES` in `TABLE` below.
// #[cfg(all(windows, feature = "backend-tauri"))]
// mod spikes_b;

#[cfg(all(windows, feature = "backend-tauri"))]
pub use harness::{Harness, Outcome, RequestRecord, Spike};

/// Every spike the runner can execute, in run order.
#[cfg(all(windows, feature = "backend-tauri"))]
const TABLE: &[&[Spike]] = &[spikes_a::SPIKES];

#[cfg(all(windows, feature = "backend-tauri"))]
const DEFAULT_OUT: &str = "target/portal-spikes.jsonl";
/// One spike gets this long, then it is recorded as failed.
#[cfg(all(windows, feature = "backend-tauri"))]
const SPIKE_TIMEOUT_SECONDS: u64 = 30;
/// The wry release the harness is written against (fe-webview/Cargo.toml).
#[cfg(all(windows, feature = "backend-tauri"))]
const WRY_VERSION: &str = "0.54";

/// `SP-1`, `sp1` and `Sp-1` are one spike.
#[cfg(all(windows, feature = "backend-tauri"))]
fn normalize(id: &str) -> String {
    id.chars().filter(|c| *c != '-').collect::<String>().to_ascii_uppercase()
}

#[cfg(all(windows, feature = "backend-tauri"))]
fn main() {
    use std::io::Write as _;

    let mut selection: Vec<String> = Vec::new();
    let mut out_path = DEFAULT_OUT.to_owned();
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        if argument == "--out" {
            match arguments.next() {
                Some(path) => out_path = path,
                None => usage("--out needs a file path"),
            }
        } else {
            selection.extend(argument.split(',').map(normalize));
        }
    }
    if selection.is_empty() {
        usage("name a spike (SP-1, SP-1,SP-3 or all)");
    }
    let all = selection.iter().any(|id| id == "ALL");
    let table: Vec<&Spike> = TABLE.iter().flat_map(|group| group.iter()).collect();
    let chosen: Vec<&Spike> = table
        .iter()
        .copied()
        .filter(|spike| all || selection.contains(&normalize(spike.id)))
        .collect();
    let unknown: Vec<&String> = selection
        .iter()
        .filter(|id| *id != "ALL" && !table.iter().any(|spike| normalize(spike.id) == **id))
        .collect();
    if !unknown.is_empty() {
        usage(&format!("no such spike in this build: {unknown:?}"));
    }

    let wry_version = WRY_VERSION;
    let webview2_version = wry::webview_version().unwrap_or_else(|error| format!("unknown ({error})"));
    let mut harness = match Harness::new() {
        Ok(harness) => harness,
        Err(error) => {
            eprintln!("harness failed to start: {error:#}");
            std::process::exit(1);
        }
    };
    let mut out = match std::fs::OpenOptions::new().create(true).append(true).open(&out_path) {
        Ok(file) => file,
        Err(error) => {
            eprintln!("cannot open {out_path}: {error}");
            std::process::exit(1);
        }
    };

    let mut failed = 0usize;
    for spike in chosen {
        harness.begin_spike(std::time::Duration::from_secs(SPIKE_TIMEOUT_SECONDS));
        let mut outcome = (spike.run)(&mut harness);
        if harness.timed_out() {
            outcome.pass = false;
            outcome.notes = format!("{} [{SPIKE_TIMEOUT_SECONDS} s timeout reached]", outcome.notes);
        }
        println!(
            "{} {}: {}\n    {}",
            if outcome.pass { "PASS" } else { "FAIL" },
            spike.id,
            outcome.observed,
            outcome.notes
        );
        let line = serde_json::json!({
            "spike": spike.id,
            "question": spike.question,
            "observed": outcome.observed,
            "pass": outcome.pass,
            "notes": outcome.notes,
            "wry_version": wry_version,
            "webview2_version": webview2_version,
        });
        if let Err(error) = writeln!(out, "{line}") {
            eprintln!("cannot append to {out_path}: {error}");
        }
        if !outcome.pass {
            failed += 1;
        }
    }
    println!("results appended to {out_path}");
    drop(harness);
    std::process::exit(i32::from(failed > 0));
}

#[cfg(all(windows, feature = "backend-tauri"))]
fn usage(message: &str) -> ! {
    eprintln!("{message}\nusage: cargo run -p fe-webview --example portal_spikes -- <SP-1|...|all> [--out <file>]");
    std::process::exit(2);
}

/// The window, webview, sealed fixture bundle and observation logs every spike drives.
#[cfg(all(windows, feature = "backend-tauri"))]
mod harness {
    use std::cell::{Cell, RefCell};
    use std::collections::HashMap;
    use std::io::Write as _;
    use std::path::PathBuf;
    use std::rc::Rc;
    use std::sync::{Arc, Mutex, PoisonError};
    use std::time::{Duration, Instant};

    use ed25519_dalek::SigningKey;
    use fe_webview::ipc::BundleState;
    use fe_webview::portal_bundle::origin::{admitted_label, Label};
    use fe_webview::portal_bundle::protocol::serve;
    use fe_webview::portal_bundle::{
        prepare, AdmissionVerdict, Attempt, BundleIdentity, BundleResolver, BundleSource, Decision, NavigationGate,
        PortalMounts, SCHEME,
    };
    use http::{Request, Response, StatusCode};
    use serde_json::json;
    use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2;
    use webview2_com::FrameNavigationStartingEventHandler;
    use windows::core::{HSTRING, PWSTR};
    use winit::application::ApplicationHandler;
    use winit::dpi::LogicalSize;
    use winit::event::WindowEvent;
    use winit::event_loop::{ActiveEventLoop, EventLoop};
    use winit::platform::pump_events::EventLoopExtPumpEvents;
    use winit::window::{Window, WindowAttributes, WindowId};
    use wry::{WebView, WebViewBuilder, WebViewExtWindows};

    const PACKAGE: &str = "org.example.portal-spikes";
    /// Harness-owned scheme standing in for an external page (the production gate admits only public https origins).
    const SPIKE_SCHEME: &str = "fe-spike";
    /// Where a page reports back: answered by the harness before the production handler sees the request.
    const REPORT_PATH: &str = "/__spike/report";
    /// Pump granularity while waiting.
    const PUMP_SLEEP: Duration = Duration::from_millis(2);

    /// What a spike observed.
    pub struct Outcome {
        pub observed: String,
        pub pass: bool,
        pub notes: String,
    }

    /// One row of the §10 table with its runner.
    pub struct Spike {
        pub id: &'static str,
        pub question: &'static str,
        pub run: fn(&mut Harness) -> Outcome,
    }

    /// One request the `fe-portal` handler answered.
    #[derive(Debug, Clone)]
    pub struct RequestRecord {
        pub uri: String,
        pub method: String,
        pub service_worker: Option<String>,
        pub status: u16,
    }

    /// Host-side state the navigation callback reads and writes (it runs inside `pump`).
    #[derive(Default)]
    struct NavigationHook {
        /// Messages to post from inside the callback when navigation to the SP-3 page starts.
        posts_on_navigation: Cell<u32>,
        posted: Cell<u32>,
        navigation_started: Cell<bool>,
        native: RefCell<Option<ICoreWebView2>>,
    }

    impl NavigationHook {
        fn on_spike_page_navigation(&self) {
            self.navigation_started.set(true);
            let count = self.posts_on_navigation.get();
            if let Some(native) = self.native.borrow().as_ref() {
                for _ in 0..count {
                    let seq = self.posted.get() + 1;
                    if post_message(native, seq) {
                        self.posted.set(seq);
                    }
                }
            }
        }
    }

    /// `PostWebMessageAsJson` of one numbered envelope; false when WebView2 refused it.
    fn post_message(native: &ICoreWebView2, seq: u32) -> bool {
        let json = json!({ "spike": "sp3", "seq": seq }).to_string();
        // SAFETY: called on the UI thread that owns the webview.
        unsafe { native.PostWebMessageAsJson(&HSTRING::from(json)) }.is_ok()
    }

    struct App {
        window: Option<Window>,
    }

    impl ApplicationHandler for App {
        fn resumed(&mut self, event_loop: &ActiveEventLoop) {
            if self.window.is_none() {
                let attributes = WindowAttributes::default()
                    .with_title("FractalEngine portal spikes")
                    .with_inner_size(LogicalSize::new(960.0, 720.0));
                match event_loop.create_window(attributes) {
                    Ok(window) => self.window = Some(window),
                    Err(error) => eprintln!("window creation failed: {error}"),
                }
            }
        }

        fn window_event(&mut self, _: &ActiveEventLoop, _: WindowId, _: WindowEvent) {}
    }

    /// A sealed fixture bundle on disk, removed on drop.
    struct TempFile(PathBuf);

    impl Drop for TempFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    struct FixedResolver {
        path: PathBuf,
        publisher_did: String,
    }

    impl BundleResolver for FixedResolver {
        fn resolve(&self, _node_id: &str) -> Result<BundleSource, BundleState> {
            Ok(BundleSource { asset_id: "spike-asset".into(), blob_path: Some(self.path.clone()), revoked: None })
        }

        fn verdict(&self, _identity: &BundleIdentity) -> AdmissionVerdict {
            AdmissionVerdict::Admitted { publisher_did: self.publisher_did.clone() }
        }
    }

    /// Bundle A: one page whose behaviour is chosen by `?spike=` (app.js), plus the service-worker script SP-4 registers.
    fn archive() -> Vec<u8> {
        let portal = json!({
            "schema": "fe-portal-bundle/1", "entry": "web/index.html", "title": "Portal spikes",
            "csp": { "connect": [], "img": [], "font": [], "media": [], "frame": ["https://example.org"],
                     "wasm": false, "inline_styles": false },
            "navigation": [],
            "subscriptions": { "node": false, "readings": false, "queries": [] },
            "commands": {}, "hooks": [], "streams": [], "ui_intents": []
        });
        let extension = json!({ "manifest_version": 2, "id": PACKAGE, "version": "1.0.0", "kind": "web-bundle", "portal": portal });
        let manifest = json!({
            "schema_version": "1.0.0", "hexon_id": PACKAGE, "hexon_type": "plugin",
            "publisher_did": "did:key:z6Mkunsealed", "version": "1.0.0", "name": "Portal spikes",
            "created_at": "2026-11-13T00:00:00Z", "updated_at": "2026-11-13T00:00:00Z"
        });
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default();
        let members: [(&str, Vec<u8>); 6] = [
            ("manifest.json", serde_json::to_vec(&manifest).unwrap_or_default()),
            ("entries.json", br#"{"entries":[]}"#.to_vec()),
            ("plugin/extension.json", serde_json::to_vec(&extension).unwrap_or_default()),
            (
                "web/index.html",
                b"<!doctype html><title>spike A</title><script src=\"/app.js\"></script>".to_vec(),
            ),
            ("web/app.js", super::spikes_a::BUNDLE_SCRIPT.as_bytes().to_vec()),
            ("web/sw.js", b"self.addEventListener('fetch', () => {});".to_vec()),
        ];
        for (name, bytes) in members {
            zip.start_file(name, options).expect("start zip member");
            zip.write_all(&bytes).expect("write zip member");
        }
        zip.finish().expect("finish zip").into_inner()
    }

    /// Pages served on the harness scheme (external stand-ins).
    fn spike_page(path: &str) -> Option<&'static str> {
        match path {
            "/b.html" => Some(super::spikes_a::PAGE_B),
            "/probe.html" => Some(super::spikes_a::PAGE_PROBE),
            _ => None,
        }
    }

    fn plain(status: StatusCode) -> Response<Vec<u8>> {
        Response::builder()
            .status(status)
            .header("access-control-allow-origin", "*")
            .header("cache-control", "no-store")
            .body(Vec::new())
            .unwrap_or_default()
    }

    /// Record `query` when `request` is a page report; the answer is an empty 204.
    fn take_report(request: &Request<Vec<u8>>, reports: &Mutex<Vec<String>>) -> Option<Response<Vec<u8>>> {
        let uri = request.uri().to_string();
        let marker = uri.find(REPORT_PATH)?;
        let query = uri[marker + REPORT_PATH.len()..].trim_start_matches('?').to_owned();
        reports.lock().unwrap_or_else(PoisonError::into_inner).push(query);
        Some(plain(StatusCode::NO_CONTENT))
    }

    pub struct Harness {
        // Field order is drop order: the webview goes before the window that hosts it.
        webview: WebView,
        native: ICoreWebView2,
        event_loop: EventLoop<()>,
        app: App,
        pub mounts: PortalMounts,
        pub gate: Rc<NavigationGate>,
        /// Document URL of bundle A (no query).
        pub entry_url: String,
        pub label_a: Label,
        /// A well-formed label nothing mounts.
        pub label_unmounted: Label,
        pub requests: Arc<Mutex<Vec<RequestRecord>>>,
        reports: Arc<Mutex<Vec<String>>>,
        /// Every URL wry's navigation callback saw.
        pub navigations: Rc<RefCell<Vec<String>>>,
        /// Every URL the native `FrameNavigationStarting` handler saw.
        pub frame_navigations: Arc<Mutex<Vec<String>>>,
        bypass: Rc<RefCell<Vec<String>>>,
        hook: Rc<NavigationHook>,
        deadline: Instant,
        _fixture: TempFile,
    }

    impl Harness {
        pub fn new() -> anyhow::Result<Self> {
            let key = SigningKey::from_bytes(&[7u8; 32]);
            let publisher_did = fe_format::verifying_key_to_did(&key.verifying_key());
            let sealed = fe_format::seal(&archive(), &key)?;
            let path = std::env::temp_dir().join(format!("fe-portal-spikes-{}.hexon", ulid::Ulid::new()));
            std::fs::write(&path, &sealed)?;
            let fixture = TempFile(path.clone());
            let resolver = FixedResolver { path, publisher_did: publisher_did.clone() };
            let mount = prepare(&resolver, "spike-node", u64::MAX)
                .map_err(|state| anyhow::anyhow!("fixture bundle did not mount: {state:?}"))?;
            let label_a = mount.label.clone();
            let entry_url = mount.entry_url.clone();
            let mounts = PortalMounts::new();
            mounts.install(mount);
            let gate = Rc::new(NavigationGate::new(mounts.clone()));
            let label_unmounted = admitted_label("did:key:zSpikeUnmounted", "org.example.unmounted");

            let mut event_loop = EventLoop::new()?;
            let mut app = App { window: None };
            let started = Instant::now();
            while app.window.is_none() && started.elapsed() < Duration::from_secs(5) {
                let _ = event_loop.pump_app_events(Some(Duration::ZERO), &mut app);
                std::thread::sleep(PUMP_SLEEP);
            }
            let window = app.window.as_ref().ok_or_else(|| anyhow::anyhow!("no window within 5 s"))?;

            let requests: Arc<Mutex<Vec<RequestRecord>>> = Arc::default();
            let reports: Arc<Mutex<Vec<String>>> = Arc::default();
            let navigations: Rc<RefCell<Vec<String>>> = Rc::default();
            let frame_navigations: Arc<Mutex<Vec<String>>> = Arc::default();
            let bypass: Rc<RefCell<Vec<String>>> = Rc::default();
            let hook: Rc<NavigationHook> = Rc::default();

            let protocol_mounts = mounts.clone();
            let protocol_requests = requests.clone();
            let protocol_reports = reports.clone();
            let spike_reports = reports.clone();
            let navigation_gate = gate.clone();
            let navigation_log = navigations.clone();
            let navigation_bypass = bypass.clone();
            let navigation_hook = hook.clone();

            use wry::WebViewBuilderExtWindows as _;
            let webview = WebViewBuilder::new()
                // The production handler, wrapped only to record each request and to answer page reports.
                .with_asynchronous_custom_protocol(SCHEME.to_owned(), move |_id, request, responder| {
                    let response = match take_report(&request, &protocol_reports) {
                        Some(response) => response,
                        None => {
                            let response = serve(&protocol_mounts, &request);
                            protocol_requests.lock().unwrap_or_else(PoisonError::into_inner).push(RequestRecord {
                                uri: request.uri().to_string(),
                                method: request.method().to_string(),
                                service_worker: request
                                    .headers()
                                    .get("service-worker")
                                    .and_then(|value| value.to_str().ok())
                                    .map(str::to_owned),
                                status: response.status().as_u16(),
                            });
                            response
                        }
                    };
                    responder.respond(response);
                })
                .with_asynchronous_custom_protocol(SPIKE_SCHEME.to_owned(), move |_id, request, responder| {
                    let response = match take_report(&request, &spike_reports) {
                        Some(response) => response,
                        None => match spike_page(request.uri().path()) {
                            Some(html) => Response::builder()
                                .status(StatusCode::OK)
                                .header("content-type", "text/html; charset=utf-8")
                                .header("cache-control", "no-store")
                                .body(html.as_bytes().to_vec())
                                .unwrap_or_default(),
                            None => plain(StatusCode::NOT_FOUND),
                        },
                    };
                    responder.respond(response);
                })
                // Mirrors the production navigation callback: the real gate decides every bundle-related URL.
                .with_navigation_handler(move |url: String| {
                    navigation_log.borrow_mut().push(url.clone());
                    if url.contains("fe-spike.") {
                        if url.contains("/b.html") {
                            navigation_hook.on_spike_page_navigation();
                        }
                        return true;
                    }
                    if navigation_bypass.borrow().iter().any(|allowed| *allowed == url) {
                        return true;
                    }
                    let attempt = Attempt { raw_url: &url, is_redirect: false, is_main_frame: true };
                    !matches!(navigation_gate.decide(&attempt), Decision::Refuse(_))
                })
                .with_https_scheme(true)
                .build(window)?;

            let native = webview.webview();
            *hook.native.borrow_mut() = Some(native.clone());
            let frame_log = frame_navigations.clone();
            let frame_handler = FrameNavigationStartingEventHandler::create(Box::new(move |_, args| {
                if let Some(args) = args {
                    let mut raw = PWSTR::null();
                    // SAFETY: on success WebView2 hands over a CoTaskMem string that `take_pwstr` frees.
                    if unsafe { args.Uri(&mut raw) }.is_ok() {
                        let uri = webview2_com::take_pwstr(raw);
                        frame_log.lock().unwrap_or_else(PoisonError::into_inner).push(uri);
                    }
                }
                Ok(())
            }));
            let mut token = 0i64;
            // SAFETY: registered on the UI thread that owns the webview.
            unsafe { native.add_FrameNavigationStarting(&frame_handler, &mut token) }
                .map_err(|error| anyhow::anyhow!("FrameNavigationStarting registration failed: {error}"))?;

            Ok(Self {
                webview,
                native,
                event_loop,
                app,
                mounts,
                gate,
                entry_url,
                label_a,
                label_unmounted,
                requests,
                reports,
                navigations,
                frame_navigations,
                bypass,
                hook,
                deadline: Instant::now(),
                _fixture: fixture,
            })
        }

        /// Reset every log and start the spike's clock.
        pub fn begin_spike(&mut self, limit: Duration) {
            self.deadline = Instant::now() + limit;
            self.requests.lock().unwrap_or_else(PoisonError::into_inner).clear();
            self.reports.lock().unwrap_or_else(PoisonError::into_inner).clear();
            self.frame_navigations.lock().unwrap_or_else(PoisonError::into_inner).clear();
            self.navigations.borrow_mut().clear();
            self.bypass.borrow_mut().clear();
            self.hook.posts_on_navigation.set(0);
            self.hook.posted.set(0);
            self.hook.navigation_started.set(false);
        }

        pub fn timed_out(&self) -> bool {
            Instant::now() >= self.deadline
        }

        /// Dispatch window and WebView2 messages once.
        pub fn pump_once(&mut self) {
            let _ = self.event_loop.pump_app_events(Some(Duration::ZERO), &mut self.app);
            std::thread::sleep(PUMP_SLEEP);
        }

        /// Pump for `span` (bounded by the spike deadline).
        pub fn pump_for(&mut self, span: Duration) {
            let end = (Instant::now() + span).min(self.deadline);
            while Instant::now() < end {
                self.pump_once();
            }
        }

        /// Pump until `condition` holds; false on `timeout` or the spike deadline.
        pub fn wait_until(&mut self, timeout: Duration, mut condition: impl FnMut(&Self) -> bool) -> bool {
            let end = (Instant::now() + timeout).min(self.deadline);
            while Instant::now() < end {
                if condition(self) {
                    return true;
                }
                self.pump_once();
            }
            condition(self)
        }

        /// The query string of the first page report with `id=<id>`.
        pub fn report(&self, id: &str) -> Option<String> {
            let reports = self.reports.lock().unwrap_or_else(PoisonError::into_inner);
            reports.iter().find(|query| query_map(query).get("id").is_some_and(|found| found == id)).cloned()
        }

        /// Wait for a page report and return its fields.
        pub fn wait_report(&mut self, id: &str, timeout: Duration) -> Option<HashMap<String, String>> {
            self.wait_until(timeout, |harness| harness.report(id).is_some());
            self.report(id).map(|query| query_map(&query))
        }

        pub fn clear_reports(&self) {
            self.reports.lock().unwrap_or_else(PoisonError::into_inner).clear();
        }

        /// URL of a harness-scheme page.
        pub fn spike_url(&self, path_and_query: &str) -> String {
            format!("https://{SPIKE_SCHEME}.localhost{path_and_query}")
        }

        /// Open bundle A as the host does (entry token, then load) with `?<query>`.
        pub fn enter_bundle(&mut self, query: &str) -> anyhow::Result<()> {
            let url = format!("{}?{query}", self.entry_url);
            self.gate.expect_entry(url.clone());
            self.webview.load_url(&url)?;
            Ok(())
        }

        /// Load an external harness page (never judged by the gate).
        pub fn load_spike_page(&mut self, path_and_query: &str) -> anyhow::Result<()> {
            self.webview.load_url(&self.spike_url(path_and_query))?;
            Ok(())
        }

        /// Load `url` bypassing the gate once: lets a request reach the handler that the gate would refuse.
        pub fn load_ungated(&mut self, url: &str) -> anyhow::Result<()> {
            self.bypass.borrow_mut().push(url.to_owned());
            self.webview.load_url(url)?;
            Ok(())
        }

        pub fn eval(&self, script: &str) -> anyhow::Result<()> {
            self.webview.evaluate_script(script)?;
            Ok(())
        }

        /// Post one numbered envelope to the current document; counted in [`Self::posted`].
        pub fn post_numbered(&self) -> bool {
            let seq = self.hook.posted.get() + 1;
            let delivered = post_message(&self.native, seq);
            if delivered {
                self.hook.posted.set(seq);
            }
            delivered
        }

        /// Ask the hook to post `count` envelopes from inside the navigation callback to the SP-3 page.
        pub fn post_inside_navigation_callback(&self, count: u32) {
            self.hook.posts_on_navigation.set(count);
        }

        pub fn posted(&self) -> u32 {
            self.hook.posted.get()
        }

        /// True once NavigationStarting for the SP-3 page has fired.
        pub fn navigation_started(&self) -> bool {
            self.hook.navigation_started.get()
        }

        /// Reset the post counters between SP-3 variants.
        pub fn reset_posts(&self) {
            self.hook.posted.set(0);
            self.hook.navigation_started.set(false);
            self.hook.posts_on_navigation.set(0);
        }

        /// Requests the `fe-portal` handler answered, as a snapshot.
        pub fn handler_requests(&self) -> Vec<RequestRecord> {
            self.requests.lock().unwrap_or_else(PoisonError::into_inner).clone()
        }

        pub fn frame_navigation_urls(&self) -> Vec<String> {
            self.frame_navigations.lock().unwrap_or_else(PoisonError::into_inner).clone()
        }

        pub fn wry_navigation_urls(&self) -> Vec<String> {
            self.navigations.borrow().clone()
        }
    }

    /// `a=1&b=2` into a map (percent-decoded).
    fn query_map(query: &str) -> HashMap<String, String> {
        url::form_urlencoded::parse(query.as_bytes()).map(|(key, value)| (key.into_owned(), value.into_owned())).collect()
    }
}
