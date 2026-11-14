//! `GET /api/v1/network/diagnostics`: scope-confined, address-redacted network posture (see `fe-api/AGENTS.md` §network-diagnostics).

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use fe_identity::api_token::ApiClaims;
use fe_runtime::net_diagnostics::{
    redact, redact_host, redact_relay_url, NetworkDiagnostics, NetworkDiagnosticsCall,
    NetworkDiagnosticsCallSender,
};

use crate::auth::{require_role_and_scope, require_role_level};
use crate::network_ops::NetworkOps;
use crate::operator_gate::{opt_in_value_enables, OperatorGate};
use crate::server::ApiState;

/// Env var that lets an Owner token see unredacted peer addresses.
pub const NETWORK_DETAIL_ENV: &str = "FE_API_NETWORK_DETAIL";

/// How long a request waits for the sync thread before answering 504.
pub const REPLY_TIMEOUT: Duration = Duration::from_secs(2);

/// Gate for unredacted addresses: opt-in off unless `=1`, Owner only; also carries the per-DID rate.
pub const NETWORK_DETAIL_GATE: OperatorGate = OperatorGate {
    opt_in_env: NETWORK_DETAIL_ENV,
    endpoint: "network diagnostics detail",
    enabled_warning: "GET /api/v1/network/diagnostics and the MCP `get_network_diagnostics` tool \
                      return peers' full IP addresses to Owner tokens; leave unset unless you \
                      are debugging connectivity on this node",
    disabled_message: "network diagnostics detail is disabled on this node",
    rate_key_prefix: "network_diagnostics",
    rate_per_sec: 2,
    rate_label: "2 network diagnostics requests/sec",
    default_on: false,
};

/// Why a diagnostics request produced no snapshot; maps to 503 / 504.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticsError {
    /// No diagnostics bridge is wired, or the sync thread dropped the request.
    Unavailable,
    /// The sync thread did not answer within the reply timeout.
    TimedOut,
}

impl DiagnosticsError {
    /// HTTP status for this failure.
    pub fn status(self) -> StatusCode {
        match self {
            Self::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
            Self::TimedOut => StatusCode::GATEWAY_TIMEOUT,
        }
    }

    /// Client-facing text (REST error body, MCP tool error).
    pub fn message(self) -> &'static str {
        match self {
            Self::Unavailable => "network diagnostics unavailable",
            Self::TimedOut => "network diagnostics timed out",
        }
    }
}

/// True when this caller may see full addresses: operator opted in AND the token is Owner.
pub fn detail_allowed(claims: &ApiClaims, detail_enabled: bool) -> bool {
    detail_enabled && require_role_level(claims, fe_database::RoleLevel::Owner).is_ok()
}

/// Reads the detail opt-in without logging (default off; only `1` enables).
pub fn detail_enabled_from_env() -> bool {
    let value = std::env::var(NETWORK_DETAIL_ENV).ok();
    opt_in_value_enables(value.as_deref(), NETWORK_DETAIL_GATE.default_on)
}

/// Keeps only peers and hazard-B verses the token reads at Viewer+ (each verse judged once), and redacts addresses and IP-literal relay hosts unless `detail`.
pub fn confine_to_claims(mut snapshot: NetworkDiagnostics, claims: &ApiClaims, detail: bool) -> NetworkDiagnostics {
    let mut judged: BTreeMap<String, bool> = BTreeMap::new();
    let mut readable = |verse_id: &str| -> bool {
        if let Some(allowed) = judged.get(verse_id) {
            return *allowed;
        }
        let allowed = require_role_and_scope(claims, "viewer", &format!("VERSE#{verse_id}")).is_ok();
        judged.insert(verse_id.to_owned(), allowed);
        allowed
    };
    snapshot.peers.retain_mut(|peer| {
        peer.verse_ids.retain(|verse_id| readable(verse_id));
        !peer.verse_ids.is_empty()
    });
    snapshot.node.unadvertised_home_verses.retain(|verse_id| readable(verse_id));
    if !detail {
        // Defense in depth: the sync side already redacts, but a raw address must never leave this seam.
        for peer in &mut snapshot.peers {
            for address in &mut peer.addrs {
                if let Ok(parsed) = address.parse::<SocketAddr>() {
                    *address = redact(&parsed);
                }
            }
            if let Some(host) = peer.relay_host.as_mut() {
                *host = redact_host(host, false);
            }
        }
        if let Some(home_relay) = snapshot.node.home_relay.as_mut() {
            *home_relay = redact_relay_url(home_relay, false);
        }
    }
    snapshot
}

/// Asks the sync thread for a snapshot and confines it to the caller; shared by REST and MCP.
pub async fn collect_scoped_diagnostics(
    sender: Option<&NetworkDiagnosticsCallSender>,
    claims: &ApiClaims,
    detail_enabled: bool,
    reply_timeout: Duration,
) -> Result<NetworkDiagnostics, DiagnosticsError> {
    let sender = sender.ok_or(DiagnosticsError::Unavailable)?;
    let detail = detail_allowed(claims, detail_enabled);
    let (reply, receiver) = tokio::sync::oneshot::channel();
    if let Err(error) = sender.send(NetworkDiagnosticsCall { detail, reply }) {
        tracing::warn!(%error, "network diagnostics bridge is gone; request dropped");
        return Err(DiagnosticsError::Unavailable);
    }
    match tokio::time::timeout(reply_timeout, receiver).await {
        Ok(Ok(snapshot)) => Ok(confine_to_claims(snapshot, claims, detail)),
        Ok(Err(_)) => {
            tracing::warn!("network diagnostics reply dropped by the sync thread");
            Err(DiagnosticsError::Unavailable)
        }
        Err(_) => Err(DiagnosticsError::TimedOut),
    }
}

fn error_response(status: StatusCode, message: &str) -> Response {
    (status, Json(serde_json::json!({ "ok": false, "error": message }))).into_response()
}

/// `GET /api/v1/network/diagnostics`: Viewer+; peers limited to verses the token covers.
pub async fn get_network_diagnostics(
    State(state): State<Arc<ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    network_ops: Option<Extension<NetworkOps>>,
) -> Response {
    if require_role_level(&claims, fe_database::RoleLevel::Viewer).is_err() {
        return error_response(StatusCode::FORBIDDEN, "insufficient permissions");
    }
    if let Err(refusal) = NETWORK_DETAIL_GATE.check_rate(&state, &claims).await {
        return error_response(refusal.status(), &refusal.message());
    }
    let sender = network_ops.and_then(|Extension(ops)| ops.network_diagnostics);
    match collect_scoped_diagnostics(sender.as_ref(), &claims, detail_enabled_from_env(), REPLY_TIMEOUT).await {
        Ok(snapshot) => Json(snapshot).into_response(),
        Err(error) => error_response(error.status(), error.message()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fe_runtime::net_diagnostics::{NodeDiagnostics, PeerConnectionKind, PeerDiagnostics};
    use std::collections::HashSet;
    use tower::ServiceExt;

    fn test_state() -> Arc<ApiState> {
        let (api_cmd_tx, _) = crossbeam::channel::bounded(1);
        let (transform_broadcast_tx, _) = tokio::sync::broadcast::channel(1);
        let (entity_change_tx, _) = tokio::sync::broadcast::channel(1);
        Arc::new(ApiState {
            api_cmd_tx,
            transform_broadcast_tx,
            entity_change_tx,
            verifying_key: ed25519_dalek::VerifyingKey::from_bytes(&[0u8; 32]).unwrap(),
            revoked_jtis: Arc::new(tokio::sync::RwLock::new(HashSet::new())),
            blob_store: None,
            cors_origins: vec![],
            db_reader: None,
            terrain_capture: Default::default(),
            query_rate_limiter: tokio::sync::Mutex::new(std::collections::HashMap::new()),
            entity_store: None,
            tileset_registry: None,
            hexon_registry: None,
            announcement_store: None,
            replication_tx: None,
            distributed_tx: None,
            sim_control_tx: None,
            elevated_query_allowed: false,
            upload_quota_bytes: crate::upload::DEFAULT_UPLOAD_QUOTA_BYTES,
            screenshot: crate::screenshot::ScreenshotSurface::disabled(),
            input: crate::input::InputSurface::disabled(),
            share_signer: Arc::new(fe_identity::NodeKeypair::generate()),
        })
    }

    fn claims(scope: &str, role: &str) -> ApiClaims {
        ApiClaims {
            sub: "did:key:z6MkNetDiag".to_string(),
            scope: scope.to_string(),
            max_role: role.to_string(),
            token_type: "api".to_string(),
            iat: 0,
            exp: u64::MAX,
            jti: "jti-net-diag".to_string(),
            client: None,
        }
    }

    fn peer(did: &str, verses: &[&str], addr: &str) -> PeerDiagnostics {
        PeerDiagnostics {
            peer_did: did.to_string(),
            verse_ids: verses.iter().map(|verse| verse.to_string()).collect(),
            kind: PeerConnectionKind::Direct,
            relay_host: None,
            addrs: vec![addr.to_string()],
        }
    }

    /// Fake sync-thread edge: answers every call with a fixture that leaks raw addresses and reports the `detail` flag it saw.
    fn spawn_fake_bridge() -> (NetworkDiagnosticsCallSender, std::sync::mpsc::Receiver<bool>) {
        let (sender, receiver) = crossbeam::channel::unbounded::<NetworkDiagnosticsCall>();
        let (seen_tx, seen_rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            while let Ok(call) = receiver.recv() {
                let _ = seen_tx.send(call.detail);
                let mut visible = peer("did:key:zVisible", &["v1"], "203.0.113.7:51820");
                visible.relay_host = Some("198.51.100.77".to_string());
                let fixture = NetworkDiagnostics {
                    node: NodeDiagnostics {
                        home_relay: Some("https://203.0.113.50:3340/".to_string()),
                        unadvertised_home_verses: vec!["v1".to_string(), "v2".to_string()],
                        ..NodeDiagnostics::default()
                    },
                    peers: vec![
                        visible,
                        peer("did:key:zHidden", &["v2"], "198.51.100.9:51820"),
                        peer("did:key:zShared", &["v1", "v2"], "192.0.2.4:443"),
                    ],
                };
                let _ = call.reply.send(fixture);
            }
        });
        (sender, seen_rx)
    }

    async fn get_body(router: axum::Router) -> (StatusCode, String) {
        let request = axum::http::Request::builder()
            .uri("/api/v1/network/diagnostics")
            .body(axum::body::Body::empty())
            .expect("request");
        let response = router.oneshot(request).await.expect("response");
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 1 << 20).await.expect("body");
        (status, String::from_utf8_lossy(&bytes).to_string())
    }

    fn router_for(claims: ApiClaims, ops: NetworkOps) -> axum::Router {
        axum::Router::new()
            .route("/api/v1/network/diagnostics", axum::routing::get(get_network_diagnostics))
            .layer(Extension(claims))
            .layer(Extension(ops))
            .with_state(test_state())
    }

    #[tokio::test]
    async fn api_diagnostics_hides_peers_from_unreadable_verses_and_redacts_ips() {
        // No bridge wired: 503.
        let (status, _) = get_body(router_for(claims("VERSE#v1", "viewer"), NetworkOps::default())).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);

        // Viewer scoped to v1: the v2-only peer is hidden, the shared peer is trimmed to v1, no raw IP leaves the API.
        let (sender, seen) = spawn_fake_bridge();
        let ops = NetworkOps { network_diagnostics: Some(sender.clone()), ..NetworkOps::default() };
        let (status, body) = get_body(router_for(claims("VERSE#v1", "viewer"), ops)).await;
        assert_eq!(status, StatusCode::OK);
        let snapshot: NetworkDiagnostics = serde_json::from_str(&body).expect("snapshot json");
        let dids: Vec<&str> = snapshot.peers.iter().map(|peer| peer.peer_did.as_str()).collect();
        assert_eq!(dids, vec!["did:key:zVisible", "did:key:zShared"]);
        assert_eq!(snapshot.peers[1].verse_ids, vec!["v1".to_string()]);
        assert_eq!(snapshot.peers[0].addrs, vec!["ipv4:ephemeral".to_string()]);
        assert_eq!(snapshot.peers[0].relay_host.as_deref(), Some("ipv4-literal"), "a peer-run relay IP stays hidden");
        assert_eq!(snapshot.node.home_relay.as_deref(), Some("https://ipv4-literal"));
        assert_eq!(snapshot.node.unadvertised_home_verses, vec!["v1".to_string()], "hazard-B list confined to readable verses");
        assert!(!body.contains("203.0.113") && !body.contains("198.51.100") && !body.contains("192.0.2"));
        assert!(!body.contains("v2"), "an unreadable verse id never leaves the API");
        assert!(!seen.recv().expect("call seen"), "sync side must be asked for redacted addresses");

        // Owner without the operator opt-in stays redacted; with it, sees full addresses of readable peers only.
        let owner = claims("VERSE#v1", "owner");
        let off = collect_scoped_diagnostics(Some(&sender), &owner, false, REPLY_TIMEOUT).await.expect("off");
        assert_eq!(off.peers[0].addrs, vec!["ipv4:ephemeral".to_string()]);
        assert!(!seen.recv().expect("call seen"));
        let on = collect_scoped_diagnostics(Some(&sender), &owner, true, REPLY_TIMEOUT).await.expect("on");
        assert_eq!(on.peers[0].addrs, vec!["203.0.113.7:51820".to_string()]);
        assert_eq!(on.peers.len(), 2);
        assert!(seen.recv().expect("call seen"));

        // A viewer never gets detail even when the operator opted in.
        let viewer = claims("VERSE#v1", "viewer");
        let viewer_on = collect_scoped_diagnostics(Some(&sender), &viewer, true, REPLY_TIMEOUT).await.expect("viewer");
        assert_eq!(viewer_on.peers[0].addrs, vec!["ipv4:ephemeral".to_string()]);
        assert!(!seen.recv().expect("call seen"));
    }
}
