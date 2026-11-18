//! Operator-gated sync, field-clock, admission, and extension trace diagnostics
//! (`GET /api/v1/debug/sync-state`, `GET /api/v1/debug/extensions-trace`).

use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use fe_identity::api_token::ApiClaims;
use fe_runtime::extension_trace::ExtensionTraceBuffer;
use fe_runtime::peering::{PeeringCall, PeeringCallSender};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::auth::require_scope;
use crate::endpoint::is_addressable_node_id;
use crate::network_ops::NetworkOps;
use crate::operator_gate::{opt_in_value_enables, OperatorGate};
use crate::server::{ApiHosts, ApiState};
use crate::types::is_valid_ulid;

pub const DIAGNOSTICS_ALLOW_ENV: &str = "FE_API_DIAGNOSTICS_ALLOW";

pub const DIAGNOSTICS_GATE: OperatorGate = OperatorGate {
    opt_in_env: DIAGNOSTICS_ALLOW_ENV,
    endpoint: "sync & extension debug diagnostics",
    enabled_warning: "GET /api/v1/debug/sync-state and GET /api/v1/debug/extensions-trace \
                      return internal replica, field-clock, role-grant, and extension trace \
                      telemetry to Owner tokens",
    disabled_message: "debug diagnostics are disabled on this node (set FE_API_DIAGNOSTICS_ALLOW=1 or FE_API_INPUT_ALLOW=1)",
    rate_key_prefix: "debug_diagnostics",
    rate_per_sec: 5,
    rate_label: "5 debug diagnostics requests/sec",
    default_on: false,
};

pub fn diagnostics_allowed(state: &ApiState) -> bool {
    let explicit = std::env::var(DIAGNOSTICS_ALLOW_ENV).ok();
    opt_in_value_enables(explicit.as_deref(), DIAGNOSTICS_GATE.default_on) || state.input.allowed
}

#[derive(Debug, Default, Deserialize)]
pub struct SyncStateQuery {
    pub verse_id: Option<String>,
    pub node_id: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Debug, Default, Deserialize)]
pub struct ExtensionTraceQuery {
    pub package_id: Option<String>,
    pub petal_id: Option<String>,
    pub limit: Option<usize>,
}

fn error_response(status: StatusCode, msg: &str) -> Response {
    (
        status,
        Json(json!({ "ok": false, "data": null, "error": msg })),
    )
        .into_response()
}

fn peering_sender<'a>(
    hosts: Option<&'a ApiHosts>,
    ops: Option<&'a NetworkOps>,
) -> Option<&'a PeeringCallSender> {
    hosts
        .and_then(|h| h.peering.as_ref())
        .or_else(|| ops.and_then(|o| o.peering.as_ref()))
}

fn trace_buffer<'a>(
    hosts: Option<&'a ApiHosts>,
    ops: Option<&'a NetworkOps>,
) -> Option<&'a ExtensionTraceBuffer> {
    hosts
        .and_then(|h| h.extension_trace.as_ref())
        .or_else(|| ops.and_then(|o| o.extension_trace.as_ref()))
}

pub(crate) async fn get_sync_state_core(
    state: &ApiState,
    claims: &ApiClaims,
    sender: Option<&PeeringCallSender>,
    q: SyncStateQuery,
) -> Result<Value, (StatusCode, String)> {
    DIAGNOSTICS_GATE
        .admit(diagnostics_allowed(state), state, claims)
        .await
        .map_err(|r| (r.status(), r.message()))?;

    let scope_parts = fe_database::parse_scope(&claims.scope).map_err(|_| {
        (
            StatusCode::FORBIDDEN,
            "diagnostics require a verse-scoped token".to_owned(),
        )
    })?;

    let verse_id = q
        .verse_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| scope_parts.verse_id.clone());
    if !is_valid_ulid(&verse_id) {
        return Err((StatusCode::BAD_REQUEST, "invalid verse_id".to_owned()));
    }
    if require_scope(claims, &format!("VERSE#{verse_id}")).is_err() {
        return Err((
            StatusCode::NOT_FOUND,
            crate::rest::NOT_FOUND_OR_DENIED.to_owned(),
        ));
    }

    let node_id = q
        .node_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    if let Some(ref nid) = node_id {
        if !is_addressable_node_id(nid) {
            return Err((StatusCode::BAD_REQUEST, "invalid node_id".to_owned()));
        }
    }
    let limit = q.limit.unwrap_or(50).clamp(1, 200) as i64;

    let runtime_snapshot = if let Some(sender) = sender {
        let (tx, rx) = tokio::sync::oneshot::channel();
        if sender
            .try_send(PeeringCall::GetSyncState {
                verse_id: Some(verse_id.clone()),
                reply: tx,
            })
            .is_err()
        {
            return Err((
                StatusCode::SERVICE_UNAVAILABLE,
                "sync diagnostics bridge unavailable".to_owned(),
            ));
        }
        match tokio::time::timeout(Duration::from_secs(5), rx).await {
            Ok(Ok(snap)) => serde_json::to_value(snap).unwrap_or_default(),
            Ok(Err(_)) => {
                return Err((
                    StatusCode::SERVICE_UNAVAILABLE,
                    "sync diagnostics unavailable".to_owned(),
                ))
            }
            Err(_) => {
                return Err((
                    StatusCode::GATEWAY_TIMEOUT,
                    "sync diagnostics timed out".to_owned(),
                ))
            }
        }
    } else {
        json!(null)
    };

    let db_telemetry = if let Some(db) = &state.db_reader {
        read_db_sync_telemetry(db, &verse_id, node_id.as_deref(), limit).await
    } else {
        json!(null)
    };

    Ok(json!({
        "runtime": runtime_snapshot,
        "db": db_telemetry,
    }))
}

async fn read_db_sync_telemetry(
    db: &crate::db_reader::ReadDb,
    verse_id: &str,
    node_id: Option<&str>,
    limit: i64,
) -> Value {
    let mut role_grants: Vec<Value> = Vec::new();
    let mut pending_roles: Vec<Value> = Vec::new();
    let mut members: Vec<Value> = Vec::new();
    let mut row_field_versions: Vec<Value> = Vec::new();
    let mut node_logs: Vec<Value> = Vec::new();
    let mut op_logs: Vec<Value> = Vec::new();

    if let Ok(mut resp) = db
        .query(
            "SELECT statement_key, scope, peer_did, grantor_did, role, revoked, hlc, editor_since, manager_since, windows, updated_at FROM role_grant WHERE verse_id = $vid ORDER BY hlc DESC LIMIT $lim;\
             SELECT statement_key, scope, peer_did, grantor_did, role, revoked, hlc, received_at FROM role_statement_pending WHERE verse_id = $vid ORDER BY received_at DESC LIMIT $lim;\
             SELECT member_id, peer_did, status, joined_at FROM verse_member WHERE verse_id = $vid ORDER BY joined_at DESC LIMIT $lim;\
             SELECT tbl, rid, verse_id, field_key, author, hlc, deleted, claim_hlc FROM row_field_version WHERE verse_id = $vid ORDER BY hlc DESC LIMIT $lim;",
        )
        .bind(("vid", verse_id.to_owned()))
        .bind(("lim", limit))
        .await
        .and_then(|r| r.check().map_err(Into::into))
    {
        role_grants = resp.take(0).unwrap_or_default();
        pending_roles = resp.take(1).unwrap_or_default();
        members = resp.take(2).unwrap_or_default();
        row_field_versions = resp.take(3).unwrap_or_default();
    }

    if let Some(nid) = node_id {
        if let Ok(mut resp) = db
            .query(
                "SELECT tbl, rid, verse_id, field_key, author, hlc, value_json, deleted, claim_hlc FROM row_field_version WHERE tbl = 'node' AND rid = $nid AND verse_id = $vid ORDER BY field_key, hlc DESC LIMIT $lim;\
                 SELECT log_id, node_id, hlc_timestamp, op, source_did, row_version, created_at FROM node_log WHERE node_id = $nid AND node_id IN (SELECT VALUE node_id FROM node WHERE petal_id IN (SELECT VALUE petal_id FROM petal WHERE fractal_id IN (SELECT VALUE fractal_id FROM fractal WHERE verse_id = $vid))) ORDER BY row_version DESC LIMIT $lim;",
            )
            .bind(("nid", nid.to_owned()))
            .bind(("vid", verse_id.to_owned()))
            .bind(("lim", limit))
            .await
            .and_then(|r| r.check().map_err(Into::into))
        {
            row_field_versions = resp.take(0).unwrap_or_default();
            node_logs = resp.take(1).unwrap_or_default();
        }
    }

    let op_log_sql = if node_id.is_some() {
        "SELECT lamport_clock, hlc_timestamp, node_id, op_type FROM op_log WHERE node_id = $nid AND node_id IN (SELECT VALUE node_id FROM node WHERE petal_id IN (SELECT VALUE petal_id FROM petal WHERE fractal_id IN (SELECT VALUE fractal_id FROM fractal WHERE verse_id = $vid))) ORDER BY lamport_clock DESC LIMIT $lim;"
    } else {
        "SELECT lamport_clock, hlc_timestamp, node_id, op_type FROM op_log WHERE node_id IN (SELECT VALUE node_id FROM node WHERE petal_id IN (SELECT VALUE petal_id FROM petal WHERE fractal_id IN (SELECT VALUE fractal_id FROM fractal WHERE verse_id = $vid))) ORDER BY lamport_clock DESC LIMIT $lim;"
    };
    let mut op_query = db
        .query(op_log_sql)
        .bind(("vid", verse_id.to_owned()))
        .bind(("lim", limit));
    if let Some(nid) = node_id {
        op_query = op_query.bind(("nid", nid.to_owned()));
    }
    if let Ok(mut resp) = op_query.await.and_then(|r| r.check().map_err(Into::into)) {
        op_logs = resp.take(0).unwrap_or_default();
    }

    json!({
        "role_grants": role_grants,
        "role_statement_pending": pending_roles,
        "verse_members": members,
        "row_field_version": row_field_versions,
        "node_log": node_logs,
        "op_log": op_logs,
    })
}

pub async fn get_sync_state(
    State(state): State<Arc<ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    hosts: Option<Extension<ApiHosts>>,
    ops: Option<Extension<NetworkOps>>,
    Query(q): Query<SyncStateQuery>,
) -> Response {
    let sender = peering_sender(
        hosts.as_ref().map(|Extension(h)| h),
        ops.as_ref().map(|Extension(o)| o),
    );
    match get_sync_state_core(&state, &claims, sender, q).await {
        Ok(data) => Json(json!({ "ok": true, "data": data })).into_response(),
        Err((status, msg)) => error_response(status, &msg),
    }
}

pub(crate) async fn get_extensions_trace_core(
    state: &ApiState,
    claims: &ApiClaims,
    hosts: Option<&ApiHosts>,
    ops: Option<&NetworkOps>,
    q: ExtensionTraceQuery,
) -> Result<Value, (StatusCode, String)> {
    DIAGNOSTICS_GATE
        .admit(diagnostics_allowed(state), state, claims)
        .await
        .map_err(|r| (r.status(), r.message()))?;

    let scope_parts = fe_database::parse_scope(&claims.scope).map_err(|_| {
        (
            StatusCode::FORBIDDEN,
            "diagnostics require a verse-scoped token".to_owned(),
        )
    })?;

    let trace = trace_buffer(hosts, ops);
    let ext_host = hosts.and_then(|h| h.extension.as_ref());
    if trace.is_none() && ext_host.is_none() {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "extension trace unavailable on this host".to_owned(),
        ));
    }

    let requested_petal = q
        .petal_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let (allowed_petals, effective_petal): (std::collections::HashSet<String>, Option<&str>) =
        if let Some(ref token_petal) = scope_parts.petal_id {
            if let Some(req) = requested_petal {
                if req != token_petal {
                    return Err((
                        StatusCode::NOT_FOUND,
                        crate::rest::NOT_FOUND_OR_DENIED.to_owned(),
                    ));
                }
            }
            (
                std::iter::once(token_petal.clone()).collect(),
                Some(token_petal.as_str()),
            )
        } else {
            let petals: std::collections::HashSet<String> =
                crate::query_guard::resolve_scope_petals(state, &claims.scope)
                    .await
                    .map_err(|_| {
                        (
                            StatusCode::SERVICE_UNAVAILABLE,
                            "petal scope resolution unavailable".to_owned(),
                        )
                    })?
                    .into_iter()
                    .collect();
            if let Some(req) = requested_petal {
                if !petals.contains(req) {
                    return Err((
                        StatusCode::NOT_FOUND,
                        crate::rest::NOT_FOUND_OR_DENIED.to_owned(),
                    ));
                }
            }
            (petals, requested_petal)
        };

    let limit = q.limit.unwrap_or(100).clamp(1, 512);
    let mut snapshot = trace.map(|buf| {
        buf.snapshot(
            q.package_id.as_deref().map(str::trim).filter(|s| !s.is_empty()),
            effective_petal,
            limit,
        )
    });
    if let Some(ref mut snap) = snapshot {
        snap.events
            .retain(|ev| ev.petal_id.as_ref().is_none_or(|p| allowed_petals.contains(p)));
    }
    let mut installations = ext_host.map(|h| h.list()).unwrap_or_default();
    for entry in &mut installations {
        if let Some(instances) = entry.get_mut("instances").and_then(Value::as_array_mut) {
            instances.retain(|inst| {
                let verse_ok = inst
                    .get("verse_id")
                    .and_then(Value::as_str)
                    .is_some_and(|v| v == scope_parts.verse_id);
                let petal_ok = inst
                    .get("petal_id")
                    .and_then(Value::as_str)
                    .is_some_and(|p| allowed_petals.contains(p));
                verse_ok && petal_ok
            });
        }
    }
    Ok(json!({
        "trace": snapshot,
        "installations": installations,
    }))
}

pub async fn get_extensions_trace(
    State(state): State<Arc<ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    hosts: Option<Extension<ApiHosts>>,
    ops: Option<Extension<NetworkOps>>,
    Query(q): Query<ExtensionTraceQuery>,
) -> Response {
    match get_extensions_trace_core(
        &state,
        &claims,
        hosts.as_ref().map(|Extension(h)| h),
        ops.as_ref().map(|Extension(o)| o),
        q,
    )
    .await
    {
        Ok(data) => Json(json!({ "ok": true, "data": data })).into_response(),
        Err((status, msg)) => error_response(status, &msg),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fe_runtime::extension_trace::ExtensionTraceInput;
    use tokio::sync::broadcast;

    fn test_state_with_petals(allowed: bool, petals: Vec<&'static str>) -> Arc<ApiState> {
        let kp = fe_identity::NodeKeypair::generate();
        let (api_cmd_tx, rx) = crossbeam::channel::bounded::<fe_runtime::messages::ApiCommand>(8);
        std::thread::spawn(move || {
            while let Ok(cmd) = rx.recv() {
                if let fe_runtime::messages::ApiCommand::DbRequest {
                    cmd: fe_runtime::messages::DbCommand::RawQuery { correlation_id, .. },
                    reply_tx,
                } = cmd
                {
                    let _ = reply_tx.send(fe_runtime::messages::DbResult::QueryResult {
                        data: petals.iter().map(|p| json!(p)).collect(),
                        correlation_id,
                    });
                }
            }
        });
        let (tx_bcast, _) = broadcast::channel(8);
        let (ent_bcast, _) = broadcast::channel(8);
        let input = if allowed {
            crate::input::InputSurface::new(true, None)
        } else {
            crate::input::InputSurface::disabled()
        };
        Arc::new(ApiState {
            terrain_capture: Default::default(),
            api_cmd_tx,
            transform_broadcast_tx: tx_bcast,
            entity_change_tx: ent_bcast,
            verifying_key: kp.verifying_key(),
            revoked_jtis: Default::default(),
            blob_store: None,
            cors_origins: vec!["*".into()],
            db_reader: None,
            query_rate_limiter: tokio::sync::Mutex::new(Default::default()),
            entity_store: None,
            tileset_registry: None,
            hexon_registry: None,
            announcement_store: None,
            replication_tx: None,
            distributed_tx: None,
            sim_control_tx: None,
            share_signer: Arc::new(kp),
            elevated_query_allowed: false,
            upload_quota_bytes: 1024 * 1024,
            screenshot: crate::screenshot::ScreenshotSurface::disabled(),
            input,
        })
    }

    fn test_state(allowed: bool) -> Arc<ApiState> {
        test_state_with_petals(allowed, vec!["p1", "p2"])
    }

    fn claims(role: &str) -> ApiClaims {
        ApiClaims {
            sub: "did:key:zOwner".into(),
            scope: "VERSE#01J00000000000000000000000".into(),
            max_role: role.into(),
            exp: u64::MAX / 2,
            iat: 0,
            jti: "jti-debug".into(),
            token_type: "api".into(),
            client: None,
        }
    }

    #[tokio::test]
    async fn debug_endpoints_fail_closed_when_disabled_or_below_owner() {
        let off = test_state(false);
        let (status, _) = get_sync_state_core(&off, &claims("owner"), None, SyncStateQuery::default())
            .await
            .unwrap_err();
        assert_eq!(status, StatusCode::FORBIDDEN);

        let on = test_state(true);
        let (status, _) = get_sync_state_core(&on, &claims("manager"), None, SyncStateQuery::default())
            .await
            .unwrap_err();
        assert_eq!(status, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn extensions_trace_returns_filtered_ring_when_opted_in() {
        let on = test_state(true);
        let buf = ExtensionTraceBuffer::with_capacity(16);
        buf.record(ExtensionTraceInput {
            kind: "command".into(),
            package_id: "org.example.alpha".into(),
            petal_id: Some("p1".into()),
            hook: None,
            key: Some("ping".into()),
            outcome: "ok".into(),
            detail: None,
            duration_ms: Some(3),
        });
        buf.record(ExtensionTraceInput {
            kind: "hook".into(),
            package_id: "org.example.beta".into(),
            petal_id: Some("p2".into()),
            hook: Some("scene.create".into()),
            key: None,
            outcome: "refuse".into(),
            detail: Some("policy".into()),
            duration_ms: Some(2),
        });
        let hosts = ApiHosts {
            extension_trace: Some(buf),
            ..Default::default()
        };
        let res = get_extensions_trace_core(
            &on,
            &claims("owner"),
            Some(&hosts),
            None,
            ExtensionTraceQuery {
                package_id: Some("org.example.beta".into()),
                petal_id: None,
                limit: Some(10),
            },
        )
        .await
        .expect("trace ok");
        assert_eq!(res["trace"]["events"].as_array().unwrap().len(), 1);
        assert_eq!(res["trace"]["events"][0]["outcome"], "refuse");
    }

    #[tokio::test]
    async fn debug_endpoints_require_verse_scope_and_reject_foreign_verse_or_petal() {
        let on = test_state(true);
        let mut global_claims = claims("owner");
        global_claims.scope = "global".into();
        let (status, _) =
            get_sync_state_core(&on, &global_claims, None, SyncStateQuery::default())
                .await
                .unwrap_err();
        assert_eq!(status, StatusCode::FORBIDDEN);

        // Omitted verse_id defaults to the caller's token verse_id; requesting a foreign verse_id is 404.
        let foreign_vid = ulid::Ulid::new().to_string();
        let (status, _) = get_sync_state_core(
            &on,
            &claims("owner"),
            None,
            SyncStateQuery {
                verse_id: Some(foreign_vid),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
        assert_eq!(status, StatusCode::NOT_FOUND);

        let hosts = ApiHosts {
            extension_trace: Some(ExtensionTraceBuffer::with_capacity(8)),
            ..Default::default()
        };
        let mut petal_claims = claims("owner");
        petal_claims.scope = "VERSE#01J00000000000000000000000-FRACTAL#f1-PETAL#p1".into();
        let (status, _) = get_extensions_trace_core(
            &on,
            &petal_claims,
            Some(&hosts),
            None,
            ExtensionTraceQuery {
                petal_id: Some("p2".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
        assert_eq!(status, StatusCode::NOT_FOUND);

        let mut sub_verse_claims = claims("owner");
        sub_verse_claims.scope = "VERSE#01J00000000000000000000000-FRACTAL#f1".into();
        let (status, _) =
            get_sync_state_core(&on, &sub_verse_claims, None, SyncStateQuery::default())
                .await
                .unwrap_err();
        assert_eq!(status, StatusCode::NOT_FOUND, "FRACTAL# token cannot escalate to verse-wide sync-state");

        // When db_reader is None, VERSE# claims still resolve petals via api_cmd_tx and reject foreign petal_id.
        let one_petal = test_state_with_petals(true, vec!["p1"]);
        let (status, _) = get_extensions_trace_core(
            &one_petal,
            &claims("owner"),
            Some(&hosts),
            None,
            ExtensionTraceQuery {
                petal_id: Some("p2".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
        assert_eq!(status, StatusCode::NOT_FOUND);

        let res = get_sync_state_core(&on, &claims("owner"), None, SyncStateQuery::default())
            .await
            .expect("sync-state with omitted verse_id succeeds for valid verse token");
        assert!(res.get("runtime").is_some());
    }
}
