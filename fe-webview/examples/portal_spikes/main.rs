//! Portal WebView2 spike runner (protocol.md §10, §12); see `fe-webview/src/portal_bundle/AGENTS.md` §spikes.
//!
//! `cargo run -p fe-webview --example portal_spikes -- <SP-1|SP-2|...|SP-9|all> [--out <file>]`
//! (SP-9 re-runs this binary as a child with `--sp9-child <wry|native>`.)

#[cfg(not(all(windows, feature = "backend-tauri")))]
fn main() {
    println!("WebView2 spikes run on Windows with backend-tauri");
}

#[cfg(all(windows, feature = "backend-tauri"))]
mod spikes_a;
#[cfg(all(windows, feature = "backend-tauri"))]
mod spikes_b;

#[cfg(all(windows, feature = "backend-tauri"))]
pub use harness::{Admission, Fixture, Harness, HarnessOptions, IpcRecord, Outcome, RequestRecord, Spike};

/// Every spike the runner can execute, in run order.
#[cfg(all(windows, feature = "backend-tauri"))]
const TABLE: &[&[Spike]] = &[spikes_a::SPIKES, spikes_b::SPIKES];

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
    let mut arguments = std::env::args().skip(1).peekable();
    // SP-9's child process: one variant, one result line on stdout, never the runner's output file.
    if arguments.peek().map(String::as_str) == Some(spikes_b::CHILD_FLAG) {
        arguments.next();
        let mode = arguments.next().unwrap_or_default();
        std::process::exit(spikes_b::sp9_child(&mode));
    }
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
    eprintln!("{message}\nusage: cargo run -p fe-webview --example portal_spikes -- <SP-1|...|SP-9|all> [--out <file>]");
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
    use fe_webview::portal_bundle::feeds::FeedSource;
    use fe_webview::portal_bundle::limits::MAX_SOURCE_BYTES;
    use fe_webview::portal_bundle::origin::{admitted_label, Label};
    use fe_webview::portal_bundle::protocol::serve;
    use fe_webview::portal_bundle::{
        prepare, AdmissionVerdict, Attempt, BundleIdentity, BundleResolver, BundleSource, Decision, Mount,
        NavigationGate, PortalMounts, SCHEME,
    };
    use http::{Request, Response, StatusCode};
    use serde_json::json;
    use webview2_com::Microsoft::Web::WebView2::Win32::{ICoreWebView2, COREWEBVIEW2_PERMISSION_KIND};
    use webview2_com::{FrameNavigationStartingEventHandler, PermissionRequestedEventHandler, WebMessageReceivedEventHandler};
    use windows::core::{HSTRING, PWSTR};
    use winit::application::ApplicationHandler;
    use winit::dpi::LogicalSize;
    use winit::event::WindowEvent;
    use winit::event_loop::{ActiveEventLoop, EventLoop};
    use winit::platform::pump_events::EventLoopExtPumpEvents;
    use winit::window::{Window, WindowAttributes, WindowId};
    use wry::{WebView, WebViewBuilder, WebViewExtWindows};

    const PACKAGE: &str = "org.example.portal-spikes";
    /// Bundle B: same publisher, another package, so another admitted label (SP-6).
    const PACKAGE_B: &str = "org.example.portal-spikes-b";
    /// Stored prefix of each IPC sender URL.
    const SOURCE_KEPT_CHARS: usize = 512;
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

    /// One page-to-host message the native `WebMessageReceived` logger saw.
    #[derive(Debug, Clone)]
    pub struct IpcRecord {
        /// The sender URL, cut to its first 512 characters.
        pub source: String,
        /// Full sender URL length in UTF-16 units.
        pub source_length: usize,
        /// The string body; `None` when dropped or not a string.
        pub body: Option<String>,
        /// Dropped unread because the sender URL is over `MAX_SOURCE_BYTES` (P-2).
        pub dropped: bool,
    }

    /// Which sealed fixture a mount reads.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Fixture {
        A,
        B,
    }

    /// How the fixture resolver judges the build.
    pub enum Admission {
        /// Admitted for the sealing publisher: an `a-` label.
        Admitted,
        /// No admission record: a static `s-` mount with these feeds.
        Static(FeedSource),
    }

    /// Runner switches; SP-9's child sets `wry_ipc_handler`.
    #[derive(Debug, Clone, Copy, Default)]
    pub struct HarnessOptions {
        /// Register wry's `with_ipc_handler` (the handler P-2 replaced).
        pub wry_ipc_handler: bool,
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
        admitted: bool,
        feeds: FeedSource,
    }

    impl BundleResolver for FixedResolver {
        fn resolve(&self, _node_id: &str) -> Result<BundleSource, BundleState> {
            Ok(BundleSource { asset_id: "spike-asset".into(), blob_path: Some(self.path.clone()), revoked: None })
        }

        fn verdict(&self, _identity: &BundleIdentity) -> AdmissionVerdict {
            if self.admitted {
                AdmissionVerdict::Admitted { publisher_did: self.publisher_did.clone() }
            } else {
                AdmissionVerdict::Unadmitted { can_admit: false }
            }
        }

        fn feeds(&self, _node_id: &str) -> FeedSource {
            self.feeds.clone()
        }
    }

    /// A fresh mount of the sealed fixture at `path` (a `Mount` is installed by value, so each open prepares one).
    fn mount_fixture(path: PathBuf, publisher_did: &str, admission: Admission) -> anyhow::Result<Mount> {
        let (admitted, feeds) = match admission {
            Admission::Admitted => (true, FeedSource::default()),
            Admission::Static(feeds) => (false, feeds),
        };
        let resolver = FixedResolver { path, publisher_did: publisher_did.to_owned(), admitted, feeds };
        prepare(&resolver, "spike-node", u64::MAX).map_err(|state| anyhow::anyhow!("fixture bundle did not mount: {state:?}"))
    }

    /// A fixture bundle for `package`: one page whose behaviour is chosen by `?spike=` (app.js, app_b.js), plus the
    /// service-worker script SP-4 registers.
    fn archive(package: &str) -> Vec<u8> {
        let portal = json!({
            "schema": "fe-portal-bundle/1", "entry": "web/index.html", "title": "Portal spikes",
            "csp": { "connect": [], "img": [], "font": [], "media": [], "frame": ["https://example.org"],
                     "wasm": false, "inline_styles": false },
            "navigation": [],
            "subscriptions": { "node": false, "readings": false, "queries": [] },
            "commands": {}, "hooks": [], "streams": [], "ui_intents": []
        });
        let extension = json!({ "manifest_version": 2, "id": package, "version": "1.0.0", "kind": "web-bundle", "portal": portal });
        let manifest = json!({
            "schema_version": "1.0.0", "hexon_id": package, "hexon_type": "plugin",
            "publisher_did": "did:key:z6Mkunsealed", "version": "1.0.0", "name": "Portal spikes",
            "created_at": "2026-11-13T00:00:00Z", "updated_at": "2026-11-13T00:00:00Z"
        });
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default();
        let members: [(&str, Vec<u8>); 7] = [
            ("manifest.json", serde_json::to_vec(&manifest).unwrap_or_default()),
            ("entries.json", br#"{"entries":[]}"#.to_vec()),
            ("plugin/extension.json", serde_json::to_vec(&extension).unwrap_or_default()),
            (
                "web/index.html",
                b"<!doctype html><title>spike bundle</title><script src=\"/app.js\"></script><script src=\"/app_b.js\"></script>"
                    .to_vec(),
            ),
            ("web/app.js", super::spikes_a::BUNDLE_SCRIPT.as_bytes().to_vec()),
            ("web/app_b.js", super::spikes_b::BUNDLE_SCRIPT.as_bytes().to_vec()),
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
            "/sp8.html" => Some(super::spikes_b::PAGE_SP8_EXTERNAL),
            "/sp9.html" => Some(super::spikes_b::PAGE_SP9),
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
        /// Every `(url, decision)` the navigation callback produced, in order.
        decisions: Rc<RefCell<Vec<(String, String)>>>,
        /// URL prefixes the callback judges, logs, then cancels (SP-7).
        probes: Rc<RefCell<Vec<String>>>,
        ipc_messages: Arc<Mutex<Vec<IpcRecord>>>,
        permissions: Arc<Mutex<Vec<(i32, String)>>>,
        wry_ipc_received: Rc<Cell<u32>>,
        hook: Rc<NavigationHook>,
        deadline: Instant,
        publisher_did: String,
        fixture_a: TempFile,
        fixture_b: TempFile,
    }

    /// Seal `package`'s fixture under `key` into a temp file.
    fn write_fixture(package: &str, key: &SigningKey) -> anyhow::Result<TempFile> {
        let sealed = fe_format::seal(&archive(package), key)?;
        let path = std::env::temp_dir().join(format!("fe-portal-spikes-{}.hexon", ulid::Ulid::new()));
        std::fs::write(&path, &sealed)?;
        Ok(TempFile(path))
    }

    impl Harness {
        pub fn new() -> anyhow::Result<Self> {
            Self::with_options(HarnessOptions::default())
        }

        pub fn with_options(options: HarnessOptions) -> anyhow::Result<Self> {
            let key = SigningKey::from_bytes(&[7u8; 32]);
            let publisher_did = fe_format::verifying_key_to_did(&key.verifying_key());
            let fixture_a = write_fixture(PACKAGE, &key)?;
            let fixture_b = write_fixture(PACKAGE_B, &key)?;
            let mount = mount_fixture(fixture_a.0.clone(), &publisher_did, Admission::Admitted)?;
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
            let decisions: Rc<RefCell<Vec<(String, String)>>> = Rc::default();
            let probes: Rc<RefCell<Vec<String>>> = Rc::default();
            let ipc_messages: Arc<Mutex<Vec<IpcRecord>>> = Arc::default();
            let permissions: Arc<Mutex<Vec<(i32, String)>>> = Arc::default();
            let wry_ipc_received: Rc<Cell<u32>> = Rc::default();
            let hook: Rc<NavigationHook> = Rc::default();

            let protocol_mounts = mounts.clone();
            let protocol_requests = requests.clone();
            let protocol_reports = reports.clone();
            let spike_reports = reports.clone();
            let navigation_gate = gate.clone();
            let navigation_log = navigations.clone();
            let navigation_bypass = bypass.clone();
            let navigation_hook = hook.clone();
            let decision_log = decisions.clone();
            let navigation_probes = probes.clone();

            use wry::WebViewBuilderExtWindows as _;
            let mut builder = WebViewBuilder::new()
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
                        decision_log.borrow_mut().push((url, "spike-scheme".to_owned()));
                        return true;
                    }
                    if navigation_bypass.borrow().iter().any(|allowed| *allowed == url) {
                        decision_log.borrow_mut().push((url, "bypass".to_owned()));
                        return true;
                    }
                    let attempt = Attempt { raw_url: &url, is_redirect: false, is_main_frame: true };
                    let decision = navigation_gate.decide(&attempt);
                    let probe = navigation_probes.borrow().iter().any(|prefix| url.starts_with(prefix.as_str()));
                    let allow = !probe && !matches!(decision, Decision::Refuse(_));
                    let logged = if probe { format!("{decision:?} (probe, cancelled)") } else { format!("{decision:?}") };
                    decision_log.borrow_mut().push((url, logged));
                    allow
                })
                .with_https_scheme(true);
            if options.wry_ipc_handler {
                // SP-9: wry's own handler, whose `Request::builder().uri(source).unwrap()` is the abort under test.
                let received = wry_ipc_received.clone();
                builder = builder.with_ipc_handler(move |_request| received.set(received.get() + 1));
            }
            let webview = builder.build(window)?;

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

            // Native page-to-host logger (P-2's shape): the sender URL is judged against its cap before the body is read.
            let message_log = ipc_messages.clone();
            let message_handler = WebMessageReceivedEventHandler::create(Box::new(move |_, args| {
                let Some(args) = args else { return Ok(()) };
                let mut raw_source = PWSTR::null();
                // SAFETY: on success WebView2 hands over a CoTaskMem string that `take_pwstr` frees.
                let source = match unsafe { args.Source(&mut raw_source) } {
                    Ok(()) => webview2_com::take_pwstr(raw_source),
                    Err(_) => String::new(),
                };
                let source_length = source.encode_utf16().count();
                let dropped = source_length > MAX_SOURCE_BYTES;
                let body = if dropped {
                    None
                } else {
                    let mut raw_body = PWSTR::null();
                    // SAFETY: as above; a non-string message fails here and is kept as `None`.
                    match unsafe { args.TryGetWebMessageAsString(&mut raw_body) } {
                        Ok(()) => Some(webview2_com::take_pwstr(raw_body).chars().take(1024).collect()),
                        Err(_) => None,
                    }
                };
                message_log.lock().unwrap_or_else(PoisonError::into_inner).push(IpcRecord {
                    source: source.chars().take(SOURCE_KEPT_CHARS).collect(),
                    source_length,
                    body,
                    dropped,
                });
                Ok(())
            }));
            let mut message_token = 0i64;
            // SAFETY: registered on the UI thread that owns the webview.
            unsafe { native.add_WebMessageReceived(&message_handler, &mut message_token) }
                .map_err(|error| anyhow::anyhow!("WebMessageReceived registration failed: {error}"))?;

            // Permission requests (SP-8's local-network question): logged, left at the default state.
            let permission_log = permissions.clone();
            let permission_handler = PermissionRequestedEventHandler::create(Box::new(move |_, args| {
                if let Some(args) = args {
                    let mut kind = COREWEBVIEW2_PERMISSION_KIND::default();
                    // SAFETY: plain out-parameter on the UI thread.
                    let _ = unsafe { args.PermissionKind(&mut kind) };
                    let mut raw = PWSTR::null();
                    // SAFETY: as for `Source` above.
                    let uri = match unsafe { args.Uri(&mut raw) } {
                        Ok(()) => webview2_com::take_pwstr(raw),
                        Err(_) => String::new(),
                    };
                    permission_log.lock().unwrap_or_else(PoisonError::into_inner).push((kind.0, uri));
                }
                Ok(())
            }));
            let mut permission_token = 0i64;
            // SAFETY: registered on the UI thread that owns the webview.
            unsafe { native.add_PermissionRequested(&permission_handler, &mut permission_token) }
                .map_err(|error| anyhow::anyhow!("PermissionRequested registration failed: {error}"))?;

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
                decisions,
                probes,
                ipc_messages,
                permissions,
                wry_ipc_received,
                hook,
                deadline: Instant::now(),
                publisher_did,
                fixture_a,
                fixture_b,
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
            self.decisions.borrow_mut().clear();
            self.probes.borrow_mut().clear();
            self.ipc_messages.lock().unwrap_or_else(PoisonError::into_inner).clear();
            self.permissions.lock().unwrap_or_else(PoisonError::into_inner).clear();
            self.wry_ipc_received.set(0);
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

        /// Open bundle A as the host does (entry token, then load) with `?<query>`; remounts A if another mount is live.
        pub fn enter_bundle(&mut self, query: &str) -> anyhow::Result<()> {
            let a_live = self.mounts.current().is_some_and(|mount| mount.label == self.label_a);
            if !a_live {
                let mount = self.prepare_mount(Fixture::A, Admission::Admitted)?;
                self.mounts.install(mount);
            }
            let url = format!("{}?{query}", self.entry_url);
            self.gate.expect_entry(url.clone());
            self.webview.load_url(&url)?;
            Ok(())
        }

        /// A fresh mount of `fixture`, not installed.
        pub fn prepare_mount(&self, fixture: Fixture, admission: Admission) -> anyhow::Result<Mount> {
            let path = match fixture {
                Fixture::A => self.fixture_a.0.clone(),
                Fixture::B => self.fixture_b.0.clone(),
            };
            mount_fixture(path, &self.publisher_did, admission)
        }

        /// Install `mount` as the live mount and open it as the host does, with `?<query>`.
        pub fn open_mount(&mut self, mount: Mount, query: &str) -> anyhow::Result<Label> {
            let label = mount.label.clone();
            let url = format!("{}?{query}", mount.entry_url);
            self.mounts.install(mount);
            self.gate.expect_entry(url.clone());
            self.webview.load_url(&url)?;
            Ok(label)
        }

        /// Host `load_url`, judged by the gate like any navigation.
        pub fn load_url(&mut self, url: &str) -> anyhow::Result<()> {
            self.webview.load_url(url)?;
            Ok(())
        }

        /// Host `load_html` (`NavigateToString`), judged by the gate like any navigation.
        pub fn load_html(&mut self, html: &str) -> anyhow::Result<()> {
            self.webview.load_html(html)?;
            Ok(())
        }

        /// The native webview, for DevTools protocol calls.
        pub fn native(&self) -> &ICoreWebView2 {
            &self.native
        }

        /// Snapshot of every `(url, decision)` the navigation callback logged.
        pub fn decisions(&self) -> Vec<(String, String)> {
            self.decisions.borrow().clone()
        }

        pub fn clear_decisions(&self) {
            self.decisions.borrow_mut().clear();
        }

        /// Judge, log and then cancel every navigation whose URL starts with `prefix` (until the next spike).
        pub fn probe_navigation(&self, prefix: &str) {
            self.probes.borrow_mut().push(prefix.to_owned());
        }

        /// Page-to-host messages the native logger saw.
        pub fn ipc_messages(&self) -> Vec<IpcRecord> {
            self.ipc_messages.lock().unwrap_or_else(PoisonError::into_inner).clone()
        }

        /// `(permission kind, uri)` of every permission request.
        pub fn permission_requests(&self) -> Vec<(i32, String)> {
            self.permissions.lock().unwrap_or_else(PoisonError::into_inner).clone()
        }

        /// Messages wry's `with_ipc_handler` delivered (only with `HarnessOptions::wry_ipc_handler`).
        pub fn wry_ipc_received(&self) -> u32 {
            self.wry_ipc_received.get()
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
