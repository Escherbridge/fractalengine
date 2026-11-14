//! Spikes SP-5..SP-9 (protocol.md §10, §12); see `fe-webview/src/portal_bundle/AGENTS.md` §spikes.

use std::collections::{BTreeMap, HashMap};
use std::io::{Read, Write as _};
use std::net::{TcpListener, TcpStream};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use fe_webview::portal_bundle::feeds::FeedSource;
use fe_webview::portal_bundle::origin;
use fe_webview::portal_bundle::{Attempt, Decision, LabelKind, NavigationGate, PortalMounts, UNLOAD_URL};
use serde_json::json;
use webview2_com::CallDevToolsProtocolMethodCompletedHandler;
use windows::core::HSTRING;

use super::{Admission, Fixture, Harness, HarnessOptions, Outcome, Spike};

/// The spikes this file runs, after `spikes_a::SPIKES` in `TABLE`.
pub const SPIKES: &[Spike] = &[
    Spike {
        id: "SP-5",
        question: "How does a load_html unload present to the navigation guard?",
        run: sp5_unload_url,
    },
    Spike {
        id: "SP-6",
        question: "Can storage be cleared for one origin?",
        run: sp6_origin_storage_clear,
    },
    Spike {
        id: "SP-7",
        question: "Does the non-Windows form fe-portal://<label>.localhost pass origin::parse, the navigation callback and the IPC sender URL?",
        run: sp7_custom_spelling,
    },
    Spike {
        id: "SP-8",
        question: "Can a static mount read the local share route?",
        run: sp8_share_route,
    },
    Spike {
        id: "SP-9",
        question: "Does a 70 000-byte sender URL abort wry's IPC handler while the native handler survives and drops it?",
        run: sp9_long_sender_url,
    },
];

/// `web/app_b.js` of the fixture bundles: `?spike=` picks the behaviour; scoped so it never clashes with `app.js`.
pub const BUNDLE_SCRIPT: &str = r#"
(() => {
const query = new URLSearchParams(location.search);
const spike = query.get('spike');
const enc = encodeURIComponent;
const report = (id, extra) => fetch('/__spike/report?id=' + id + (extra ? '&' + extra : ''));
if (spike === 'sp5') {
  window.addEventListener('pageshow', (event) => { if (event.persisted) { report('sp5restored'); } });
  report('sp5ready');
}
if (spike === 'sp6write') {
  const key = query.get('key');
  localStorage.setItem('sp6', key);
  const open = indexedDB.open('sp6', 1);
  open.onupgradeneeded = () => open.result.createObjectStore('kv');
  open.onerror = () => report('sp6written', 'error=' + enc(String(open.error)));
  open.onsuccess = () => {
    const db = open.result;
    const transaction = db.transaction('kv', 'readwrite');
    transaction.objectStore('kv').put(key, 'value');
    transaction.oncomplete = () => { db.close(); report('sp6written', 'key=' + enc(key)); };
    transaction.onerror = () => { db.close(); report('sp6written', 'error=' + enc(String(transaction.error))); };
  };
}
if (spike === 'sp6read') {
  const local = localStorage.getItem('sp6');
  const finish = (indexed) => report('sp6read', 'local=' + enc(local === null ? '<none>' : local) + '&indexed=' + enc(indexed));
  const open = indexedDB.open('sp6', 1);
  open.onupgradeneeded = () => open.result.createObjectStore('kv');
  open.onerror = () => finish('<error>');
  open.onsuccess = () => {
    const db = open.result;
    const read = db.transaction('kv').objectStore('kv').get('value');
    read.onsuccess = () => { db.close(); finish(read.result === undefined ? '<none>' : String(read.result)); };
    read.onerror = () => { db.close(); finish('<error>'); };
  };
}
if (spike === 'sp7') {
  const bridge = window.chrome && window.chrome.webview;
  if (bridge) { bridge.postMessage('sp7'); }
  report('sp7posted', 'bridge=' + (bridge ? 'yes' : 'no'));
}
if (spike === 'sp8') {
  const blocked = [];
  document.addEventListener('securitypolicyviolation', (event) => blocked.push(event.violatedDirective + ' ' + event.blockedURI));
  const done = (fields) => setTimeout(() => report('sp8', fields + '&csp=' + enc(blocked.join(','))), 300);
  fetch(query.get('feed')).then(
    (response) => response.text().then((text) => done('outcome=read&status=' + response.status + '&body=' + enc(text.slice(0, 200)))),
    (error) => done('outcome=error&name=' + enc(error.name) + '&message=' + enc(error.message)));
}
})();
"#;

/// SP-8's external page (harness scheme): the same feed fetch as the bundle, from a non-bundle origin.
pub const PAGE_SP8_EXTERNAL: &str = r#"<!doctype html><title>spike sp8 external</title><script>
const query = new URLSearchParams(location.search);
const enc = encodeURIComponent;
const blocked = [];
document.addEventListener('securitypolicyviolation', (event) => blocked.push(event.violatedDirective + ' ' + event.blockedURI));
const done = (fields) => setTimeout(() => fetch('/__spike/report?id=sp8&' + fields + '&csp=' + enc(blocked.join(','))), 300);
fetch(query.get('feed')).then(
  (response) => response.text().then((text) => done('outcome=read&status=' + response.status + '&body=' + enc(text.slice(0, 200)))),
  (error) => done('outcome=error&name=' + enc(error.name) + '&message=' + enc(error.message)));
</script>"#;

/// SP-9's page: replace the document URL with a 70 000-byte one, post one message, then report.
pub const PAGE_SP9: &str = r#"<!doctype html><title>spike sp9</title><script>
const bridge = window.chrome && window.chrome.webview;
history.replaceState(null, '', '/' + 'a'.repeat(70000));
if (bridge) { bridge.postMessage('sp9'); }
setTimeout(() => fetch('/__spike/report?id=sp9posted&bridge=' + (bridge ? 'yes' : 'no') + '&length=' + location.href.length), 300);
</script>"#;

/// The harness re-runs itself with this flag for one SP-9 variant (`wry` or `native`).
pub const CHILD_FLAG: &str = "--sp9-child";
/// Prefix of the child's one result line on stdout.
const CHILD_RESULT_PREFIX: &str = "sp9-result ";
/// One SP-9 child gets this long, then it is killed and recorded as inconclusive.
const CHILD_LIMIT: Duration = Duration::from_secs(13);

const LOAD_WAIT: Duration = Duration::from_secs(8);
const NAVIGATION_WAIT: Duration = Duration::from_secs(4);
/// The fixed document SP-5 unloads with through `load_html`.
const UNLOAD_HTML: &str = "<!doctype html><title>unloaded</title>";
/// A share-token-shaped token (`<payload>.<signature>`, base64url); the spike server never verifies it.
const SHARE_TOKEN: &str = "c3Bpa2U.c2lnbmF0dXJl";
const SHARE_ROUTE: &str = "/api/v1/shared/";
const SHARE_BODY: &str = r#"{"spike":"sp8","rows":3}"#;
/// Longest request head the spike server reads.
const MAX_REQUEST_HEAD: usize = 16 * 1024;

fn encode(raw: &str) -> String {
    url::form_urlencoded::byte_serialize(raw.as_bytes()).collect()
}

fn failed(observed: impl Into<String>, notes: impl Into<String>) -> Outcome {
    Outcome { observed: observed.into(), pass: false, notes: notes.into() }
}

/// At most `limit` characters of `text`, marked when cut.
fn clip(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_owned();
    }
    let head: String = text.chars().take(limit).collect();
    format!("{head}... ({} chars)", text.chars().count())
}

/// Wait for the first logged navigation decision whose URL matches; `(url, decision)`.
fn await_decision(harness: &mut Harness, matches: impl Fn(&str) -> bool) -> Option<(String, String)> {
    harness.wait_until(NAVIGATION_WAIT, |harness| harness.decisions().iter().any(|(url, _)| matches(url)));
    harness.decisions().into_iter().find(|(url, _)| matches(url))
}

fn describe(decision: &Option<(String, String)>) -> String {
    match decision {
        Some((url, decision)) => format!("{} -> {decision}", clip(url, 200)),
        None => "callback never fired".into(),
    }
}

/// Enter bundle A with `?spike=sp5`, wait for its ready report, then clear the decision log.
fn enter_ready(harness: &mut Harness) -> Result<(), String> {
    harness.clear_reports();
    harness.enter_bundle("spike=sp5").map_err(|error| format!("bundle A did not load: {error:#}"))?;
    if harness.wait_report("sp5ready", LOAD_WAIT).is_none() {
        return Err("bundle A never reported ready".into());
    }
    harness.clear_decisions();
    Ok(())
}

/// SP-5: the URL a `load_html` unload shows the guard, the token rule for the fixed unload URL, and the refusals after it.
fn sp5_unload_url(harness: &mut Harness) -> Outcome {
    let mut html_attempts: Vec<Option<(String, String)>> = Vec::new();
    for _ in 0..2 {
        if let Err(error) = enter_ready(harness) {
            return failed(error, "load_html variant");
        }
        harness.gate.expect_unload();
        if let Err(error) = harness.load_html(UNLOAD_HTML) {
            return failed("load_html failed", format!("{error:#}"));
        }
        html_attempts.push(await_decision(harness, |_| true));
        harness.pump_for(Duration::from_millis(500));
    }
    let html_urls: Vec<Option<&str>> =
        html_attempts.iter().map(|attempt| attempt.as_ref().map(|(url, _)| url.as_str())).collect();
    let html_stable = html_urls.len() == 2 && html_urls[0].is_some() && html_urls[0] == html_urls[1];
    let html_admitted = html_attempts
        .iter()
        .all(|attempt| attempt.as_ref().is_some_and(|(_, decision)| decision == "AllowHostUnload"));

    // The production unload: load_url(UNLOAD_URL) behind the one-shot token.
    if let Err(error) = enter_ready(harness) {
        return failed(error, "host unload variant");
    }
    harness.gate.expect_unload();
    if let Err(error) = harness.load_url(UNLOAD_URL) {
        return failed("host unload load failed", format!("{error:#}"));
    }
    let host_unload = await_decision(harness, |url| url == UNLOAD_URL);
    harness.pump_for(Duration::from_millis(500));

    // The page navigates to the same URL with no token.
    if let Err(error) = enter_ready(harness) {
        return failed(error, "page-initiated variant");
    }
    if let Err(error) = harness.eval("location.href = 'about:blank'") {
        return failed("could not start the page navigation", format!("{error:#}"));
    }
    let page_unload = await_decision(harness, |url| url == UNLOAD_URL);
    harness.pump_for(Duration::from_millis(500));

    // P1-05 race: the page navigates first while the host's token is minted.
    if let Err(error) = enter_ready(harness) {
        return failed(error, "token race variant");
    }
    harness.gate.expect_unload();
    if let Err(error) = harness.eval("location.href = 'about:blank'") {
        return failed("could not start the racing page navigation", format!("{error:#}"));
    }
    let race_page = await_decision(harness, |url| url == UNLOAD_URL);
    harness.pump_for(Duration::from_millis(500));
    harness.clear_decisions();
    if let Err(error) = harness.load_url(UNLOAD_URL) {
        return failed("host unload after the race failed", format!("{error:#}"));
    }
    let race_host = await_decision(harness, |url| url == UNLOAD_URL);
    harness.pump_for(Duration::from_millis(500));

    // history.back() from the unload document into A.
    if let Err(error) = enter_ready(harness) {
        return failed(error, "history variant");
    }
    harness.gate.expect_unload();
    if let Err(error) = harness.load_url(UNLOAD_URL) {
        return failed("unload before history.back failed", format!("{error:#}"));
    }
    let _ = await_decision(harness, |url| url == UNLOAD_URL);
    harness.pump_for(Duration::from_millis(800));
    harness.clear_reports();
    harness.clear_decisions();
    let entry_requests_before = entry_requests(harness);
    if let Err(error) = harness.eval("history.back()") {
        return failed("could not run history.back()", format!("{error:#}"));
    }
    let back = await_decision(harness, |url| origin::parse(url).is_some());
    harness.pump_for(Duration::from_secs(2));
    let reloaded = harness.report("sp5ready").is_some();
    let restored = harness.report("sp5restored").is_some();
    let entry_refetches = entry_requests(harness).saturating_sub(entry_requests_before);

    let host_ok = host_unload.as_ref().is_some_and(|(_, decision)| decision == "AllowHostUnload");
    let page_refused = page_unload.as_ref().is_some_and(|(_, decision)| decision.starts_with("Refuse"));
    let back_refused = back.as_ref().is_none_or(|(_, decision)| decision.starts_with("Refuse"));
    let pass = host_ok && page_refused && back_refused && !reloaded && !restored;
    Outcome {
        observed: format!(
            "load_html x2: [{}] | [{}] (same URL both times = {html_stable}, token admitted both = {html_admitted}); \
             host load_url({UNLOAD_URL}) with token: {}; page-initiated {UNLOAD_URL} without token: {}; \
             race (token minted, page first): page {} then host {}; history.back(): {} (A reloaded = {reloaded}, \
             restored from back-forward cache = {restored}, entry refetches = {entry_refetches})",
            describe(&html_attempts[0]),
            describe(&html_attempts[1]),
            describe(&host_unload),
            describe(&page_unload),
            describe(&race_page),
            describe(&race_host),
            describe(&back),
        ),
        pass,
        notes: "production unloads with load_url(about:blank) (portal_bundle/AGENTS.md §decisions-taken); the load_html \
                rows are the recorded URI for protocol.md §6 row 9. The race row is P1-05: a page that navigates to \
                about:blank while the host's token is minted consumes it"
            .into(),
    }
}

/// Requests for bundle A's entry document the handler answered so far.
fn entry_requests(harness: &Harness) -> usize {
    let entry = harness.entry_url.clone();
    harness.handler_requests().iter().filter(|record| record.uri.starts_with(&entry)).count()
}

/// `Storage.clearDataForOrigin` through `CallDevToolsProtocolMethod`; `Ok(result json)` once the call completes.
fn clear_origin(harness: &mut Harness, origin: &str) -> Result<String, String> {
    let completion: Arc<Mutex<Option<Result<String, String>>>> = Arc::default();
    let sink = completion.clone();
    let handler = CallDevToolsProtocolMethodCompletedHandler::create(Box::new(move |result, json| {
        let outcome = match result {
            Ok(()) => Ok(json),
            Err(error) => Err(error.to_string()),
        };
        *sink.lock().unwrap_or_else(PoisonError::into_inner) = Some(outcome);
        Ok(())
    }));
    let parameters = json!({ "origin": origin, "storageTypes": "all" }).to_string();
    // SAFETY: called on the UI thread that owns the webview; the HSTRINGs outlive the call.
    let issued = unsafe {
        harness.native().CallDevToolsProtocolMethod(
            &HSTRING::from("Storage.clearDataForOrigin"),
            &HSTRING::from(parameters),
            &handler,
        )
    };
    if let Err(error) = issued {
        return Err(format!("not issued: {error}"));
    }
    harness.wait_until(Duration::from_secs(8), |_| {
        completion.lock().unwrap_or_else(PoisonError::into_inner).is_some()
    });
    let taken = completion.lock().unwrap_or_else(PoisonError::into_inner).take();
    taken.unwrap_or_else(|| Err("no completion within 8 s".into()))
}

fn field(report: &Option<HashMap<String, String>>, name: &str) -> String {
    report
        .as_ref()
        .and_then(|fields| fields.get(name).cloned())
        .unwrap_or_else(|| "<no report>".into())
}

/// SP-6: A and B write localStorage and IndexedDB; after A's unload, clearing A's origin leaves B intact.
fn sp6_origin_storage_clear(harness: &mut Harness) -> Outcome {
    let label_a = harness.label_a.clone();
    let mount_b = match harness.prepare_mount(Fixture::B, Admission::Admitted) {
        Ok(mount) => mount,
        Err(error) => return failed("bundle B did not mount", format!("{error:#}")),
    };
    let label_b = mount_b.label.clone();
    if label_a == label_b {
        return failed("A and B share a label", "the fixtures must differ by package id");
    }

    harness.clear_reports();
    if let Err(error) = harness.enter_bundle("spike=sp6write&key=A") {
        return failed("bundle A did not load", format!("{error:#}"));
    }
    let wrote_a = harness.wait_report("sp6written", LOAD_WAIT);
    harness.clear_reports();
    if let Err(error) = harness.open_mount(mount_b, "spike=sp6write&key=B") {
        return failed("bundle B did not load", format!("{error:#}"));
    }
    let wrote_b = harness.wait_report("sp6written", LOAD_WAIT);

    harness.clear_decisions();
    harness.gate.expect_unload();
    if let Err(error) = harness.load_url(UNLOAD_URL) {
        return failed("unload failed", format!("{error:#}"));
    }
    let unloaded = await_decision(harness, |url| url == UNLOAD_URL);
    harness.pump_for(Duration::from_millis(800));
    let cleared = clear_origin(harness, &label_a.https_origin());

    harness.clear_reports();
    if let Err(error) = harness.enter_bundle("spike=sp6read") {
        return failed("bundle A did not reopen", format!("{error:#}"));
    }
    let read_a = harness.wait_report("sp6read", LOAD_WAIT);
    harness.clear_reports();
    let reopened_b = harness
        .prepare_mount(Fixture::B, Admission::Admitted)
        .and_then(|mount| harness.open_mount(mount, "spike=sp6read"));
    if let Err(error) = reopened_b {
        return failed("bundle B did not reopen", format!("{error:#}"));
    }
    let read_b = harness.wait_report("sp6read", LOAD_WAIT);

    let wrote = field(&wrote_a, "key") == "A" && field(&wrote_b, "key") == "B";
    let a_gone = field(&read_a, "local") == "<none>" && field(&read_a, "indexed") == "<none>";
    let b_kept = field(&read_b, "local") == "B" && field(&read_b, "indexed") == "B";
    Outcome {
        observed: format!(
            "wrote A {wrote_a:?}, B {wrote_b:?}; unload {}; clearDataForOrigin({}) -> {cleared:?}; \
             A after clear: local={} indexed={}; B: local={} indexed={}",
            describe(&unloaded),
            label_a.https_origin(),
            field(&read_a, "local"),
            field(&read_a, "indexed"),
            field(&read_b, "local"),
            field(&read_b, "indexed"),
        ),
        pass: wrote && cleared.is_ok() && a_gone && b_kept,
        notes: format!(
            "storageTypes \"all\" as in backends/tauri.rs::clear_origin_storage; B is {}",
            label_b.https_origin()
        ),
    }
}

/// SP-7: the `fe-portal://` spelling through `origin::parse` and the gate in-process; the live callback on Windows.
fn sp7_custom_spelling(harness: &mut Harness) -> Outcome {
    let label = harness.label_a.clone();
    let custom_entry = format!("fe-portal://{label}.localhost/index.html");
    let https_entry = format!("{}/index.html", label.https_origin());
    let parse_ok = origin::parse(&custom_entry).as_ref() == Some(&label)
        && origin::parse(&https_entry).as_ref() == Some(&label);
    let refused_spellings = [
        format!("fe-portal://{label}.localhost:8080/"),
        format!("FE-PORTAL://{label}.localhost/"),
        format!("fe-portal://user@{label}.localhost/"),
        format!("fe-portal://{label}.localhost./"),
    ];
    let refused_ok = refused_spellings.iter().all(|raw| origin::parse(raw).is_none());

    // The gate over an isolated table, so the shared gate's state is untouched.
    let isolated = match harness.prepare_mount(Fixture::A, Admission::Admitted) {
        Ok(mount) => {
            let mounts = PortalMounts::new();
            mounts.install(mount);
            let gate = NavigationGate::new(mounts);
            gate.expect_entry(custom_entry.clone());
            let enter = gate.decide(&Attempt { raw_url: &custom_entry, is_redirect: false, is_main_frame: true });
            let member = format!("fe-portal://{label}.localhost/app.js");
            let same = gate.decide(&Attempt { raw_url: &member, is_redirect: false, is_main_frame: true });
            Ok((enter, same))
        }
        Err(error) => Err(format!("{error:#}")),
    };
    let isolated_ok = matches!(isolated, Ok((Decision::AllowEnter, Decision::AllowSame)));

    // Windows: the IPC sender URL of a post from A (https spelling expected).
    harness.clear_reports();
    if let Err(error) = harness.enter_bundle("spike=sp7") {
        return failed("bundle A did not load", format!("{error:#}"));
    }
    let posted = harness.wait_report("sp7posted", LOAD_WAIT);
    harness.wait_until(Duration::from_secs(2), |harness| !harness.ipc_messages().is_empty());
    let senders: Vec<String> = harness.ipc_messages().into_iter().map(|message| message.source).collect();
    let sender_ok = senders.iter().any(|source| origin::parse(source).as_ref() == Some(&label));

    // Windows: what the navigation callback sees for the custom spelling; cancelled so nothing launches.
    harness.clear_decisions();
    harness.probe_navigation("fe-portal://");
    let script = format!("location.href = '{custom_entry}'");
    let callback = match harness.eval(&script) {
        Ok(()) => await_decision(harness, |url| url.to_ascii_lowercase().starts_with("fe-portal://")),
        Err(error) => return failed("could not start the custom-spelling navigation", format!("{error:#}")),
    };
    harness.pump_for(Duration::from_millis(500));

    Outcome {
        observed: format!(
            "origin::parse custom+https = {parse_ok}, refused variants refused = {refused_ok}; isolated gate on \
             {custom_entry}: {isolated:?}; Windows sender URLs {senders:?} (page bridge {}); Windows callback for the \
             custom spelling: {}",
            field(&posted, "bridge"),
            describe(&callback),
        ),
        pass: parse_ok && refused_ok && isolated_ok && sender_ok,
        notes: "needs non-Windows host: the navigation callback committing fe-portal:// and the IPC sender URL in that \
                spelling (macOS and Linux, OD-10); on Windows the custom-spelling navigation is logged then cancelled"
            .into(),
    }
}

/// One request the spike share server answered.
#[derive(Debug, Clone)]
struct ShareRequest {
    method: String,
    path: String,
    origin: Option<String>,
    private_network: bool,
    allowed: bool,
}

/// A tiny loopback HTTP/1.1 server answering the share route with the admission.md §8.2 CORS rule.
struct ShareServer {
    port: u16,
    stop: Arc<AtomicBool>,
    log: Arc<Mutex<Vec<ShareRequest>>>,
    thread: Option<JoinHandle<()>>,
}

impl ShareServer {
    fn start() -> std::io::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        listener.set_nonblocking(true)?;
        let port = listener.local_addr()?.port();
        let stop = Arc::new(AtomicBool::new(false));
        let log: Arc<Mutex<Vec<ShareRequest>>> = Arc::default();
        let thread_stop = stop.clone();
        let thread_log = log.clone();
        let thread = std::thread::spawn(move || {
            while !thread_stop.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => answer_share(stream, &thread_log),
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => break,
                }
            }
        });
        Ok(Self { port, stop, log, thread: Some(thread) })
    }

    fn requests(&self) -> Vec<ShareRequest> {
        self.log.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

impl Drop for ShareServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Exactly `https://fe-portal.s-[a-z2-7]{26}.localhost`: static labels only, never `a-` (admission.md §8.2).
fn is_static_origin(raw: &str) -> bool {
    raw.starts_with("https://")
        && origin::split(raw).is_some_and(|(label, tail)| tail.is_empty() && label.kind() == LabelKind::Static)
}

/// Read one request head, answer it and log it; `Connection: close`.
fn answer_share(mut stream: TcpStream, log: &Mutex<Vec<ShareRequest>>) {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let mut head = Vec::new();
    let mut chunk = [0u8; 1024];
    while head.len() < MAX_REQUEST_HEAD && !head.windows(4).any(|window| window == b"\r\n\r\n") {
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(read) => head.extend_from_slice(&chunk[..read]),
        }
    }
    let text = String::from_utf8_lossy(&head);
    let mut lines = text.split("\r\n");
    let mut request_line = lines.next().unwrap_or_default().split(' ');
    let method = request_line.next().unwrap_or_default().to_owned();
    let path = request_line.next().unwrap_or_default().to_owned();
    let mut request_origin = None;
    let mut private_network = false;
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            match name.trim().to_ascii_lowercase().as_str() {
                "origin" => request_origin = Some(value.trim().to_owned()),
                "access-control-request-private-network" => private_network = value.trim().eq_ignore_ascii_case("true"),
                _ => {}
            }
        }
    }
    let on_route = path.starts_with(SHARE_ROUTE);
    let allowed_origin = request_origin.as_deref().filter(|raw| on_route && is_static_origin(raw));
    let (status, body) = match (method.as_str(), on_route) {
        ("GET", true) => ("200 OK", SHARE_BODY),
        ("OPTIONS", true) => ("204 No Content", ""),
        (_, true) => ("405 Method Not Allowed", ""),
        _ => ("404 Not Found", ""),
    };
    let mut response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nCache-Control: no-store\r\n\
         Vary: Origin\r\nConnection: close\r\n",
        body.len()
    );
    if let Some(allowed) = allowed_origin {
        response.push_str(&format!("Access-Control-Allow-Origin: {allowed}\r\n"));
        if method == "OPTIONS" {
            response.push_str("Access-Control-Allow-Methods: GET, OPTIONS\r\n");
            if private_network {
                response.push_str("Access-Control-Allow-Private-Network: true\r\n");
            }
        }
    }
    response.push_str("\r\n");
    response.push_str(body);
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
    let allowed = allowed_origin.is_some();
    log.lock().unwrap_or_else(PoisonError::into_inner).push(ShareRequest {
        method,
        path,
        origin: request_origin,
        private_network,
        allowed,
    });
}

fn summarize_fetch(report: &Option<HashMap<String, String>>) -> String {
    match report {
        None => "no report".into(),
        Some(fields) => {
            let get = |name: &str| fields.get(name).cloned().unwrap_or_default();
            if get("outcome") == "read" {
                format!("read status {} body {:?} csp [{}]", get("status"), get("body"), get("csp"))
            } else {
                format!("{} {:?} csp [{}]", get("name"), get("message"), get("csp"))
            }
        }
    }
}

fn read_body(report: &Option<HashMap<String, String>>) -> Option<(String, String)> {
    let fields = report.as_ref()?;
    (fields.get("outcome")? == "read").then(|| {
        (fields.get("status").cloned().unwrap_or_default(), fields.get("body").cloned().unwrap_or_default())
    })
}

/// SP-8: a static mount reads the share route under §8's CORS rule; the admitted mount and an external page cannot.
fn sp8_share_route(harness: &mut Harness) -> Outcome {
    let server = match ShareServer::start() {
        Ok(server) => server,
        Err(error) => return failed("could not bind a loopback port", format!("{error}")),
    };
    let api_origin = format!("http://127.0.0.1:{}", server.port);
    let feed = format!("{api_origin}{SHARE_ROUTE}{SHARE_TOKEN}");
    let feeds = FeedSource {
        feeds: BTreeMap::from([("flow".to_owned(), feed.clone())]),
        local_api_origin: Some(api_origin.clone()),
    };
    let mount = match harness.prepare_mount(Fixture::A, Admission::Static(feeds)) {
        Ok(mount) => mount,
        Err(error) => return failed("the static mount did not prepare", format!("{error:#}")),
    };
    if mount.label.kind() != LabelKind::Static {
        return failed("the feed mount is not static", format!("label {}", mount.label));
    }
    let query = format!("spike=sp8&feed={}", encode(&feed));

    harness.clear_reports();
    let static_label = match harness.open_mount(mount, &query) {
        Ok(label) => label,
        Err(error) => return failed("the static mount did not load", format!("{error:#}")),
    };
    let static_fetch = harness.wait_report("sp8", LOAD_WAIT);

    harness.clear_reports();
    if let Err(error) = harness.enter_bundle(&query) {
        return failed("bundle A did not load", format!("{error:#}"));
    }
    let admitted_fetch = harness.wait_report("sp8", LOAD_WAIT);

    harness.clear_reports();
    if let Err(error) = harness.load_spike_page(&format!("/sp8.html?{query}")) {
        return failed("the external page did not load", format!("{error:#}"));
    }
    let external_fetch = harness.wait_report("sp8", LOAD_WAIT);
    harness.pump_for(Duration::from_millis(300));

    let requests = server.requests();
    let preflights: Vec<String> = requests
        .iter()
        .filter(|request| request.method == "OPTIONS")
        .map(|request| format!("{:?} private-network={}", request.origin, request.private_network))
        .collect();
    let seen: Vec<String> = requests
        .iter()
        .map(|request| {
            format!("{} {} origin={:?} allowed={}", request.method, clip(&request.path, 40), request.origin, request.allowed)
        })
        .collect();
    let permissions = harness.permission_requests();

    let static_read = read_body(&static_fetch).is_some_and(|(status, body)| status == "200" && body == SHARE_BODY);
    let admitted_blocked = admitted_fetch.is_some() && read_body(&admitted_fetch).is_none();
    let external_blocked = external_fetch.is_some() && read_body(&external_fetch).is_none();
    Outcome {
        observed: format!(
            "static {}: {}; admitted {}: {}; external https://fe-spike.localhost: {}; server saw {seen:?}; \
             preflights {preflights:?}; permission requests (kind, uri) {permissions:?}",
            static_label,
            summarize_fetch(&static_fetch),
            harness.label_a,
            summarize_fetch(&admitted_fetch),
            summarize_fetch(&external_fetch),
        ),
        pass: static_read && admitted_blocked && external_blocked,
        notes: "the external stand-in is the harness scheme under .localhost (loopback address space), so it cannot show \
                a public-to-local private-network preflight; repeat that half from a public https page by hand"
            .into(),
    }
}

/// How one SP-9 child ended.
struct ChildRun {
    exit: String,
    success: bool,
    killed: bool,
    result: Option<serde_json::Value>,
    stderr_tail: String,
}

fn drain<R: Read + Send + 'static>(pipe: Option<R>) -> JoinHandle<String> {
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        if let Some(mut pipe) = pipe {
            let _ = pipe.read_to_end(&mut bytes);
        }
        String::from_utf8_lossy(&bytes).into_owned()
    })
}

fn describe_status(status: &ExitStatus) -> String {
    match status.code() {
        // Two's-complement view, so NTSTATUS codes such as 0xc0000409 read as written.
        Some(code) => format!("exit {code} ({:#010x})", code as u32),
        None => "no exit code".into(),
    }
}

/// Run this binary as an SP-9 child, pumping the parent's window while it runs.
fn run_child(harness: &mut Harness, mode: &str) -> Result<ChildRun, String> {
    let program = std::env::current_exe().map_err(|error| format!("current_exe: {error}"))?;
    let mut child = Command::new(program)
        .args([CHILD_FLAG, mode])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("spawn: {error}"))?;
    let stdout = drain(child.stdout.take());
    let stderr = drain(child.stderr.take());
    let deadline = Instant::now() + CHILD_LIMIT;
    let mut killed = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() < deadline => harness.pump_once(),
            _ => {
                killed = true;
                let _ = child.kill();
                break child.wait().ok();
            }
        }
    };
    let stdout = stdout.join().unwrap_or_default();
    let stderr = stderr.join().unwrap_or_default();
    let result = stdout
        .lines()
        .find_map(|line| line.strip_prefix(CHILD_RESULT_PREFIX))
        .and_then(|line| serde_json::from_str(line).ok());
    let tail_start = stderr.chars().count().saturating_sub(400);
    Ok(ChildRun {
        exit: status.as_ref().map_or_else(|| "unknown".into(), describe_status),
        success: status.is_some_and(|status| status.success()),
        killed,
        result,
        stderr_tail: stderr.chars().skip(tail_start).collect(),
    })
}

/// SP-9: wry's `with_ipc_handler` aborts its process on a 70 000-byte sender URL; the native handler drops the post.
fn sp9_long_sender_url(harness: &mut Harness) -> Outcome {
    let wry = match run_child(harness, "wry") {
        Ok(run) => run,
        Err(error) => return failed("the wry child did not start", error),
    };
    let native = match run_child(harness, "native") {
        Ok(run) => run,
        Err(error) => return failed("the native child did not start", error),
    };
    let wry_aborted = !wry.success && !wry.killed && wry.result.is_none();
    let native_dropped = native.result.as_ref().is_some_and(|result| {
        result["native"].as_array().is_some_and(|messages| {
            messages.iter().any(|message| {
                message["dropped"].as_bool() == Some(true)
                    && message["source_length"].as_u64().is_some_and(|length| length >= 70_000)
            })
        })
    });
    let native_survived = native.success && native.result.is_some();
    Outcome {
        observed: format!(
            "with_ipc_handler child: {} (killed after {} s = {}, result {:?}, stderr tail {:?}); native child: {} \
             (killed = {}, result {:?})",
            wry.exit,
            CHILD_LIMIT.as_secs(),
            wry.killed,
            wry.result,
            wry.stderr_tail,
            native.exit,
            native.killed,
            native.result,
        ),
        pass: wry_aborted && native_survived && native_dropped,
        notes: "each variant runs in a child process of this runner, so the expected abort never ends the run; \
                0xc0000409 is the fail-fast code a panic inside a COM callback produces"
            .into(),
    }
}

/// Child entry for one SP-9 variant: `wry` registers wry's `with_ipc_handler`, `native` only the native handler.
pub fn sp9_child(mode: &str) -> i32 {
    let options = HarnessOptions { wry_ipc_handler: mode == "wry" };
    let mut harness = match Harness::with_options(options) {
        Ok(harness) => harness,
        Err(error) => {
            eprintln!("sp9 child harness failed to start: {error:#}");
            return 3;
        }
    };
    harness.begin_spike(Duration::from_secs(10));
    if let Err(error) = harness.load_spike_page("/sp9.html") {
        eprintln!("sp9 page load failed: {error:#}");
        return 4;
    }
    let posted = harness.wait_report("sp9posted", Duration::from_secs(8));
    harness.pump_for(Duration::from_secs(1));
    let messages: Vec<serde_json::Value> = harness
        .ipc_messages()
        .into_iter()
        .map(|message| {
            json!({ "source_length": message.source_length, "dropped": message.dropped, "body": message.body })
        })
        .collect();
    let result = json!({
        "mode": mode,
        "page_posted": posted.is_some(),
        "page_url_length": posted.as_ref().and_then(|report| report.get("length").cloned()),
        "page_bridge": posted.as_ref().and_then(|report| report.get("bridge").cloned()),
        "wry_received": harness.wry_ipc_received(),
        "native": messages,
    });
    println!("{CHILD_RESULT_PREFIX}{result}");
    let _ = std::io::stdout().flush();
    0
}
