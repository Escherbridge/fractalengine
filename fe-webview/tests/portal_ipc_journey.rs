//! Portal IPC journey (spec R8-R11, R14) over the real mount table, gate, host, page-API and feed serving and bridge;
//! only the broker and governance edges are faked.

use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use ed25519_dalek::SigningKey;
use fe_policy::RoleLevel;
use fe_webview::ipc::{BlockedReason, BrowserEvent, BundleState, RevokedReason};
use fe_webview::portal_bundle::admission::{
    AdmissionEvent, AdmissionRecord, AdmissionSignal, GovernanceView, RecordReason, RecordState,
};
use fe_webview::portal_bundle::feeds::{serve, FeedSource, FEEDS_PATH};
use fe_webview::portal_bundle::ipc::PAGE_API_PATH;
use fe_webview::portal_bundle::origin::{admitted_label, static_label};
use fe_webview::portal_bundle::qa_rows::{self, IpcPostRow};
use fe_webview::portal_bundle::{
    prepare, AdmissionVerdict, Attempt, BindRequest, BrokerClient, BrokerEvent, BundleHost, BundleIdentity, BundleParts,
    BundleResolver, BundleSource, BundleTarget, BundleWebview, Decision, DeliveryKind, Ingress, IpcError, Label,
    MountMode, NavigationGate, PendingReply, PortalBridge, PortalBroker, PortalIntent, PortalMounts, UiIntent,
    UNLOAD_URL,
};
use http::StatusCode;
use serde_json::{json, Value};

const PACKAGE: &str = "org.example.flow-dashboard";
const PETAL: &str = "01JPETAL000000000000000000";
const NODE_A: &str = "01JNODEA000000000000000000";
const BROKER_CLIENT: u64 = 7;
const GRANTED: &[&str] = &[
    "subscribe:node",
    "subscribe:readings",
    "subscribe:query/hourly_flow",
    "subscribe:org.example.fleet-sim/positions",
    "act:set_property",
    "invoke:org.example.fleet-sim/start",
    "emit:selection",
    "intent:focus_node",
];
/// A share token whose payload is `{"v":1,"exp":1794240000}` (2026-11-09T16:00:00Z); the signature is never checked here.
const SHARE_TOKEN: &str = "eyJ2IjoxLCJleHAiOjE3OTQyNDAwMDB9.c2lnbmF0dXJl";

// ---------------------------------------------------------------------------
// A sealed bundle declaring every capability class (README §5)
// ---------------------------------------------------------------------------

struct Fixture {
    path: PathBuf,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn sealed_bundle(key: &SigningKey) -> Fixture {
    sealed_bundle_with(key, "1.0.0", &["fleet.*"])
}

/// The same package at `version`, declaring `set_property` over `keys`.
fn sealed_bundle_with(key: &SigningKey, version: &str, keys: &[&str]) -> Fixture {
    let portal = json!({
        "schema": "fe-portal-bundle/1", "entry": "web/index.html", "title": "Flow dashboard",
        "csp": { "connect": [], "img": [], "font": [], "media": [], "frame": [], "wasm": false, "inline_styles": false },
        "navigation": [],
        "subscriptions": {
            "node": true, "readings": true,
            "queries": [{ "name": "hourly_flow", "sql": "SELECT metric FROM iot_reading WHERE metric = $metric",
                          "params": { "metric": "string" } }]
        },
        "commands": { "set_property": { "keys": keys },
                      "invoke": [{ "package": "org.example.fleet-sim", "command": "start" }] },
        "hooks": [{ "source": "org.example.fleet-sim/positions" }],
        "streams": [{ "id": "selection", "max_rate_hz": 4, "max_event_bytes": 2048 }],
        "ui_intents": ["focus_node"]
    });
    let extension = json!({ "manifest_version": 2, "id": PACKAGE, "version": version, "kind": "web-bundle", "portal": portal });
    let manifest = json!({
        "schema_version": "1.0.0", "hexon_id": PACKAGE, "hexon_type": "plugin",
        "publisher_did": "did:key:z6Mkunsealed", "version": version, "name": "Flow dashboard",
        "created_at": "2026-11-08T00:00:00Z", "updated_at": "2026-11-08T00:00:00Z"
    });
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default();
    let members: [(&str, Vec<u8>); 4] = [
        ("manifest.json", serde_json::to_vec(&manifest).unwrap()),
        ("entries.json", br#"{"entries":[]}"#.to_vec()),
        ("plugin/extension.json", serde_json::to_vec(&extension).unwrap()),
        ("web/index.html", b"<!doctype html><title>Flow</title><script src=\"/__fe/portal-v1.js\"></script>".to_vec()),
    ];
    for (name, bytes) in members {
        zip.start_file(name, options).unwrap();
        zip.write_all(&bytes).unwrap();
    }
    let sealed = fe_format::seal(&zip.finish().unwrap().into_inner(), key).unwrap();
    let path = std::env::temp_dir().join(format!("fe-portal-ipc-{}.hexon", ulid::Ulid::new()));
    std::fs::write(&path, sealed).unwrap();
    Fixture { path }
}

struct Resolver {
    path: PathBuf,
    admit: Option<String>,
    feeds: FeedSource,
}

impl BundleResolver for Resolver {
    fn resolve(&self, _node_id: &str) -> Result<BundleSource, BundleState> {
        Ok(BundleSource { asset_id: "asset-1".into(), blob_path: Some(self.path.clone()), revoked: None })
    }
    fn verdict(&self, _identity: &BundleIdentity) -> AdmissionVerdict {
        match &self.admit {
            Some(publisher_did) => AdmissionVerdict::Admitted { publisher_did: publisher_did.clone() },
            None => AdmissionVerdict::Unadmitted { can_admit: true },
        }
    }
    fn feeds(&self, _node_id: &str) -> FeedSource {
        self.feeds.clone()
    }
}

// ---------------------------------------------------------------------------
// The faked governance edge (A-2, A-15) and a resolver that consults it
// ---------------------------------------------------------------------------

#[derive(Default)]
struct FakeGovernance {
    records: Mutex<BTreeMap<(String, String), AdmissionRecord>>,
    killed: Mutex<BTreeSet<String>>,
    /// Audited verse-Owner overrides as `(target anchor, content_hash)`.
    overrides: Mutex<BTreeSet<(String, String)>>,
}

impl FakeGovernance {
    /// Record consent for `(package_id, content_hash)` covering `capability_digest`.
    fn admit(&self, package_id: &str, content_hash: &str, publisher_did: &str, capability_digest: &str) {
        let record = AdmissionRecord {
            state: RecordState::Admitted,
            reason: RecordReason::Consented,
            publisher_did: publisher_did.into(),
            capability_digest: capability_digest.into(),
        };
        self.records.lock().unwrap().insert((package_id.into(), content_hash.into()), record);
    }
    /// Accept a list entry naming `digest`.
    fn kill(&self, digest: &str) {
        self.killed.lock().unwrap().insert(digest.into());
    }
    /// The verse Owner's audited override for `content_hash` at `target`'s verse on this peer.
    fn override_for(&self, target: &BundleTarget, content_hash: &str) {
        self.overrides.lock().unwrap().insert((target.anchor(), content_hash.into()));
    }
}

impl GovernanceView for FakeGovernance {
    fn host_disabled(&self) -> bool {
        false
    }
    fn kill_listed(&self, digests: &[&str]) -> bool {
        let killed = self.killed.lock().unwrap();
        digests.iter().any(|digest| killed.contains(*digest))
    }
    fn kill_list_override(&self, target: &BundleTarget, content_hash: &str) -> bool {
        self.overrides.lock().unwrap().contains(&(target.anchor(), content_hash.to_owned()))
    }
    fn record(&self, package_id: &str, content_hash: &str) -> Option<AdmissionRecord> {
        self.records.lock().unwrap().get(&(package_id.to_owned(), content_hash.to_owned())).cloned()
    }
    fn can_admit(&self, _target: &BundleTarget, _identity: &BundleIdentity) -> bool {
        true
    }
}

/// A repository node whose `portal.bundle_asset` an Editor can repoint, judged by governance.
struct GovernedResolver {
    path: Mutex<PathBuf>,
    governance: Arc<FakeGovernance>,
}

impl GovernedResolver {
    fn new(path: &Path, governance: &Arc<FakeGovernance>) -> Arc<Self> {
        Arc::new(Self { path: Mutex::new(path.to_path_buf()), governance: Arc::clone(governance) })
    }
    fn point_at(&self, path: &Path) {
        *self.path.lock().unwrap() = path.to_path_buf();
    }
}

impl BundleResolver for GovernedResolver {
    fn resolve(&self, _node_id: &str) -> Result<BundleSource, BundleState> {
        Ok(BundleSource { asset_id: "asset-gov".into(), blob_path: Some(self.path.lock().unwrap().clone()), revoked: None })
    }
    /// The installed build the binding at the petal's nearest scope selects (the same bytes, for the journey).
    fn resolve_bound(&self, _petal_id: &str, _package_id: &str) -> Result<BundleSource, BundleState> {
        self.resolve("bound")
    }
    fn governance(&self) -> Option<&dyn GovernanceView> {
        Some(self.governance.as_ref())
    }
}

/// Records what the host asked of the webview.
#[derive(Default)]
struct FakeWebview {
    loads: Vec<String>,
    shown: bool,
    cleared: Vec<String>,
}

impl BundleWebview for FakeWebview {
    fn bundle_load(&mut self, url: &str) -> anyhow::Result<()> {
        self.loads.push(url.to_owned());
        Ok(())
    }
    fn bundle_show(&mut self) -> anyhow::Result<()> {
        self.shown = true;
        Ok(())
    }
    fn bundle_hide(&mut self) -> anyhow::Result<()> {
        self.shown = false;
        Ok(())
    }
    fn bundle_clear_storage(&mut self, label: &Label) {
        self.cleared.push(label.to_string());
    }
}

fn states(events: &[BrowserEvent]) -> Vec<BundleState> {
    events
        .iter()
        .filter_map(|event| match event {
            BrowserEvent::BundleState { state, .. } => Some(state.clone()),
            _ => None,
        })
        .collect()
}

/// Poll until the open worker's outcome lands (a promotion at install starts another worker, so keep polling).
fn settle(host: &mut BundleHost, webview: &mut FakeWebview) -> Vec<BrowserEvent> {
    for _ in 0..1_000 {
        let events = host.poll(webview);
        if !events.is_empty() {
            return events;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("no bundle outcome within 5 s");
}

// ---------------------------------------------------------------------------
// The faked edge: a broker that records what the bridge asked of it
// ---------------------------------------------------------------------------

#[derive(Default)]
struct FakeState {
    log: Vec<String>,
    audits: Vec<String>,
    events: Vec<BrokerEvent>,
    next_sub: u64,
    write_outcome: Option<Result<Value, IpcError>>,
    /// A faulty broker that opens at the scope role whatever the viewer's token allows.
    ignores_viewer: bool,
    /// The kill-list keys and capability digest of the last bind: the broker's re-check reads these.
    bound_keys: Vec<String>,
    /// The next invoke is refused here, as the bus does (and audits) for a target it denies.
    refuse_invoke: Option<IpcError>,
}

struct FakeBroker {
    granted: BTreeSet<String>,
    state: Mutex<FakeState>,
}

impl FakeBroker {
    /// A broker granting every capability in [`GRANTED`]; the installing identity is Owner at the scope.
    fn new() -> Arc<Self> {
        Arc::new(FakeBroker {
            granted: GRANTED.iter().map(|capability| (*capability).to_owned()).collect(),
            state: Mutex::new(FakeState::default()),
        })
    }
    fn ignore_viewer(&self, ignores: bool) {
        self.state.lock().unwrap().ignores_viewer = ignores;
    }
    fn log(&self) -> Vec<String> {
        self.state.lock().unwrap().log.clone()
    }
    fn audits(&self) -> Vec<String> {
        self.state.lock().unwrap().audits.clone()
    }
    fn push(&self, subscription: u64, kind: DeliveryKind, data: Value) {
        self.state.lock().unwrap().events.push(BrokerEvent { subscription, kind, dropped_before: 0, data });
    }
    fn set_write_outcome(&self, outcome: Result<Value, IpcError>) {
        self.state.lock().unwrap().write_outcome = Some(outcome);
    }
    fn refuse_next_invoke(&self, refusal: IpcError) {
        self.state.lock().unwrap().refuse_invoke = Some(refusal);
    }
    fn bound_keys(&self) -> Vec<String> {
        self.state.lock().unwrap().bound_keys.clone()
    }
    fn record(&self, line: String) {
        self.state.lock().unwrap().log.push(line);
    }
}

fn answered(outcome: Result<Value, IpcError>) -> PendingReply {
    let (sender, receiver) = mpsc::channel();
    sender.send(outcome).unwrap();
    receiver
}

/// The native handler's body read, counted: proves which senders ever get their body read.
fn counted_read(reads: &Cell<u32>, body: Option<String>) -> impl FnOnce() -> Option<String> + '_ {
    move || {
        reads.set(reads.get() + 1);
        body
    }
}

impl PortalBroker for FakeBroker {
    fn open(&self, request: &BindRequest<'_>) -> Result<BrokerClient, IpcError> {
        self.record(format!("open {} {} {}", request.package_id, request.target.anchor(), request.viewer_ceiling));
        let mut keys = vec![request.content_hash.to_owned(), request.archive_digest.to_owned()];
        keys.extend(request.member_digests.iter().cloned());
        keys.push(request.capability_digest.to_owned());
        self.state.lock().unwrap().bound_keys = keys;
        let scope_role = RoleLevel::Owner;
        let role_ceiling = if self.state.lock().unwrap().ignores_viewer {
            scope_role
        } else {
            scope_role.min(request.viewer_ceiling)
        };
        Ok(BrokerClient {
            client: BROKER_CLIENT,
            installing_did: "did:key:z6Mklocal".into(),
            petal_id: PETAL.into(),
            granted: self.granted.clone(),
            role_ceiling,
        })
    }
    fn close(&self, client: u64) {
        self.record(format!("close {client}"));
    }
    fn set_paused(&self, _client: u64, paused: bool) {
        self.record(format!("paused {paused}"));
    }
    fn subscribe(&self, _client: u64, address: &str, params: &Value) -> Result<u64, IpcError> {
        let mut state = self.state.lock().unwrap();
        state.next_sub += 1;
        state.log.push(format!("subscribe {address} {params}"));
        Ok(state.next_sub)
    }
    fn unsubscribe(&self, _client: u64, subscription: u64) {
        self.record(format!("unsubscribe {subscription}"));
    }
    fn take_events(&self, _client: u64, max: usize) -> Vec<BrokerEvent> {
        let mut state = self.state.lock().unwrap();
        let take = state.events.len().min(max);
        state.events.drain(..take).collect()
    }
    fn invoke(&self, _client: u64, package_id: &str, command_id: &str, _args: &Value) -> Result<PendingReply, IpcError> {
        self.record(format!("invoke {package_id}/{command_id}"));
        if let Some(refusal) = self.state.lock().unwrap().refuse_invoke.take() {
            return Err(refusal);
        }
        Ok(answered(Ok(json!({ "started": 1 }))))
    }
    fn emit(&self, _client: u64, stream_id: &str, _data: &Value) -> Result<(), IpcError> {
        self.record(format!("emit {stream_id}"));
        Ok(())
    }
    fn set_property(&self, _client: u64, node_id: &str, key: &str, _value: &Value) -> Result<PendingReply, IpcError> {
        self.record(format!("set_property {node_id} {key}"));
        let outcome = self.state.lock().unwrap().write_outcome.clone().unwrap_or(Ok(Value::Null));
        Ok(answered(outcome))
    }
    fn audit_refusal(&self, _client: u64, operation: &str, target: &str, code: &str) {
        self.state.lock().unwrap().audits.push(format!("{operation} {target} {code}"));
    }
}

// ---------------------------------------------------------------------------
// One admitted, visible, entered document, driven frame by frame
// ---------------------------------------------------------------------------

fn main_frame(raw_url: &str) -> Attempt<'_> {
    Attempt { raw_url, is_redirect: false, is_main_frame: true }
}

fn envelope(token: &str, id: u32, kind: &str, fields: Value) -> String {
    let mut body = fields;
    body["t"] = json!(kind);
    let text = body.to_string();
    format!(r#"{{"v":1,"n":"{token}","id":{id},{}"#, &text[1..])
}

struct Page {
    bundle: Fixture,
    publisher: String,
    mounts: PortalMounts,
    gate: Rc<NavigationGate>,
    bridge: PortalBridge,
    broker: Arc<FakeBroker>,
    dyn_broker: Arc<dyn PortalBroker>,
    sender: String,
    label: String,
    token: String,
    next_id: u32,
    now: Instant,
    received: Vec<Value>,
    intents: Vec<PortalIntent>,
}

impl Page {
    fn open() -> Self {
        let key = SigningKey::from_bytes(&[9u8; 32]);
        let publisher = fe_format::verifying_key_to_did(&key.verifying_key());
        let bundle = sealed_bundle(&key);
        let resolver = Resolver { path: bundle.path.clone(), admit: Some(publisher.clone()), feeds: FeedSource::default() };
        let mount = prepare(&resolver, "node-repo", u64::MAX).expect("admitted mount");
        assert_eq!(mount.mode, MountMode::Admitted);
        let mounts = PortalMounts::new();
        let mount = mounts.install(mount);
        let gate = Rc::new(NavigationGate::new(mounts.clone()));
        gate.expect_entry(mount.entry_url.clone());
        assert_eq!(gate.decide(&main_frame(&mount.entry_url)), Decision::AllowEnter);
        let broker = FakeBroker::new();
        let dyn_broker: Arc<dyn PortalBroker> = broker.clone();
        let bridge = PortalBridge::new();
        bridge.set_visible(true);
        let mut page = Page {
            bundle,
            publisher,
            mounts,
            gate,
            bridge,
            broker,
            dyn_broker,
            sender: mount.entry_url.clone(),
            label: mount.label.to_string(),
            token: String::new(),
            next_id: 1,
            now: Instant::now(),
            received: Vec::new(),
            intents: Vec::new(),
        };
        page.pump();
        page.fetch_token();
        page
    }

    /// GET `/__fe/portal-v1.js` the way the document does, and read the token literal it carries.
    fn fetch_token(&mut self) {
        let response = self.get_page_api(&self.label.clone());
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["content-type"], "text/javascript; charset=utf-8");
        assert!(response.headers().contains_key("content-security-policy"));
        let script = String::from_utf8(response.body().clone()).unwrap();
        let marker = "const DOCUMENT_TOKEN = \"";
        let start = script.find(marker).expect("token literal") + marker.len();
        self.token = script[start..start + 32].to_owned();
        assert!(self.token.bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c)), "{}", self.token);
    }

    fn get_page_api(&self, label: &str) -> http::Response<Vec<u8>> {
        self.get(label, PAGE_API_PATH)
    }

    /// GET `path` on `label` through the handler the backend registers (members, page API, feeds).
    fn get(&self, label: &str, path: &str) -> http::Response<Vec<u8>> {
        let request = http::Request::builder()
            .uri(format!("fe-portal://{label}.localhost{path}"))
            .body(Vec::new())
            .unwrap();
        serve(&self.mounts, &self.bridge, &request)
    }

    fn post_from(&self, source: &str, body: &str) -> Ingress {
        self.bridge.ingress(&self.mounts, source, body, self.now)
    }

    /// Post a well-formed envelope from the document; returns its id.
    fn send(&mut self, kind: &str, fields: Value) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        let body = envelope(&self.token, id, kind, fields);
        assert_eq!(self.post_from(&self.sender.clone(), &body), Ingress::Accepted, "{body}");
        id
    }

    fn pump(&mut self) {
        let intents = self.bridge.pump(&self.mounts, Some(&self.dyn_broker), self.now);
        self.intents.extend(intents);
        for text in self.bridge.take_outbox(&self.mounts) {
            let message: Value = serde_json::from_str(&text).unwrap();
            assert_eq!((message["v"].clone(), message["n"].clone()), (json!(1), json!(self.token)), "{text}");
            self.received.push(message);
        }
    }

    fn advance(&mut self, millis: u64) {
        self.now += Duration::from_millis(millis);
    }

    fn reload(&mut self) {
        assert_eq!(self.gate.decide(&main_frame(&self.sender.clone())), Decision::AllowSame);
    }

    fn reply(&self, id: u32) -> Option<&Value> {
        self.received.iter().find(|message| message["re"] == json!(id))
    }

    fn ok(&self, id: u32) -> Value {
        let reply = self.reply(id).unwrap_or_else(|| panic!("no reply to {id}: {:?}", self.received));
        assert_eq!(reply["ok"], json!(true), "{reply}");
        reply["data"].clone()
    }

    fn error(&self, id: u32) -> String {
        let reply = self.reply(id).unwrap_or_else(|| panic!("no reply to {id}"));
        assert_eq!(reply["ok"], json!(false), "{reply}");
        reply["error"]["code"].as_str().unwrap().to_owned()
    }

    fn deliveries(&self, sub: u64) -> Vec<&Value> {
        self.received.iter().filter(|message| message["sub"] == json!(sub)).collect()
    }

    fn subscribe(&mut self, stream: &str, params: Value) -> u32 {
        self.send("subscribe", json!({ "stream": stream, "params": params }))
    }
}

// ---------------------------------------------------------------------------
// Journeys
// ---------------------------------------------------------------------------

/// R8: only an Admitted, visible, current document is heard; everything else is dropped with no reply.
#[test]
fn sender_gate_drops_every_unauthorized_post_without_a_reply() {
    let mut page = Page::open();
    assert_eq!(page.broker.log(), vec![format!("open {PACKAGE} node-repo owner")], "the mount binds once");
    // The bind hands the broker every kill-list key of the build and its capability digest, so the bind-time re-check
    // matches by hash under any id (admission.md §4.2) and admits only the consented capability set.
    let mounted = page.mounts.current().expect("mounted");
    let mut keys = vec![mounted.content_hash.clone(), mounted.archive_digest.clone()];
    keys.extend(mounted.member_digests.iter().cloned());
    keys.push(mounted.capability_digest.clone());
    assert!(!mounted.member_digests.is_empty());
    assert_eq!(page.broker.bound_keys(), keys);

    // Control: the admitted, visible, current document gets a reply.
    let hello = page.send("hello", json!({}));
    page.pump();
    let data = page.ok(hello);
    assert_eq!(data["api"], "fe.portal.v1");
    assert_eq!(data["context"]["petal_id"], PETAL);
    assert_eq!(data["context"]["package_id"], PACKAGE);
    assert_eq!(data["context"]["publisher_did"], json!(page.publisher));
    let capabilities: Vec<&str> = data["capabilities"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
    assert!(capabilities.contains(&"act:set_property") && capabilities.contains(&"intent:focus_node"));

    // Checks 1-2: well-formed posts from the wrong sender or document. Mutation check: without check 1 these go red.
    let subscribe = envelope(&page.token, 99, "subscribe", json!({ "stream": "node", "params": {} }));
    let foreign = admitted_label("did:key:other", PACKAGE).document_url("web/index.html").unwrap();
    let reordered = format!(r#"{{"n":"{}","v":1,"id":99,"t":"hello"}}"#, page.token);
    let stale_token = envelope(&"f".repeat(32), 99, "hello", json!({}));
    let http_spelling = page.sender.replacen("https://", "http://", 1);
    let long_sender = format!("{}?{}", page.sender, "x".repeat(9_000));
    for (row, source, body) in [
        ("external site", "https://example.com/", subscribe.as_str()),
        ("foreign bundle origin", foreign.as_str(), subscribe.as_str()),
        ("http spelling of the bundle origin", http_spelling.as_str(), subscribe.as_str()),
        ("sender URL past the P-2 cap", long_sender.as_str(), subscribe.as_str()),
        ("another document's token", page.sender.as_str(), stale_token.as_str()),
        ("token not leading the envelope", page.sender.as_str(), reordered.as_str()),
        ("not a string message", page.sender.as_str(), ""),
    ] {
        let before = page.bridge.stats().total.dropped_origin;
        assert_eq!(page.post_from(source, body), Ingress::Dropped, "{row}");
        assert_eq!(page.bridge.stats().total.dropped_origin, before + 1, "{row}");
    }
    page.bridge.set_visible(false);
    assert_eq!(page.post_from(&page.sender.clone(), &subscribe), Ingress::Dropped, "hidden portal");
    page.bridge.set_visible(true);

    // PJ2's fixed rows (the `ipc-post` QA op, fixed sources and envelopes): each non-control row is exactly one origin
    // drop, never delivered or answered; control is heard and answered. Mutation check: a row posted from the admitted
    // document with its live token (or `hidden` left visible) goes red here.
    for row in IpcPostRow::ALL {
        let outcome = qa_rows::post_row(&page.bridge, &page.mounts, row, None, page.now).expect("an admitted bundle is open");
        let heard = row == IpcPostRow::Control;
        assert_eq!(
            (outcome.dropped_origin_delta, outcome.delivered, outcome.replied),
            (u64::from(!heard), heard, false),
            "{row:?}"
        );
    }
    page.pump();
    assert_eq!(page.ok(qa_rows::ROW_ENVELOPE_ID)["api"], "fe.portal.v1", "control is answered");
    let answered = page.received.iter().filter(|message| message["re"] == json!(qa_rows::ROW_ENVELOPE_ID)).count();
    assert_eq!(answered, 1, "only the control row is answered");

    // SP-9, the native path: the sender is judged before the body is read, so an unreadable or over-cap Source, or any
    // origin but the admitted document's, never has its body read; the document's own over-cap or non-string body is
    // dropped unread, not answered. Mutation check: reading the body before check 1 makes `reads` non-zero.
    let reads = Cell::new(0);
    for (row, source) in [
        ("unreadable Source", None),
        ("external site", Some("https://example.com/")),
        ("foreign bundle origin", Some(foreign.as_str())),
        ("sender URL past the P-2 cap", Some(long_sender.as_str())),
    ] {
        let before = page.bridge.stats().total.dropped_origin;
        let read = counted_read(&reads, Some(subscribe.clone()));
        assert_eq!(page.bridge.receive(&page.mounts, source, read, page.now), Ingress::Dropped, "{row}");
        assert_eq!(page.bridge.stats().total.dropped_origin, before + 1, "{row}");
    }
    assert_eq!(reads.get(), 0, "no unauthorised sender's body is ever read");
    let before = page.bridge.stats().total.dropped_origin;
    let refused_body = counted_read(&reads, None);
    assert_eq!(page.bridge.receive(&page.mounts, Some(page.sender.as_str()), refused_body, page.now), Ingress::Dropped);
    assert_eq!((reads.get(), page.bridge.stats().total.dropped_origin), (1, before + 1), "over its cap: dropped unread");
    let heard = page.received.len();
    page.pump();
    assert_eq!(page.received.len(), heard, "no dropped post is ever answered");

    // Check 3 on the authorized document: typed replies, never silence.
    let missing_type = format!(r#"{{"v":1,"n":"{}","id":40}}"#, page.token);
    assert_eq!(page.post_from(&page.sender.clone(), &missing_type), Ingress::Replied);
    let oversized = envelope(&page.token, 41, "emit", json!({ "stream": "selection", "data": "x".repeat(70_000) }));
    assert_eq!(page.post_from(&page.sender.clone(), &oversized), Ingress::Replied);
    let unknown_key = envelope(&page.token, 42, "hello", json!({ "extra": 1 }));
    assert_eq!(page.post_from(&page.sender.clone(), &unknown_key), Ingress::Accepted);
    let wrong_type = envelope(&page.token, 43, "unsubscribe", json!({ "sub": "one" }));
    assert_eq!(page.post_from(&page.sender.clone(), &wrong_type), Ingress::Accepted);
    page.pump();
    for id in [40, 41, 42, 43] {
        assert_eq!(page.error(id), "invalid", "id {id}");
    }
    page.advance(2_000);
    let burst: Vec<Ingress> = (0..60)
        .map(|index| page.post_from(&page.sender.clone(), &envelope(&page.token, 100 + index, "hello", json!({}))))
        .collect();
    assert_eq!(burst.iter().filter(|outcome| **outcome == Ingress::Accepted).count(), 50, "burst of 50");
    page.pump();
    let limited = (100..160).filter(|id| page.reply(*id).is_some_and(|reply| reply["ok"] == json!(false))).count();
    assert_eq!(limited, 10);
    assert_eq!(page.error(159), "rate_limited");

    // A reload starts a new document: the old token is dead and the page API mints a new one.
    let old_token = page.token.clone();
    page.reload();
    page.pump();
    let stale = envelope(&old_token, 200, "hello", json!({}));
    assert_eq!(page.post_from(&page.sender.clone(), &stale), Ingress::Dropped, "stale document");
    page.fetch_token();
    assert_ne!(page.token, old_token);
    let refill_attempt = envelope(&page.token, 201, "hello", json!({}));
    assert_eq!(
        page.post_from(&page.sender.clone(), &refill_attempt),
        Ingress::Replied,
        "budgets are per mount: a reload never refills them"
    );
    page.advance(1_000);
    let fresh = page.send("hello", json!({}));
    page.pump();
    assert_eq!(page.ok(fresh)["api"], "fe.portal.v1");

    // Steer 2026-11-08 23:40: the client opens at the installing identity's role at the scope, never above the viewer's
    // token. A lower viewer ceiling rebinds at once: live streams end, and an act is refused (audited) before the write
    // path. A broker reporting a ceiling above the viewer's token gets no client at all (fail closed).
    page.advance(1_000);
    let stream = page.subscribe("readings", json!({}));
    page.pump();
    let stream_sub = page.ok(stream)["sub"].as_u64().unwrap();
    page.bridge.set_viewer_ceiling(RoleLevel::Viewer);
    page.pump();
    assert_eq!(page.broker.log().last(), Some(&format!("open {PACKAGE} node-repo viewer")));
    let ended = page.deliveries(stream_sub).last().map(|delivery| (delivery["kind"].clone(), delivery["data"].clone()));
    assert_eq!(ended, Some((json!("ended"), json!({ "reason": "role-changed" }))));
    let capped = page.send("act", json!({ "op": "set_property", "node_id": NODE_A, "key": "fleet.speed", "value": 1 }));
    page.pump();
    assert_eq!(page.error(capped), "role_denied");
    assert!(!page.broker.log().iter().any(|line| line.starts_with("set_property")), "nothing reaches the write path");
    assert!(page.broker.audits().contains(&"act set_property:fleet.speed role_denied".to_owned()));
    page.broker.ignore_viewer(true);
    page.bridge.set_viewer_ceiling(RoleLevel::Editor);
    page.pump();
    assert_eq!(page.broker.log().last(), Some(&format!("close {BROKER_CLIENT}")), "an over-ceiling client is closed unused");
    let unbound = page.subscribe("readings", json!({}));
    page.pump();
    assert_eq!(page.error(unbound), "unavailable");
    page.broker.ignore_viewer(false);
    page.bridge.set_viewer_ceiling(RoleLevel::Owner);
    page.pump();
    assert_eq!(page.broker.log().last(), Some(&format!("open {PACKAGE} node-repo owner")));

    // An admitted origin never gets feeds: its data path is IPC (admission.md §7.2).
    assert_eq!(page.get(&page.label.clone(), FEEDS_PATH).status(), StatusCode::NOT_FOUND);

    // The same bytes unadmitted: a static origin, no page API, its posts dropped, and only exact share URLs as feeds.
    let local_api = "http://127.0.0.1:8765";
    let relay = "https://relay.example.org";
    let share = |origin: &str| format!("{origin}/api/v1/shared/{SHARE_TOKEN}");
    let feeds = FeedSource {
        feeds: BTreeMap::from([
            ("flow".to_owned(), share(local_api)),
            ("relay".to_owned(), share(relay)),
            ("other_port".to_owned(), share("http://127.0.0.1:9999")),
            ("intranet".to_owned(), share("https://192.168.1.10")),
            ("bundle_origin".to_owned(), share("https://fe-portal.s-aaaaaaaaaaaaaaaaaaaaaaaaaa.localhost")),
            ("with_query".to_owned(), format!("{}?coords=1", share(local_api))),
            ("other_route".to_owned(), format!("{local_api}/api/v1/query")),
            ("Upper".to_owned(), share(local_api)),
        ]),
        local_api_origin: Some(local_api.to_owned()),
    };
    let resolver = Resolver { path: page.bundle.path.clone(), admit: None, feeds };
    let static_mount = prepare(&resolver, "node-repo", u64::MAX).expect("static mount");
    assert_eq!(static_mount.mode, MountMode::Static);
    let csp = static_mount.csp().to_str().unwrap().to_owned();
    assert!(
        csp.contains(&format!("connect-src 'self' {local_api}/api/v1/shared/ {relay}/api/v1/shared/;")),
        "connect-src gains exactly the kept feeds' share routes, never the rest of either API: {csp}"
    );
    let static_label = static_mount.label.to_string();
    let static_sender = static_mount.entry_url.clone();
    page.mounts.install(static_mount);
    assert_eq!(page.get_page_api(&static_label).status(), StatusCode::NOT_FOUND, "window.fe never exists");
    let before = page.bridge.stats().total.dropped_origin;
    assert_eq!(page.post_from(&static_sender, &envelope(&page.token, 300, "hello", json!({}))), Ingress::Dropped);
    assert_eq!(page.bridge.stats().total.dropped_origin, before + 1);
    let feeds_json = page.get(&static_label, FEEDS_PATH);
    assert_eq!(feeds_json.status(), StatusCode::OK);
    assert_eq!(feeds_json.headers()["content-type"], "application/json; charset=utf-8");
    assert_eq!(feeds_json.headers()["x-content-type-options"], "nosniff");
    let expires_at = "2026-11-09T16:00:00Z";
    assert_eq!(
        serde_json::from_slice::<Value>(feeds_json.body()).unwrap(),
        json!({ "v": 1, "feeds": {
            "flow": { "url": share(local_api), "expires_at": expires_at },
            "relay": { "url": share(relay), "expires_at": expires_at }
        } }),
        "loopback only on this peer's API port, https only where the raw policy allows, the exact route only"
    );
    page.pump();
    assert!(page.broker.log().contains(&format!("close {BROKER_CLIENT}")), "unmounting the admitted build closes its client");
}

/// R9-R11: subscriptions stream without polling, actions are judged at use, bus traffic is declared-only.
#[test]
fn subscriptions_actions_and_bus_route_through_the_broker() {
    let mut page = Page::open();

    // R9: node changes — snapshot first, then deltas paced to 4 Hz with the last state winning.
    let node = page.subscribe("node", json!({}));
    page.pump();
    let node_sub = page.ok(node)["sub"].as_u64().unwrap();
    assert!(page.broker.log().contains(&"subscribe host/node-changes {}".to_owned()));
    page.broker.push(1, DeliveryKind::Snapshot, json!({ "part": 0, "last": true, "nodes": [{ "node_id": NODE_A }] }));
    page.pump();
    for step in 0..100 {
        let change = json!({ "op": "node_transform", "node_id": NODE_A, "petal_id": PETAL,
                             "position": [step, 0, 0], "rotation": [0, 0, 0], "scale": [1, 1, 1] });
        page.broker.push(1, DeliveryKind::Delta, json!({ "changes": [change] }));
        page.pump();
        page.advance(10);
    }
    page.advance(250);
    page.pump();
    let deliveries = page.deliveries(node_sub);
    assert_eq!(deliveries[0]["kind"], "snapshot");
    let deltas: Vec<&&Value> = deliveries.iter().filter(|delivery| delivery["kind"] == "delta").collect();
    assert!(deltas.len() <= 5, "100 changes in 1 s → at most 5 deliveries, got {}", deltas.len());
    let last = deltas.last().unwrap()["data"]["changes"].as_array().unwrap().last().unwrap().clone();
    assert_eq!(last["position"][0], json!(99), "the last delivery equals the final state");
    let seqs: Vec<u64> = deliveries.iter().map(|delivery| delivery["seq"].as_u64().unwrap()).collect();
    assert_eq!(seqs, (0..seqs.len() as u64).collect::<Vec<_>>(), "seq starts at 0 and steps by 1");

    // Idle for 60 s: no subscribe, call or write reaches the broker (no timer polling, QA-0125).
    let traffic = page.broker.log().len();
    for _ in 0..240 {
        page.advance(250);
        page.pump();
    }
    assert_eq!(page.broker.log().len(), traffic);

    // Out of scope answers REST's not_found: another petal, or a node the petal's snapshot lacks.
    let other_petal = page.subscribe("node", json!({ "petal_id": "01JOTHERPETAL0000000000000" }));
    let missing = page.subscribe("node", json!({ "node_id": "01JMISSING0000000000000000" }));
    page.pump();
    assert_eq!(page.error(other_petal), "not_found");
    assert!(page.reply(missing).is_none(), "scope is proven by the snapshot before the reply");
    page.broker.push(2, DeliveryKind::Snapshot, json!({ "part": 0, "last": true, "nodes": [{ "node_id": NODE_A }] }));
    page.pump();
    assert_eq!(page.error(missing), "not_found");
    assert!(page.broker.log().contains(&"unsubscribe 2".to_owned()));

    // Readings and named queries: parameters map onto the bus; only declared queries with typed args.
    let readings = page.subscribe("readings", json!({ "node_id": NODE_A }));
    page.pump();
    let readings_sub = page.ok(readings)["sub"].as_u64().unwrap();
    assert_eq!(page.deliveries(readings_sub)[0]["data"], json!({ "readings": [] }), "an empty snapshot first");
    assert!(page.broker.log().contains(&format!(r#"subscribe host/readings {{"anchors":["{NODE_A}"]}}"#)));
    for (params, expected) in [
        (json!({ "name": "adhoc" }), Some("capability_denied")),
        (json!({ "name": "hourly_flow", "args": { "metric": 3 } }), Some("invalid")),
        (json!({ "name": "hourly_flow", "args": {} }), Some("invalid")),
        (json!({ "name": "hourly_flow", "args": { "metric": "flow" } }), None),
    ] {
        let id = page.subscribe("query", params.clone());
        page.pump();
        match expected {
            Some(code) => assert_eq!(page.error(id), code, "{params}"),
            None => assert!(page.ok(id)["sub"].is_u64(), "{params}"),
        }
    }
    assert!(page
        .broker
        .log()
        .contains(&r#"subscribe host/query {"query":"hourly_flow","vars":{"metric":"flow"}}"#.to_owned()));

    // R11: a declared hook's events reach the page, coalesced into one delta.
    let hook = page.subscribe("org.example.fleet-sim/positions", json!({}));
    page.pump();
    let hook_sub = page.ok(hook)["sub"].as_u64().unwrap();
    let hook_broker_sub = page.broker.log().iter().filter(|line| line.starts_with("subscribe ")).count() as u64;
    page.broker.push(hook_broker_sub, DeliveryKind::Delta, json!({ "count": 4 }));
    page.pump();
    assert_eq!(page.deliveries(hook_sub)[0]["data"], json!({ "events": [{ "count": 4 }] }));
    let undeclared_hook = page.subscribe("org.example.other/feed", json!({}));
    page.pump();
    assert_eq!(page.error(undeclared_hook), "capability_denied");

    // Sixteen live subscriptions, then too_many_subscriptions.
    while page.bridge.stats().total.live_subscriptions < 16 {
        let id = page.subscribe("org.example.fleet-sim/positions", json!({}));
        page.pump();
        assert!(page.ok(id)["sub"].is_u64());
    }
    let seventeenth = page.subscribe("readings", json!({}));
    page.pump();
    assert_eq!(page.error(seventeenth), "too_many_subscriptions");

    // R10: set_property judged at use; undeclared and reserved keys never reach the write path.
    page.advance(3_000);
    for (key, expected) in [
        ("fleet.speed", None),
        ("console.note", Some("capability_denied")),
        ("portal.bundle_asset", Some("capability_denied")),
        ("node_kind", Some("capability_denied")),
    ] {
        let id = page.send("act", json!({ "op": "set_property", "node_id": NODE_A, "key": key, "value": 4 }));
        page.pump();
        match expected {
            Some(code) => assert_eq!(page.error(id), code, "{key}"),
            None => assert_eq!(page.ok(id), json!({ "node_id": NODE_A, "key": key })),
        }
        let wrote = page.broker.log().contains(&format!("set_property {NODE_A} {key}"));
        assert_eq!(wrote, expected.is_none(), "{key}: only a declared key reaches the write path");
    }
    assert!(page.broker.audits().contains(&"act set_property:console.note capability_denied".to_owned()));
    page.broker.set_write_outcome(Err(IpcError::RoleDenied));
    page.advance(3_000);
    let demoted = page.send("act", json!({ "op": "set_property", "node_id": NODE_A, "key": "fleet.speed", "value": 5 }));
    page.pump();
    assert_eq!(page.error(demoted), "role_denied", "a demoted identity is refused at write time");
    page.broker.set_write_outcome(Ok(Value::Null));
    page.advance(3_000);
    let mut accepted = 0;
    for _ in 0..20 {
        let id = page.send("act", json!({ "op": "set_property", "node_id": NODE_A, "key": "fleet.speed", "value": 6 }));
        page.pump();
        if page.reply(id).is_some_and(|reply| reply["ok"] == json!(true)) {
            accepted += 1;
        } else {
            assert_eq!(page.error(id), "rate_limited");
        }
        page.advance(50);
    }
    assert!((5..=7).contains(&accepted), "20 calls in 1 s → burst 5 plus refill, got {accepted}");

    // R11: declared emit/invoke reach the broker; undeclared ones are refused before it.
    for (kind, fields, expected) in [
        ("emit", json!({ "stream": "selection", "data": { "node": NODE_A } }), Ok(json!({}))),
        ("emit", json!({ "stream": "undeclared", "data": {} }), Err("capability_denied")),
        ("invoke", json!({ "package": "org.example.fleet-sim", "command": "start", "args": {} }),
         Ok(json!({ "result": { "started": 1 } }))),
        ("invoke", json!({ "package": "org.example.fleet-sim", "command": "stop", "args": {} }), Err("capability_denied")),
        ("intent", json!({ "intent": "focus_node", "node_id": NODE_A }), Ok(json!({}))),
        ("intent", json!({ "intent": "select_node", "node_id": NODE_A }), Err("capability_denied")),
    ] {
        let id = page.send(kind, fields.clone());
        page.pump();
        match expected {
            Ok(data) => assert_eq!(page.ok(id), data, "{kind} {fields}"),
            Err(code) => assert_eq!(page.error(id), code, "{kind} {fields}"),
        }
    }
    let log = page.broker.log();
    assert!(log.contains(&"emit selection".to_owned()) && !log.contains(&"emit undeclared".to_owned()));
    assert!(log.contains(&"invoke org.example.fleet-sim/start".to_owned()));
    assert!(!log.contains(&"invoke org.example.fleet-sim/stop".to_owned()));
    // One audit entry per refusal (ipc-v1.md §9): the bridge audits its own check-5 refusal (`stop`, undeclared); a
    // declared target the bus refuses is audited by the bus, never a second time here.
    assert!(page.broker.audits().contains(&"invoke org.example.fleet-sim/stop capability_denied".to_owned()));
    page.broker.refuse_next_invoke(IpcError::CapabilityDenied);
    let refused_at_bus = page.send("invoke", json!({ "package": "org.example.fleet-sim", "command": "start", "args": {} }));
    page.pump();
    assert_eq!(page.error(refused_at_bus), "capability_denied");
    assert!(
        !page.broker.audits().iter().any(|line| line.starts_with("invoke org.example.fleet-sim/start")),
        "the bus's refusal is not audited twice"
    );
    assert_eq!(
        page.intents,
        vec![PortalIntent {
            intent: UiIntent::FocusNode,
            node_id: NODE_A.into(),
            petal_id: PETAL.into(),
            package_id: PACKAGE.into()
        }]
    );
    let stats = page.bridge.stats();
    assert!(stats.total.refused_capability >= 6 && stats.total.refused_role >= 3 && stats.total.rate_limited >= 13);

    // A reload ends every subscription at the broker.
    let unsubscribed = |log: &[String]| log.iter().filter(|line| line.starts_with("unsubscribe ")).count();
    let before = unsubscribed(&page.broker.log());
    page.reload();
    page.pump();
    assert_eq!(unsubscribed(&page.broker.log()), before + 16);
    assert_eq!(page.bridge.stats().total.live_subscriptions, 0);

    // PJ3 (admission.md §5): three live subscriptions and one queued act, then a revocation list names this build's
    // hash under another package id. The revoke runs in one call, before the bridge pumps again.
    page.fetch_token();
    page.advance(2_000);
    for stream in ["node", "readings", "org.example.fleet-sim/positions"] {
        let id = page.subscribe(stream, json!({}));
        page.pump();
        assert!(page.ok(id)["sub"].is_u64(), "{stream}");
    }
    assert_eq!(page.bridge.stats().total.live_subscriptions, 3);
    page.send("act", json!({ "op": "set_property", "node_id": NODE_A, "key": "fleet.queued", "value": 1 }));
    let mount = page.mounts.current().expect("still mounted");
    let mut webview = FakeWebview::default();
    let mut host = BundleHost::new(BundleParts { mounts: page.mounts.clone(), gate: Rc::clone(&page.gate), supported: true });
    let listed = AdmissionEvent {
        package_id: "org.example.renamed".into(),
        content_hash: mount.content_hash.clone(),
        state: RecordState::Revoked,
        reason: RecordReason::RevocationList,
    };
    let revoked = host.apply_admission(&AdmissionSignal::Event(listed), &mut webview, Some(&page.bridge));
    assert_eq!(states(&revoked), vec![BundleState::Revoked(RevokedReason::RevocationList)]);
    assert!(page.mounts.current().is_none());
    assert_eq!(page.get(&page.label.clone(), "/index.html").status(), StatusCode::NOT_FOUND, "no byte after the decision");
    assert_eq!(page.bridge.stats().total.live_subscriptions, 0);
    let log = page.broker.log();
    assert_eq!(unsubscribed(&log), before + 16 + 3, "every subscription ends at the broker inside the revoke");
    assert_eq!(log.last(), Some(&format!("close {BROKER_CLIENT}")));
    let dropped = page.bridge.stats().total.dropped_origin;
    let stale = envelope(&page.token, 900, "hello", json!({}));
    assert_eq!(page.post_from(&page.sender.clone(), &stale), Ingress::Dropped, "the dying document is not heard");
    assert_eq!(page.bridge.stats().total.dropped_origin, dropped + 1);
    page.pump();
    assert!(!page.broker.log().iter().any(|line| line.ends_with("fleet.queued")), "the queued act never reaches the write path");
    assert_eq!(webview.loads, vec![UNLOAD_URL.to_owned()]);
    assert!(!webview.shown);
    assert_eq!(page.gate.decide(&main_frame(UNLOAD_URL)), Decision::AllowHostUnload);
    let static_twin = static_label(&mount.content_hash).unwrap().to_string();
    assert_eq!(webview.cleared, vec![page.label.clone(), static_twin], "SP-6 clears both of the build's origins");

    // Re-opening renders nothing while the list stands; the archive digest alone matches, before any parse.
    let governance = Arc::new(FakeGovernance::default());
    governance.kill(&mount.archive_digest);
    let reopen: Arc<dyn BundleResolver> = GovernedResolver::new(&page.bundle.path, &governance);
    assert!(host.open("node-repo", Some(reopen), &mut webview).is_empty());
    assert_eq!(states(&settle(&mut host, &mut webview)), vec![BundleState::Revoked(RevokedReason::RevocationList)]);
    assert!(page.mounts.current().is_none());
    assert_eq!(webview.loads, vec![UNLOAD_URL.to_owned()], "no entry is loaded");

    // admission.md §4.2/§4.7: an entry naming one member's digest under another id refuses the build too (a repackaged
    // bundle with the same files). The Owner's audited override for this build in this verse lets it render as its
    // record allows (static until a fresh Admit), and it never reaches another target.
    assert!(!mount.member_digests.is_empty(), "sealed: the signed members.json digests");
    let member_listed = Arc::new(FakeGovernance::default());
    member_listed.kill(&mount.member_digests[0]);
    let reopen: Arc<dyn BundleResolver> = GovernedResolver::new(&page.bundle.path, &member_listed);
    host.open("node-repo", Some(Arc::clone(&reopen)), &mut webview);
    assert_eq!(states(&settle(&mut host, &mut webview)), vec![BundleState::Revoked(RevokedReason::RevocationList)]);
    member_listed.override_for(&BundleTarget::Node("node-repo".into()), &mount.content_hash);
    host.open("node-repo", Some(Arc::clone(&reopen)), &mut webview);
    assert_eq!(states(&settle(&mut host, &mut webview)), vec![BundleState::StaticSandbox { can_admit: true }]);
    host.open("node-elsewhere", Some(reopen), &mut webview);
    assert_eq!(states(&settle(&mut host, &mut webview)), vec![BundleState::Revoked(RevokedReason::RevocationList)]);

    // PJ9 (admission.md §6): governance carries consent over to every build of the package, but the portal admits only
    // the capabilities consent covered. v2 keeps the admitted origin; v3 adds a key scope and opens Static on its own
    // origin; consent for v3 promotes the open portal back to the admitted origin.
    let key = SigningKey::from_bytes(&[9u8; 32]);
    let v2 = sealed_bundle_with(&key, "1.1.0", &["fleet.*"]);
    let v3 = sealed_bundle_with(&key, "1.2.0", &["fleet.*", "console.*"]);
    let facts = |path: &Path| {
        let unjudged = Resolver { path: path.to_path_buf(), admit: None, feeds: FeedSource::default() };
        let mount = prepare(&unjudged, "node-upd", u64::MAX).expect("identity");
        (mount.content_hash.clone(), mount.capability_digest.clone())
    };
    let (h1, d1) = facts(&page.bundle.path);
    let (h2, d2) = facts(&v2.path);
    let (h3, d3) = facts(&v3.path);
    assert_eq!(d1, d2, "a version bump alone changes no capability");
    assert_ne!(d1, d3);
    let governance = Arc::new(FakeGovernance::default());
    for hash in [&h1, &h2, &h3] {
        governance.admit(PACKAGE, hash, &page.publisher, &d1);
    }
    let node = GovernedResolver::new(&page.bundle.path, &governance);
    let node_resolver: Arc<dyn BundleResolver> = node.clone();
    let mounts = PortalMounts::new();
    let mut host = BundleHost::new(BundleParts {
        mounts: mounts.clone(),
        gate: Rc::new(NavigationGate::new(mounts.clone())),
        supported: true,
    });
    let mut webview = FakeWebview::default();
    let admitted = BundleState::Admitted { publisher: page.publisher.clone() };
    host.open("node-upd", Some(node_resolver), &mut webview);
    assert_eq!(states(&settle(&mut host, &mut webview)), vec![admitted.clone()]);
    let admitted_origin = mounts.current().expect("v1").label.clone();
    node.point_at(&v2.path);
    assert!(host.refresh("node-upd", &mut webview).is_empty(), "a reload, not a Close");
    assert_eq!(states(&settle(&mut host, &mut webview)), vec![admitted.clone()]);
    assert_eq!(mounts.current().expect("v2").label, admitted_origin, "same publisher, same capabilities: storage follows");
    node.point_at(&v3.path);
    host.refresh("node-upd", &mut webview);
    assert_eq!(states(&settle(&mut host, &mut webview)), vec![BundleState::StaticSandbox { can_admit: true }]);
    let static_v3 = mounts.current().expect("v3");
    assert_eq!(static_v3.label, static_label(&h3).unwrap(), "a capability increase never inherits the admitted origin");
    governance.admit(PACKAGE, &h3, &page.publisher, &d3);
    let consent = AdmissionEvent {
        package_id: PACKAGE.into(),
        content_hash: h3.clone(),
        state: RecordState::Admitted,
        reason: RecordReason::Consented,
    };
    assert!(host.apply_admission(&AdmissionSignal::Event(consent), &mut webview, None).is_empty());
    assert_eq!(states(&settle(&mut host, &mut webview)), vec![admitted.clone()]);
    assert_eq!(mounts.current().expect("re-admitted v3").label, admitted_origin);

    // Bindings §6 `portal.open` (B10f): a petal's bound bundle mounts on a local Open of its target, under the same
    // admitted origin, and its broker client is scoped by the target (no repository node). Bytes of another package
    // never mount under a binding that names this one.
    let bound = BundleTarget::Bound { petal_id: PETAL.into(), package_id: PACKAGE.into() };
    let bound_resolver: Arc<dyn BundleResolver> = node.clone();
    host.open_target(bound.clone(), Some(Arc::clone(&bound_resolver)), &mut webview);
    assert_eq!(states(&settle(&mut host, &mut webview)), vec![admitted]);
    let bound_mount = mounts.current().expect("bound bundle");
    assert_eq!((&bound_mount.target, bound_mount.node_id.as_str()), (&bound, format!("binding:{PETAL}:{PACKAGE}").as_str()));
    assert_eq!(bound_mount.label, admitted_origin, "one origin per (publisher, package), whatever the scope");
    // The by-name command carries no URL, path or hash and keys its chrome events by the same anchor the mount reports.
    let open_bound = fe_webview::ipc::BrowserCommand::OpenBoundBundle { petal_id: PETAL.into(), package_id: PACKAGE.into() };
    assert_eq!(open_bound.bundle_anchor().as_deref(), Some(bound_mount.node_id.as_str()));
    assert!(!serde_json::to_string(&open_bound).unwrap().contains("url"));
    let bound_broker = FakeBroker::new();
    let bound_bridge = PortalBridge::new();
    let dyn_bound_broker: Arc<dyn PortalBroker> = bound_broker.clone();
    bound_bridge.pump(&mounts, Some(&dyn_bound_broker), Instant::now());
    assert_eq!(bound_broker.log().first(), Some(&format!("open {PACKAGE} binding:{PETAL}:{PACKAGE} owner")));
    let renamed = BundleTarget::Bound { petal_id: PETAL.into(), package_id: "org.example.other".into() };
    host.open_target(renamed, Some(bound_resolver), &mut webview);
    assert_eq!(states(&settle(&mut host, &mut webview)), vec![BundleState::Blocked(BlockedReason::Integrity)]);
    assert!(mounts.current().is_none());
}
