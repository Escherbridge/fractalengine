//! J6: the extensions API surface (spec R11/R12/R15) through the shared core functions REST, MCP and
//! `/ws` all call, against a recording stand-in for the plugin host (the real broker is proven in
//! fe-plugin `bus/tests.rs`). Asserts what callers observe: statuses, bodies, and what reached the host.

use std::sync::{Arc, Mutex};

use fe_api::extensions::{
    administer, caller_of, command, extension_audit, extension_command_tool, extension_status,
    ingress, list_extensions, AdminOp, AdminRefusal, AdminVerb, ApiCaller, ApiCallerKind,
    CommandFailure, CommandReply, ExtError, ExtensionHost, ExtensionStreams, HostRefusal,
    IngressAnswer, IngressCall, IngressRefusal, IngressRequestParts,
};
use fe_identity::api_token::ApiClaims;
use serde_json::{json, Value};
use tokio::sync::{oneshot, Notify};

const LOCAL: &str = "did:key:z6MkLocalHost";
const REMOTE: &str = "did:key:z6MkRemoteEditor";
const PETAL: &str = "petal-1";
const PACKAGE: &str = "org.example.fleet-sim";

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
        self.seen.lock().unwrap().admin.push((actor_did.into(), op.verb, op.installation.clone()));
        if op.verb == AdminVerb::Consent && op.params["digest"] != "good" {
            return Err(AdminRefusal { code: "consent-mismatch".into(), message: "stale digest".into() });
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

#[tokio::test]
async fn extensions_api_journey() {
    let host = FakeHost::new(4);
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
    // Local administration runs; a precondition refusal maps to its suggested status.
    assert_eq!(administer(host.as_ref(), &local, op(AdminVerb::Consent, Some("inst-1"), json!({"digest": "good"}))).unwrap()["done"], "consent");
    let stale = administer(host.as_ref(), &local, op(AdminVerb::Consent, Some("inst-1"), json!({"digest": "old"}))).unwrap_err();
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

    // Ingress: credential headers are stripped by the host whatever the caller sent; the body ceiling is 413.
    let sent = vec![("Authorization", "Bearer secret"), ("cookie", "s=1"), ("x-api-key", "k"), ("x-fe-join-lease", "l"), ("x-device-id", "d1")];
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

    // Remote request gate: budget left 2 after the earlier remote calls, then 429 before any lookup (no existence oracle).
    let (first, second, third) = (
        extension_status(host.as_ref(), &remote, "inst-1").unwrap_err().status,
        extension_status(host.as_ref(), &remote, "nope").unwrap_err().status,
        extension_status(host.as_ref(), &remote, "inst-1").unwrap_err().status,
    );
    assert_eq!((first, second, third), (404, 404, 429));
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
