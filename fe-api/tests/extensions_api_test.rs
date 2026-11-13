//! J6: the extensions API surface (spec R11/R12/R15) through the shared core functions REST, MCP and
//! `/ws` all call, against a recording stand-in for the plugin host (the real broker is proven in
//! fe-plugin `bus/tests.rs`). Asserts what callers observe: statuses, bodies, and what reached the host.
//! P2.2 extends it with governance: the R19 refusal statuses, the verse policy judged by fe-database's
//! real policy rules, and the Owner's kill-list override (the R19 ORDER is fe-plugin's lifecycle journey).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use fe_api::extensions::{
    administer, caller_of, command, extension_audit, extension_command_tool, extension_status,
    get_verse_policy, ingress, list_extensions, put_verse_policy, AdminOp, AdminRefusal, AdminReply,
    AdminVerb, ApiCaller, ApiCallerKind, CommandFailure, CommandReply, ExtError, ExtensionHost,
    ExtensionStreams, HostRefusal, IngressAnswer, IngressCall, IngressRefusal, IngressRequestParts, WsAcceptCall,
    WsData, WsHostSession, WsOut, open_ingress_websocket,
};
use fe_database::handlers::extension_policy::{evaluate, ExtensionPolicy, LocalPolicy, PolicyFacts, TrustTier};
use fe_identity::api_token::ApiClaims;
use fe_sdk::manifest_v2::ExtensionKind;
use serde_json::{json, Value};
use tokio::sync::{oneshot, Notify};

const LOCAL: &str = "did:key:z6MkLocalHost";
const REMOTE: &str = "did:key:z6MkRemoteEditor";
const PETAL: &str = "petal-1";
const PACKAGE: &str = "org.example.fleet-sim";
const VERSE: &str = "01JVERSEPOLICY0000000000000";
/// A build's component digest a verse denies (matched under any package id).
const DENIED_DIGEST: &str = "9f2c000000000000000000000000000000000000000000000000000000000000";

fn claims(sub: &str) -> ApiClaims {
    ApiClaims {
        sub: sub.to_string(),
        token_type: "api".into(),
        scope: "VERSE#v1".into(),
        max_role: "owner".into(),
        iat: 0,
        exp: u64::MAX,
        jti: "jti".into(),
        client: None,
    }
}

#[derive(Default)]
struct Seen {
    admin: Vec<(String, AdminVerb, Option<String>)>,
    denied: Vec<(String, String)>,
    commands: Vec<(ApiCaller, String)>,
    ingress: Vec<IngressCall>,
    closed_clients: Vec<u64>,
    remote_budget: u32,
    /// Stored verse policies, as the binary's host keeps them in fe-database.
    policies: HashMap<String, Value>,
    policy_writers: Vec<String>,
    /// Websocket accepts the stand-in saw, the frames it kept, the over-cap frames it dropped, closes reported.
    ws_accepts: Vec<WsAcceptCall>,
    ws_kept: Vec<WsData>,
    ws_dropped: u32,
    ws_closed: Vec<u64>,
}

/// The stand-in endpoint's `max_frame_bytes` and `max_connections`.
const WS_FRAME_CAP: usize = 8;
const WS_CONNECTION_CAP: usize = 1;

/// Row 7 as the binary's policy gate runs it: fe-database's real `evaluate` over the stored verse policy.
fn policy_gate(stored: Option<&Value>, params: &Value) -> Result<(), AdminRefusal> {
    let text = |key: &str| params[key].as_str().unwrap_or_default();
    let policy = stored.map(|value| serde_json::from_value::<ExtensionPolicy>(value.clone()).expect("stored policy"));
    let trust = serde_json::from_value::<TrustTier>(params["trust"].clone()).unwrap_or(TrustTier::Publisher);
    let facts = PolicyFacts {
        package_id: text("package_id"),
        version: text("version"),
        content_hash: text("content_hash"),
        component_digest: params["component_digest"].as_str(),
        plaintext_digest: None,
        trust,
        kind: ExtensionKind::General,
        manifest: None,
    };
    evaluate(policy.as_ref(), &LocalPolicy::default(), &facts)
        .map_err(|denial| AdminRefusal { code: "policy-denied".into(), message: denial.to_string() })
}

struct FakeHost {
    seen: Mutex<Seen>,
}

impl FakeHost {
    fn new(remote_budget: u32) -> Arc<Self> {
        Arc::new(Self { seen: Mutex::new(Seen { remote_budget, ..Seen::default() }) })
    }
}

impl ExtensionHost for FakeHost {
    fn local_did(&self) -> String {
        LOCAL.into()
    }
    fn admit_remote(&self, _did: &str) -> bool {
        let mut seen = self.seen.lock().unwrap();
        let ok = seen.remote_budget > 0;
        seen.remote_budget = seen.remote_budget.saturating_sub(1);
        ok
    }
    fn list(&self) -> Vec<Value> {
        vec![json!({"installation": "inst-1", "state": "active"})]
    }
    fn status(&self, installation: &str) -> Option<Value> {
        (installation == "inst-1").then(|| json!({"installation": "inst-1", "state": "active", "ticks_run": 120}))
    }
    fn audit(&self, installation: &str, limit: usize) -> Option<Vec<Value>> {
        (installation == "inst-1").then(|| vec![json!({"action": "command-call", "limit": limit})])
    }
    fn administer(&self, actor_did: &str, op: AdminOp) -> Result<Value, AdminRefusal> {
        let mut seen = self.seen.lock().unwrap();
        seen.admin.push((actor_did.into(), op.verb, op.installation.clone()));
        if let Some(code) = op.params["refuse"].as_str() {
            return Err(AdminRefusal { code: code.into(), message: "scripted refusal".into() });
        }
        if op.verb == AdminVerb::Consent && op.params["digest"] != "good" {
            return Err(AdminRefusal { code: "consent-mismatch".into(), message: "stale digest".into() });
        }
        if op.verb == AdminVerb::Bind && op.params.get("verse_id").is_some() {
            let verse = op.params["verse_id"].as_str().unwrap_or_default();
            policy_gate(seen.policies.get(verse), &op.params)?;
        }
        Ok(json!({"done": op.verb.as_str()}))
    }
    fn record_denied_admin(&self, actor_did: &str, target: &str) {
        self.seen.lock().unwrap().denied.push((actor_did.into(), target.into()));
    }
    fn call_command(
        &self,
        caller: &ApiCaller,
        petal_id: &str,
        package_id: &str,
        command: &str,
        _args: &Value,
    ) -> Result<CommandReply, HostRefusal> {
        self.seen.lock().unwrap().commands.push((caller.clone(), command.into()));
        if package_id != PACKAGE || petal_id != PETAL {
            return Err(HostRefusal::NotFound);
        }
        let (tx, rx) = oneshot::channel();
        let _ = tx.send(match command {
            "start" => Ok(r#"{"started":1}"#.to_string()),
            "stop" => Err(CommandFailure::Refused("not running".into())),
            _ => return Err(HostRefusal::NotFound),
        });
        Ok(rx)
    }
    fn ingress(&self, call: IngressCall) -> Result<oneshot::Receiver<Option<IngressAnswer>>, IngressRefusal> {
        self.seen.lock().unwrap().ingress.push(call);
        let (tx, rx) = oneshot::channel();
        let _ = tx.send(Some(IngressAnswer { status: 202, content_type: None, body: b"ok".to_vec() }));
        Ok(rx)
    }
    fn open_websocket(&self, call: WsAcceptCall) -> Result<WsHostSession, IngressRefusal> {
        let mut seen = self.seen.lock().unwrap();
        if seen.ws_accepts.len() - seen.ws_closed.len() >= WS_CONNECTION_CAP {
            return Err(IngressRefusal::TooManyRequests);
        }
        seen.ws_accepts.push(call);
        let (tx, outbound) = tokio::sync::mpsc::channel(4);
        let _ = tx.try_send(WsOut::Frame(WsData::Text("hello".into())));
        Ok(WsHostSession { connection: seen.ws_accepts.len() as u64, outbound })
    }
    fn websocket_frame(&self, _connection: u64, frame: WsData) {
        let mut seen = self.seen.lock().unwrap();
        let len = match &frame {
            WsData::Text(text) => text.len(),
            WsData::Binary(bytes) => bytes.len(),
        };
        if len > WS_FRAME_CAP {
            seen.ws_dropped += 1;
        } else {
            seen.ws_kept.push(frame);
        }
    }
    fn websocket_closed(&self, connection: u64) {
        self.seen.lock().unwrap().ws_closed.push(connection);
    }
    fn open_client(&self, _caller: &ApiCaller) -> (u64, Arc<Notify>) {
        (7, Arc::new(Notify::new()))
    }
    fn client_subscribe(&self, _client: u64, _petal: &str, source: &str, _params: &Value) -> Result<u64, HostRefusal> {
        if source == "org.example.fleet-sim/positions" {
            Ok(42)
        } else {
            Err(HostRefusal::NotFound)
        }
    }
    fn client_unsubscribe(&self, _client: u64, _subscription: u64) {}
    fn take_client_events(&self, _client: u64, _max: usize) -> Vec<String> {
        vec![r#"{"sub":42,"seq":1}"#.into()]
    }
    fn close_client(&self, client: u64) {
        self.seen.lock().unwrap().closed_clients.push(client);
    }
    fn verse_policy(&self, verse_id: &str) -> AdminReply {
        let (tx, rx) = oneshot::channel();
        let policy = self.seen.lock().unwrap().policies.get(verse_id).cloned();
        let _ = tx.send(Ok(json!({"verse_id": verse_id, "policy": policy})));
        rx
    }
    fn set_verse_policy(&self, actor_did: &str, verse_id: &str, policy: Value) -> AdminReply {
        // The real shape rules the DB thread applies (fe-database `ExtensionPolicy::validate`).
        let (tx, rx) = oneshot::channel();
        let parsed = serde_json::from_value::<ExtensionPolicy>(policy)
            .map_err(|e| e.to_string())
            .and_then(|policy| policy.validate(verse_id).map(|()| policy));
        let _ = tx.send(match parsed {
            Ok(policy) => {
                let mut seen = self.seen.lock().unwrap();
                seen.policies.insert(verse_id.into(), serde_json::to_value(&policy).unwrap());
                seen.policy_writers.push(actor_did.into());
                Ok(json!({"verse_id": verse_id, "written": true}))
            }
            Err(reason) => Err(AdminRefusal { code: "invalid-policy".into(), message: reason }),
        });
        rx
    }
}

fn parts(headers: Vec<(&str, &str)>, body: Vec<u8>) -> IngressRequestParts {
    IngressRequestParts {
        petal_id: PETAL.into(),
        package_id: PACKAGE.into(),
        endpoint: "devices".into(),
        method: "post".into(),
        sub_path: "a/b".into(),
        query: vec![],
        headers: headers.into_iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
        content_type: Some("application/json".into()),
        body,
    }
}

fn op(verb: AdminVerb, installation: Option<&str>, params: Value) -> AdminOp {
    AdminOp { verb, installation: installation.map(str::to_string), params }
}

/// The GUI / `fe-ext admin` path: the same host judging an administration verb as the local DID, never through the API.
fn gui(host: &FakeHost, op: AdminOp) -> Result<Value, ExtError> {
    host.administer(LOCAL, op).map_err(ExtError::from)
}

#[tokio::test]
async fn extensions_api_journey() {
    let host = FakeHost::new(6);
    let (local, remote) = (claims(LOCAL), claims(REMOTE));

    // R11: the local DID reads status, counters and audit; limit is clamped.
    assert_eq!(list_extensions(host.as_ref(), &local).unwrap()["extensions"][0]["state"], "active");
    assert_eq!(extension_status(host.as_ref(), &local, "inst-1").unwrap()["ticks_run"], 120);
    assert_eq!(extension_audit(host.as_ref(), &local, "inst-1", Some(10_000)).unwrap()["entries"][0]["limit"], 500);

    // Uniform not-found: a remote DID reading a real installation sees exactly what an unknown id gives the owner.
    let remote_real = extension_status(host.as_ref(), &remote, "inst-1").unwrap_err();
    let local_unknown = extension_status(host.as_ref(), &local, "nope").unwrap_err();
    assert_eq!(remote_real, local_unknown);
    assert_eq!(remote_real.status, 404);

    // R12: remote administration never reaches the host, leaves one denial row, and is a 404.
    let denied = administer(host.as_ref(), &remote, op(AdminVerb::Install, None, json!({"package_id": PACKAGE}))).unwrap_err();
    assert_eq!(denied, ExtError::not_found());
    {
        let seen = host.seen.lock().unwrap();
        assert!(seen.admin.is_empty());
        assert_eq!(seen.denied, vec![(REMOTE.to_string(), "install".to_string())]);
    }
    // G1: fe-identity mints every token with the node DID as `sub`; an editor token is a delegate, never the local identity.
    let delegate = ApiClaims { max_role: "editor".into(), ..claims(LOCAL) };
    let delegated = administer(host.as_ref(), &delegate, op(AdminVerb::Uninstall, Some("inst-1"), json!({}))).unwrap_err();
    assert_eq!(delegated, ExtError::not_found());
    assert!(host.seen.lock().unwrap().admin.is_empty(), "a delegated token never reaches the host's lifecycle");
    // Administration is GUI-only: even the Owner's local token is refused (403, one denial row) and the host never runs the verb.
    for verb in [AdminVerb::Install, AdminVerb::Consent, AdminVerb::Bind, AdminVerb::Unbind, AdminVerb::Enable, AdminVerb::Update, AdminVerb::Uninstall, AdminVerb::KillListOverride] {
        let refused = administer(host.as_ref(), &local, op(verb, Some("inst-1"), json!({"digest": "good"}))).unwrap_err();
        assert_eq!((refused.status, refused.code), (403, "admin-gui-only"), "{verb:?}");
        assert!(refused.message.contains("Extensions panel or fe-ext admin"), "{}", refused.message);
    }
    {
        let seen = host.seen.lock().unwrap();
        assert!(seen.admin.is_empty(), "no API caller reaches the host's lifecycle");
        assert_eq!(seen.denied.last().unwrap(), &(LOCAL.to_string(), "kill-list-override@inst-1".to_string()));
    }
    // The same host judges the verb for the GUI / CLI; a precondition refusal maps to its suggested status.
    assert_eq!(gui(host.as_ref(), op(AdminVerb::Consent, Some("inst-1"), json!({"digest": "good"}))).unwrap()["done"], "consent");
    let stale = gui(host.as_ref(), op(AdminVerb::Consent, Some("inst-1"), json!({"digest": "old"}))).unwrap_err();
    assert_eq!((stale.status, stale.code), (409, "consent-mismatch"));

    // R15 commands: the local caller gets the guest's result; guest refusals map to 409; an out-of-scope
    // petal is the same not-found as an uninstalled package.
    let started = command(host.as_ref(), &local, ApiCallerKind::Rest, true, PETAL, PACKAGE, "start", json!({})).await.unwrap();
    assert_eq!(started, json!({"started": 1}));
    let refused = command(host.as_ref(), &local, ApiCallerKind::Rest, true, PETAL, PACKAGE, "stop", json!({})).await.unwrap_err();
    assert_eq!(refused.status, 409);
    let out_of_scope = command(host.as_ref(), &local, ApiCallerKind::Rest, false, PETAL, PACKAGE, "start", json!({})).await.unwrap_err();
    let uninstalled = command(host.as_ref(), &local, ApiCallerKind::Rest, true, PETAL, "org.example.other", "start", json!({})).await.unwrap_err();
    assert_eq!(out_of_scope, uninstalled);
    let bad_args = command(host.as_ref(), &local, ApiCallerKind::Rest, true, PETAL, PACKAGE, "start", json!([1])).await.unwrap_err();
    assert_eq!(bad_args.status, 400);

    // MCP: same core, caller kind mcp; missing arguments are invalid-input.
    let mcp = extension_command_tool(host.as_ref(), &local, true, &json!({"petal_id": PETAL, "package_id": PACKAGE, "command": "start"})).await.unwrap();
    assert_eq!(mcp, json!({"started": 1}));
    assert_eq!(host.seen.lock().unwrap().commands.last().unwrap().0, caller_of(&local, ApiCallerKind::Mcp));
    assert_eq!(extension_command_tool(host.as_ref(), &local, true, &json!({"petal_id": PETAL})).await.unwrap_err().status, 400);

    // Ingress: credential headers are stripped by the host whatever the caller sent (every case, every `x-fe-*`); the body ceiling is 413.
    let sent = vec![
        ("Authorization", "Bearer secret"),
        ("Proxy-Authorization", "Basic cHJveHk="),
        ("cookie", "s=1"),
        ("Set-Cookie", "s=2"),
        ("x-api-key", "k"),
        ("x-fe-join-lease", "l"),
        ("X-FE-Session", "s"),
        ("x-device-id", "d1"),
    ];
    let answer = ingress(host.as_ref(), &local, true, parts(sent, b"{}".to_vec())).await.unwrap();
    assert_eq!((answer.status, answer.body.as_slice()), (202, b"ok".as_slice()));
    {
        let seen = host.seen.lock().unwrap();
        let call = seen.ingress.last().unwrap();
        assert_eq!(call.headers, vec![("x-device-id".to_string(), "d1".to_string())]);
        assert_eq!((call.method.as_str(), call.caller_did.as_str()), ("POST", LOCAL));
    }
    let too_big = ingress(host.as_ref(), &local, true, parts(vec![], vec![0; 1024 * 1024 + 1])).await.unwrap_err();
    assert_eq!(too_big.status, 413);

    // Websocket accept: the same gate and petal scope; the host sees the caller and ceiling, never a header.
    let editor_token = ApiClaims { max_role: "editor".into(), ..claims(LOCAL) };
    let mut session = open_ingress_websocket(host.as_ref(), &editor_token, true, PETAL, PACKAGE, "stream").unwrap();
    {
        let seen = host.seen.lock().unwrap();
        let accept = seen.ws_accepts.last().unwrap();
        assert_eq!(accept.caller_did, LOCAL);
        assert_eq!(accept.role_ceiling, fe_policy::RoleLevel::from("editor"), "a delegated token stays capped, never Owner");
    }
    assert_eq!(session.outbound.recv().await, Some(WsOut::Frame(WsData::Text("hello".into()))));
    // max_connections: the next upgrade is a 429; an out-of-scope petal is the uniform 404.
    assert_eq!(open_ingress_websocket(host.as_ref(), &local, true, PETAL, PACKAGE, "stream").unwrap_err().status, 429);
    assert_eq!(open_ingress_websocket(host.as_ref(), &local, false, PETAL, PACKAGE, "stream").unwrap_err(), ExtError::not_found());
    // An over-cap frame is dropped and counted while a small one is kept; closing frees the slot.
    host.websocket_frame(1, WsData::Text("x".repeat(WS_FRAME_CAP + 1)));
    host.websocket_frame(1, WsData::Text("ok".into()));
    host.websocket_closed(1);
    {
        let seen = host.seen.lock().unwrap();
        assert_eq!((seen.ws_dropped, seen.ws_kept.len()), (1, 1));
    }
    assert!(open_ingress_websocket(host.as_ref(), &local, true, PETAL, PACKAGE, "stream").is_ok());

    // Remote request gate: budget left 2 after the earlier remote calls, then 429 before any lookup (no existence oracle).
    let (first, second, third) = (
        extension_status(host.as_ref(), &remote, "inst-1").unwrap_err().status,
        extension_status(host.as_ref(), &remote, "nope").unwrap_err().status,
        extension_status(host.as_ref(), &remote, "inst-1").unwrap_err().status,
    );
    assert_eq!((first, second, third), (404, 404, 429));

    // P2.2 governance on a fresh host (its own remote budget).
    let host = FakeHost::new(100);
    let manager = ApiClaims { max_role: "manager".into(), ..claims(LOCAL) };
    let editor = ApiClaims { max_role: "editor".into(), ..claims(LOCAL) };

    // R19: each precondition's refusal reaches REST with the contract's status and code.
    for (code, status) in [
        ("host-off", 503),
        ("package-invalid", 422),
        ("untrusted-signer", 422),
        ("revoked", 409),
        ("entitlement-required", 403),
        ("policy-denied", 403),
        ("role-denied", 403),
        ("consent-missing", 409),
    ] {
        let refused = gui(host.as_ref(), op(AdminVerb::Enable, Some("inst-1"), json!({"refuse": code}))).unwrap_err();
        assert_eq!((refused.status, refused.code), (status, code));
    }

    // R18: no row reads as the default; an Editor is refused (one denial row), an out-of-scope caller sees not-found.
    assert_eq!(get_verse_policy(host.as_ref(), &editor, true, VERSE).await.unwrap()["policy"], Value::Null);
    let allow_list = json!({
        "mode": "allow-list",
        "allow": [
            {"package_id": PACKAGE, "versions": ">=0.1.0, <0.2.0"},
            {"package_id": "org.example.renamed", "versions": "*"}
        ],
        "deny": [DENIED_DIGEST]
    });
    let by_editor = put_verse_policy(host.as_ref(), &editor, true, VERSE, allow_list.clone()).await.unwrap_err();
    assert_eq!((by_editor.status, by_editor.code), (403, "role-denied"));
    assert_eq!(host.seen.lock().unwrap().denied, vec![(LOCAL.to_string(), format!("policy-change@{VERSE}"))]);
    assert_eq!(put_verse_policy(host.as_ref(), &manager, false, VERSE, allow_list.clone()).await.unwrap_err(), ExtError::not_found());
    // An allow entry carrying both a hash and a range fails the shape rules the DB applies.
    let both = json!({"mode": "allow-list", "allow": [{"package_id": PACKAGE, "versions": "*", "content_hash": DENIED_DIGEST}]});
    let malformed = put_verse_policy(host.as_ref(), &manager, true, VERSE, both).await.unwrap_err();
    assert_eq!((malformed.status, malformed.code), (422, "invalid-policy"));
    assert!(host.seen.lock().unwrap().policy_writers.is_empty());

    // Before any row the verse is first-party-only: a publisher-signed build is refused, a release one binds.
    let bind = |package: &str, version: &str, component: &str, trust: &str| {
        op(AdminVerb::Bind, Some("inst-1"), json!({
            "verse_id": VERSE, "package_id": package, "version": version,
            "content_hash": "a".repeat(64), "component_digest": component, "trust": trust,
        }))
    };
    let component = "b".repeat(64);
    let component = component.as_str();
    let first_party = gui(host.as_ref(), bind(PACKAGE, "0.1.5", component, "publisher")).unwrap_err();
    assert_eq!((first_party.status, first_party.code), (403, "policy-denied"));
    assert!(gui(host.as_ref(), bind(PACKAGE, "0.1.5", component, "release")).is_ok());

    // A Manager sets the allow-list; the host writes it (as its local DID) and binds are judged by it.
    put_verse_policy(host.as_ref(), &manager, true, VERSE, allow_list).await.unwrap();
    assert_eq!(get_verse_policy(host.as_ref(), &editor, true, VERSE).await.unwrap()["policy"]["mode"], "allow-list");
    assert!(gui(host.as_ref(), bind(PACKAGE, "0.1.5", component, "publisher")).is_ok());
    let out_of_range = gui(host.as_ref(), bind(PACKAGE, "0.2.0", component, "publisher")).unwrap_err();
    assert_eq!(out_of_range.code, "policy-denied");
    // governance.md J6: a denied component digest stays denied when the build is re-sealed under a new id.
    let renamed = gui(host.as_ref(), bind("org.example.renamed", "1.0.0", DENIED_DIGEST, "publisher")).unwrap_err();
    assert_eq!(renamed.code, "policy-denied");
    assert!(renamed.message.contains("deny-listed"), "{}", renamed.message);

    // The kill-list override is local-identity only: a remote DID gets the uniform not-found and one denial row.
    let override_params = json!({"verse_id": VERSE, "package_id": PACKAGE, "content_hash": "a".repeat(64), "confirm": PACKAGE});
    let remote_override = administer(host.as_ref(), &remote, op(AdminVerb::KillListOverride, None, override_params.clone())).unwrap_err();
    assert_eq!(remote_override, ExtError::not_found());
    assert_eq!(host.seen.lock().unwrap().denied.last().unwrap(), &(REMOTE.to_string(), "kill-list-override".to_string()));
    gui(host.as_ref(), op(AdminVerb::KillListOverride, None, override_params)).unwrap();
    assert_eq!(host.seen.lock().unwrap().admin.last().unwrap().1, AdminVerb::KillListOverride);
    let mismatch = gui(host.as_ref(), op(AdminVerb::KillListOverride, None, json!({"refuse": "confirmation-mismatch"}))).unwrap_err();
    assert_eq!((mismatch.status, mismatch.code), (409, "confirmation-mismatch"));
}

#[tokio::test]
async fn extension_stream_journey() {
    let host = FakeHost::new(100);
    let mut streams = ExtensionStreams::open(host.clone(), claims(REMOTE));

    // Host streams keep their own routes, and an unauthorized petal is a not-found.
    assert_eq!(streams.subscribe(true, PETAL, "host/readings", &json!({})).unwrap_err(), ExtError::not_found());
    assert_eq!(streams.subscribe(false, PETAL, "org.example.fleet-sim/positions", &json!({})).unwrap_err(), ExtError::not_found());

    let sub = streams.subscribe(true, PETAL, "org.example.fleet-sim/positions", &json!({})).unwrap();
    assert_eq!(sub, 42);
    assert_eq!(streams.drain(10), vec![r#"{"sub":42,"seq":1}"#.to_string()]);

    drop(streams);
    assert_eq!(host.seen.lock().unwrap().closed_clients, vec![7]);
}
