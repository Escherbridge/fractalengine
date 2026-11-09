//! BND-8: the bindings API (docs/extensions/bindings.md §9) through the shared core functions REST and MCP call,
//! against a recording stand-in for the binary's binding plane. Asserts what callers observe: statuses, bodies,
//! and what reached the host. The DB plane's own judging is fe-database `extension_binding_test`.

use std::sync::{Arc, Mutex};

use fe_api::bindings::{
    clear_binding, effective_bindings, list_bindings, set_binding, BindingHost, BindingListReply, BindingTarget,
    BindingWriteReply, ClearBindingRequest, EffectivePoint, SetBindingRequest,
};
use fe_identity::api_token::ApiClaims;
use fe_runtime::extension_binding::{BindingMode, BindingRecord, BindingResolution, UnresolvedReason};
use fe_runtime::messages::{BindingRefusal, StoredBinding};
use serde_json::json;
use tokio::sync::oneshot;

const LOCAL: &str = "did:key:z6MkLocalHost";
const REMOTE: &str = "did:key:z6MkRemoteManager";
const SCOPE: &str = "VERSE#v1-FRACTAL#f1-PETAL#p1";

fn claims(sub: &str, max_role: &str, jti: &str) -> ApiClaims {
    ApiClaims {
        sub: sub.to_string(),
        token_type: "api".into(),
        scope: "VERSE#v1".into(),
        max_role: max_role.into(),
        iat: 0,
        exp: u64::MAX,
        jti: jti.into(),
        client: None,
    }
}

#[derive(Default)]
struct Seen {
    puts: Vec<(String, BindingRecord)>,
    lists: Vec<(String, String)>,
    stored: Vec<StoredBinding>,
    /// A scripted refusal the next put answers with.
    refuse_put: Option<BindingRefusal>,
    remote_budget: u32,
}

struct FakeBindings {
    seen: Mutex<Seen>,
}

impl FakeBindings {
    fn new(remote_budget: u32) -> Arc<Self> {
        Arc::new(Self { seen: Mutex::new(Seen { remote_budget, ..Seen::default() }) })
    }
}

impl BindingHost for FakeBindings {
    fn local_did(&self) -> String {
        LOCAL.into()
    }
    fn admit_remote(&self, _did: &str) -> bool {
        let mut seen = self.seen.lock().unwrap();
        let ok = seen.remote_budget > 0;
        seen.remote_budget = seen.remote_budget.saturating_sub(1);
        ok
    }
    fn put_binding(&self, caller_did: &str, template: BindingRecord) -> BindingWriteReply {
        let (tx, rx) = oneshot::channel();
        let mut seen = self.seen.lock().unwrap();
        seen.puts.push((caller_did.into(), template.clone()));
        if let Some(refusal) = seen.refuse_put.take() {
            let _ = tx.send(Err(refusal));
            return rx;
        }
        // The DB plane stamps the clock and keeps the row; `inherit` clears (never a delete).
        let mut stored = template;
        stored.written_at = format!("{}", 1_000 + seen.puts.len());
        seen.stored.retain(|v| !(v.record.scope == stored.scope && v.record.hook == stored.hook && v.record.key == stored.key));
        seen.stored.push(StoredBinding {
            admissible: true,
            written_authorized: true,
            effective: stored.mode != BindingMode::Inherit,
            record: stored.clone(),
        });
        let _ = tx.send(Ok(stored));
        rx
    }
    fn list_bindings(&self, caller_did: &str, scope: &str) -> BindingListReply {
        let (tx, rx) = oneshot::channel();
        let mut seen = self.seen.lock().unwrap();
        seen.lists.push((caller_did.into(), scope.into()));
        let rows = seen.stored.iter().filter(|v| v.record.scope == scope).cloned().collect();
        let _ = tx.send(Ok(rows));
        rx
    }
    fn effective(&self, petal_scope: &str, _caller_did: &str) -> Option<Vec<EffectivePoint>> {
        (petal_scope == SCOPE).then(|| {
            vec![EffectivePoint {
                hook: "petal.map".into(),
                key: String::new(),
                scope: "VERSE#v1-FRACTAL#f1".into(),
                mode: BindingMode::Extension,
                package_id: Some("org.example.city-map".into()),
                version_req: Some(">=1.0.0".into()),
                resolution: Some(BindingResolution::unresolved(
                    "org.example.city-map",
                    "VERSE#v1-FRACTAL#f1",
                    UnresolvedReason::NotInstalled,
                )),
            }]
        })
    }
}

fn target() -> BindingTarget {
    BindingTarget { verse_id: "v1".into(), fractal_id: Some("f1".into()), petal_id: Some("p1".into()) }
}

fn set_request() -> SetBindingRequest {
    serde_json::from_value(json!({
        "verse_id": "v1", "fractal_id": "f1", "petal_id": "p1",
        "hook": "petal.map", "mode": "extension",
        "package_id": "org.example.city-map", "version_req": ">=1.0.0",
        "config": { "district": "harbour" }
    }))
    .expect("request body parses with flattened target")
}

fn clear_request() -> ClearBindingRequest {
    ClearBindingRequest { target: target(), hook: "petal.map".into(), key: String::new() }
}

#[tokio::test]
async fn bindings_api_journey() {
    let host = FakeBindings::new(10);
    let manager = claims(REMOTE, "manager", "t-manager");
    let viewer = claims(REMOTE, "viewer", "t-viewer");
    let local = claims(LOCAL, "owner", "t-local");
    let scope = || Some(SCOPE.to_string());

    // PUT: a Manager token writes; the host sees the token's DID, the scope and the full record.
    let stored = set_binding(host.as_ref(), &manager, scope(), set_request()).await.unwrap();
    assert_eq!(stored["package_id"], "org.example.city-map");
    assert_eq!(stored["config"]["district"], "harbour");
    {
        let seen = host.seen.lock().unwrap();
        let (caller, template) = &seen.puts[0];
        assert_eq!(caller, REMOTE);
        assert_eq!((template.scope.as_str(), template.hook.as_str(), template.mode), (SCOPE, "petal.map", BindingMode::Extension));
    }

    // GET at the exact scope lists the version with its judging flags (Viewer+ suffices).
    let listed = list_bindings(host.as_ref(), &viewer, scope()).await.unwrap();
    assert_eq!(listed["scope"], SCOPE);
    assert_eq!(listed["bindings"][0]["hook"], "petal.map");
    assert_eq!(listed["bindings"][0]["effective"], true);
    assert_eq!(listed["bindings"][0]["admissible"], true);

    // A Viewer ceiling cannot write: 403 role-denied, and nothing reaches the host.
    let puts_before = host.seen.lock().unwrap().puts.len();
    let denied = set_binding(host.as_ref(), &viewer, scope(), set_request()).await.unwrap_err();
    assert_eq!((denied.status, denied.code), (403, "role-denied"));
    assert_eq!(host.seen.lock().unwrap().puts.len(), puts_before);

    // DELETE writes `inherit` with no package, and the version stops being effective.
    clear_binding(host.as_ref(), &manager, scope(), clear_request()).await.unwrap();
    {
        let seen = host.seen.lock().unwrap();
        let (_, template) = seen.puts.last().unwrap();
        assert_eq!((template.mode, template.package_id.as_deref()), (BindingMode::Inherit, None));
    }
    let after_clear = list_bindings(host.as_ref(), &local, scope()).await.unwrap();
    assert_eq!(after_clear["bindings"][0]["effective"], false);

    // Effective view: the local DID gets this host's state and its one offer; a remote caller gets neither.
    let local_view = effective_bindings(host.as_ref(), &local, scope()).await.unwrap();
    assert_eq!(local_view["points"][0]["resolution"]["state"], "unresolved");
    assert_eq!(local_view["points"][0]["resolution"]["offer"], "install");
    let remote_view = effective_bindings(host.as_ref(), &viewer, scope()).await.unwrap();
    assert_eq!(remote_view["points"][0]["package_id"], "org.example.city-map");
    assert!(remote_view["points"][0].get("resolution").is_none(), "remote callers never see host state: {remote_view}");
    // A delegated token of this node (node DID, below Owner) is remote too.
    let delegated = claims(LOCAL, "manager", "t-delegated");
    assert!(effective_bindings(host.as_ref(), &delegated, scope()).await.unwrap()["points"][0].get("resolution").is_none());
}

#[tokio::test]
async fn bindings_api_refusals() {
    let host = FakeBindings::new(100);
    let manager = claims(REMOTE, "manager", "t-manager");

    // Uniform not-found: out-of-scope/unknown (None) is the same error on every route, with no role detail.
    let unknown_list = list_bindings(host.as_ref(), &manager, None).await.unwrap_err();
    let unknown_put = set_binding(host.as_ref(), &claims(REMOTE, "viewer", "t-v"), None, set_request()).await.unwrap_err();
    assert_eq!(unknown_list, unknown_put);
    assert_eq!(unknown_list.status, 404);
    assert_eq!(effective_bindings(host.as_ref(), &manager, Some("VERSE#other".into())).await.unwrap_err().status, 404);

    // The remote rate gate answers before any lookup: with a budget of two, the third request is 429.
    let metered = FakeBindings::new(2);
    assert!(list_bindings(metered.as_ref(), &manager, Some(SCOPE.into())).await.is_ok());
    assert_eq!(list_bindings(metered.as_ref(), &manager, None).await.unwrap_err().status, 404);
    assert_eq!(list_bindings(metered.as_ref(), &manager, None).await.unwrap_err().status, 429);
    assert!(metered.seen.lock().unwrap().lists.len() == 1, "a gated request never reaches the plane");

    // Stateless shape failures are 422 and never reach the host; the DB plane's codes map one-to-one.
    let mut bad_runs_on = set_request();
    bad_runs_on.runs_on = Some("not-a-did".into());
    assert_eq!(set_binding(host.as_ref(), &manager, Some(SCOPE.into()), bad_runs_on).await.unwrap_err().code, "invalid-binding");
    let mut builtin_with_package = set_request();
    builtin_with_package.mode = BindingMode::Builtin;
    assert_eq!(
        set_binding(host.as_ref(), &manager, Some(SCOPE.into()), builtin_with_package).await.unwrap_err().status,
        422
    );
    assert!(host.seen.lock().unwrap().puts.is_empty());

    for (code, status) in [("unknown-hook", 422), ("role-denied", 403), ("policy-denied", 403), ("not-found", 404), ("failed", 502)] {
        host.seen.lock().unwrap().refuse_put = Some(BindingRefusal { code: code.into(), message: format!("scripted {code}") });
        let err = set_binding(host.as_ref(), &manager, Some(SCOPE.into()), set_request()).await.unwrap_err();
        assert_eq!(err.status, status, "{code}");
        if code == "not-found" {
            assert_eq!(err, fe_api::extensions::ExtError::not_found(), "the DB plane's 404 carries no detail");
        }
    }

}
