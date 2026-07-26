//! Per-endpoint read/write API surface (T5, track `endpoint_api_surface_20260725`).
//!
//! Every object is a stable `fe://verse/fractal/petal/node` endpoint you can
//! GET (its data + type-specific payload) and mutate through T1's sync-safe ops
//! (tombstone / cascade / promote), authorized by fe-policy. See
//! `fe-api/AGENTS.md` §endpoint-surface for the URI scheme, auth model, and the
//! generic-node type-tag contract this reads T2/T3 data through.

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::response::IntoResponse;
use axum::{Extension, Json};
use fe_entity_store::NodeAddress;
use fe_identity::api_token::ApiClaims;
use fe_runtime::messages::{ApiCommand, CallerAuth, DbCommand, DbResult};
use serde::{Deserialize, Serialize};

use crate::auth::{require_role, require_scope};
use crate::rest::{resolve_node_scope, resolve_petal_scope};
use crate::server::ApiState;
use crate::types::{is_valid_ulid, ApiResponse};

// ---------------------------------------------------------------------------
// Addressable node ids
// ---------------------------------------------------------------------------

/// True if `s` is a plain node ULID or a promoted-stamp instance id
/// (`<ulid>#inst-<n>`, D-A5 / T1 lazy promotion). Reads/writes accept both;
/// the free-text `#`/`-`/digits after the base ULID identify the instance.
pub fn is_addressable_node_id(s: &str) -> bool {
    if is_valid_ulid(s) {
        return true;
    }
    match s.split_once('#') {
        Some((base, inst)) => {
            is_valid_ulid(base)
                && inst
                    .strip_prefix("inst-")
                    .is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
        }
        None => false,
    }
}

/// Derive the caller identity forwarded into a sync-safe lifecycle op. The role
/// string is the token's already-resolved ceiling (validated upstream at mint);
/// the DB thread re-checks Editor+ at the node's scope via fe-policy (N-5). API
/// callers are `Identified` — never `Local` (that is the desktop UI's).
pub(crate) fn caller_auth(claims: &ApiClaims) -> CallerAuth {
    CallerAuth::Identified {
        did: claims.sub.clone(),
        role: claims.max_role.to_lowercase(),
    }
}

// ---------------------------------------------------------------------------
// FR-1: public endpoint addressing
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct AddressDto {
    pub node_id: String,
    pub address: String,
    pub scope: String,
    pub verse_id: String,
    pub fractal_id: String,
    pub petal_id: String,
}

impl AddressDto {
    fn from_address(addr: &NodeAddress) -> Self {
        Self {
            node_id: addr.node_id().to_string(),
            address: addr.to_uri(),
            scope: addr.scope(),
            verse_id: addr.verse_id.clone(),
            fractal_id: addr.fractal_id.clone(),
            petal_id: addr.petal_id.clone(),
        }
    }
}

/// GET /api/v1/nodes/:node_id/address — resolve a node id to its stable
/// `fe://` endpoint URI (FR-1). Viewer+ and scope-checked.
pub async fn get_node_address(
    State(state): State<Arc<ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    Path(node_id): Path<String>,
) -> impl IntoResponse {
    if require_role(&claims, "viewer").is_err() {
        return Json(ApiResponse::<AddressDto>::error("insufficient permissions"));
    }
    if !is_addressable_node_id(&node_id) {
        return Json(ApiResponse::<AddressDto>::error("invalid node_id"));
    }
    let Some(scope) = resolve_node_scope(&state, &node_id).await else {
        return Json(ApiResponse::<AddressDto>::error("node not found"));
    };
    if require_scope(&claims, &scope).is_err() {
        return Json(ApiResponse::<AddressDto>::error("insufficient scope"));
    }
    match NodeAddress::from_scope_and_id(&scope, &node_id) {
        Ok(addr) => Json(ApiResponse::success(AddressDto::from_address(&addr))),
        Err(e) => Json(ApiResponse::<AddressDto>::error(format!(
            "cannot address node: {e}"
        ))),
    }
}

#[derive(Debug, Deserialize)]
pub struct ResolveQuery {
    pub uri: String,
}

/// GET /api/v1/address?uri=fe://v/f/p/n — parse a public URI back into its
/// components (FR-1, the inverse of `get_node_address`). Viewer+ and
/// scope-checked against the parsed scope.
pub async fn resolve_address(
    State(_state): State<Arc<ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    Query(q): Query<ResolveQuery>,
) -> impl IntoResponse {
    if require_role(&claims, "viewer").is_err() {
        return Json(ApiResponse::<AddressDto>::error("insufficient permissions"));
    }
    let addr = match NodeAddress::from_uri(&q.uri) {
        Ok(a) => a,
        Err(e) => {
            return Json(ApiResponse::<AddressDto>::error(format!(
                "invalid uri: {e}"
            )))
        }
    };
    if require_scope(&claims, &addr.scope()).is_err() {
        return Json(ApiResponse::<AddressDto>::error("insufficient scope"));
    }
    Json(ApiResponse::success(AddressDto::from_address(&addr)))
}

// ---------------------------------------------------------------------------
// FR-2: read endpoint per object
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct EndpointNodeDto {
    pub node_id: String,
    pub address: String,
    pub scope: String,
    /// `properties.node_kind` tag (`stamp` / `earthwork_region`), or `null`.
    pub kind: Option<String>,
    /// Full node row (common + type-specific fields — the generic abstraction).
    pub data: serde_json::Value,
}

/// GET /api/v1/nodes/:node_id — read an object's full payload (FR-2). Common
/// fields plus any type-specific payload carried in `properties` (stamp
/// overrides, earthwork footprint/material/volume, path curve). Tombstoned
/// nodes read as "not found". Unknown/unauthorized → typed error, never a panic.
pub async fn get_node(
    State(state): State<Arc<ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    Path(node_id): Path<String>,
) -> impl IntoResponse {
    if require_role(&claims, "viewer").is_err() {
        return Json(ApiResponse::<EndpointNodeDto>::error(
            "insufficient permissions",
        ));
    }
    if !is_addressable_node_id(&node_id) {
        return Json(ApiResponse::<EndpointNodeDto>::error("invalid node_id"));
    }
    let Some(scope) = resolve_node_scope(&state, &node_id).await else {
        return Json(ApiResponse::<EndpointNodeDto>::error("node not found"));
    };
    if require_scope(&claims, &scope).is_err() {
        return Json(ApiResponse::<EndpointNodeDto>::error("insufficient scope"));
    }

    match load_node(&state, &node_id, &scope).await {
        Ok(dto) => Json(ApiResponse::success(dto)),
        Err(e) => Json(ApiResponse::<EndpointNodeDto>::error(e)),
    }
}

/// Tombstone-filtered single-node read composed into an [`EndpointNodeDto`]
/// (the generic node abstraction). Shared by the REST `get_node` and the MCP
/// `read_node` tool (FR-2/FR-4). Callers must have already authorized `scope`.
pub(crate) async fn load_node(
    state: &ApiState,
    node_id: &str,
    scope: &str,
) -> Result<EndpointNodeDto, String> {
    let Some(ref db) = state.db_reader else {
        return Err("read endpoint not available (no db_reader)".to_string());
    };

    // Soft-deleted (tombstoned) rows read as absent.
    let row = match tokio::time::timeout(std::time::Duration::from_secs(5), async {
        db.query("SELECT * FROM node WHERE node_id = $nid AND tombstone = NONE LIMIT 1")
            .bind(("nid", node_id.to_string()))
            .await
    })
    .await
    {
        Ok(Ok(mut res)) => {
            let rows: Vec<serde_json::Value> = res.take(0).unwrap_or_default();
            rows.into_iter().next()
        }
        Ok(Err(e)) => return Err(format!("query failed: {e}")),
        Err(_) => return Err("request timed out".to_string()),
    };

    let data = row.ok_or_else(|| "node not found".to_string())?;

    let kind = data
        .get("properties")
        .and_then(|p| p.get(fe_query::spatial_nodes::KIND_KEY))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let address = NodeAddress::from_scope_and_id(scope, node_id)
        .map(|a| a.to_uri())
        .unwrap_or_default();

    Ok(EndpointNodeDto {
        node_id: node_id.to_string(),
        address,
        scope: scope.to_string(),
        kind,
        data,
    })
}

// ---------------------------------------------------------------------------
// FR-3: write / mutate endpoint (sync-safe ops, authorized in fe-policy)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, Default)]
pub struct DeleteQuery {
    /// `?cascade=true` tombstones the whole subtree in one atomic op (D-A7).
    #[serde(default)]
    pub cascade: bool,
}

#[derive(Debug, Serialize)]
pub struct DeleteDto {
    pub node_id: String,
    pub cascade: bool,
    pub tombstoned: bool,
}

/// DELETE /api/v1/nodes/:node_id[?cascade=true] — sync-safe tombstone delete
/// (FR-3, N-4). Routed through T1's `TombstoneNode` / `CascadeTombstoneNode`
/// ops — the same the UI uses — and authorized by fe-policy (Editor+ at scope).
/// Never a raw row drop; survives P2P/HLC merge.
pub async fn delete_node(
    State(state): State<Arc<ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    Path(node_id): Path<String>,
    Query(q): Query<DeleteQuery>,
) -> impl IntoResponse {
    // Cheap ceiling check; fe-policy is the authoritative Editor+ gate (N-5).
    if require_role(&claims, "editor").is_err() {
        return Json(ApiResponse::<DeleteDto>::error("insufficient permissions"));
    }
    if !is_addressable_node_id(&node_id) {
        return Json(ApiResponse::<DeleteDto>::error("invalid node_id"));
    }
    let Some(scope) = resolve_node_scope(&state, &node_id).await else {
        return Json(ApiResponse::<DeleteDto>::error("node not found"));
    };
    if require_scope(&claims, &scope).is_err() {
        return Json(ApiResponse::<DeleteDto>::error("insufficient scope"));
    }

    let cmd = if q.cascade {
        DbCommand::CascadeTombstoneNode {
            node_id: node_id.clone(),
            auth: caller_auth(&claims),
        }
    } else {
        DbCommand::TombstoneNode {
            node_id: node_id.clone(),
            auth: caller_auth(&claims),
        }
    };

    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    if state
        .api_cmd_tx
        .send(ApiCommand::DbRequest { cmd, reply_tx })
        .is_err()
    {
        return Json(ApiResponse::<DeleteDto>::error("internal channel closed"));
    }
    match tokio::time::timeout(std::time::Duration::from_secs(5), reply_rx).await {
        Ok(Ok(DbResult::NodeDeleted { .. })) => Json(ApiResponse::success(DeleteDto {
            node_id,
            cascade: q.cascade,
            tombstoned: true,
        })),
        Ok(Ok(DbResult::Error(e))) => {
            // fe-policy denial or missing node — surface as a typed error, not a panic.
            Json(ApiResponse::<DeleteDto>::error(e))
        }
        Ok(Ok(_)) => Json(ApiResponse::<DeleteDto>::error("unexpected response")),
        Ok(Err(_)) => Json(ApiResponse::<DeleteDto>::error("request cancelled")),
        Err(_) => Json(ApiResponse::<DeleteDto>::error("request timed out")),
    }
}

#[derive(Debug, Serialize)]
pub struct PromoteDto {
    pub node_id: String,
    pub petal_id: String,
    pub path_id: String,
    pub instance_index: u32,
    pub newly_promoted: bool,
}

/// POST /api/v1/petals/:petal_id/paths/:path_id/instances/:instance_index/promote
/// — materialize a full addressable node for a single stamp instance (FR-3 /
/// FR-6, D-A6 lazy promotion). Idempotent; authorized by fe-policy (Editor+).
pub async fn promote_instance(
    State(state): State<Arc<ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    Path((petal_id, path_id, instance_index)): Path<(String, String, u32)>,
) -> impl IntoResponse {
    if require_role(&claims, "editor").is_err() {
        return Json(ApiResponse::<PromoteDto>::error("insufficient permissions"));
    }
    if !is_valid_ulid(&petal_id) {
        return Json(ApiResponse::<PromoteDto>::error("invalid petal_id"));
    }
    let Some(scope) = resolve_petal_scope(&state, &petal_id).await else {
        return Json(ApiResponse::<PromoteDto>::error("petal not found"));
    };
    if require_scope(&claims, &scope).is_err() {
        return Json(ApiResponse::<PromoteDto>::error("insufficient scope"));
    }

    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    let cmd = DbCommand::PromoteInstance {
        petal_id: petal_id.clone(),
        path_id: path_id.clone(),
        instance_index,
        auth: caller_auth(&claims),
    };
    if state
        .api_cmd_tx
        .send(ApiCommand::DbRequest { cmd, reply_tx })
        .is_err()
    {
        return Json(ApiResponse::<PromoteDto>::error("internal channel closed"));
    }
    match tokio::time::timeout(std::time::Duration::from_secs(5), reply_rx).await {
        Ok(Ok(DbResult::NodePromoted {
            node_id,
            petal_id,
            path_id,
            instance_index,
            newly_promoted,
        })) => Json(ApiResponse::success(PromoteDto {
            node_id,
            petal_id,
            path_id,
            instance_index,
            newly_promoted,
        })),
        Ok(Ok(DbResult::Error(e))) => Json(ApiResponse::<PromoteDto>::error(e)),
        Ok(Ok(_)) => Json(ApiResponse::<PromoteDto>::error("unexpected response")),
        Ok(Err(_)) => Json(ApiResponse::<PromoteDto>::error("request cancelled")),
        Err(_) => Json(ApiResponse::<PromoteDto>::error("request timed out")),
    }
}

// ---------------------------------------------------------------------------
// FR-6: new object types are queryable (generic node abstraction + type tags)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct KindQuery {
    /// `stamp` | `earthwork_region`.
    pub kind: String,
    /// Optional owning-path filter (stamps only): "all stamps on path X".
    #[serde(default)]
    pub path_id: Option<String>,
}

/// GET /api/v1/petals/:petal_id/nodes?kind=stamp[&path_id=..] — list live nodes
/// of a type through the generic abstraction (FR-6, N-10). Row-capped read.
pub async fn list_nodes_by_kind(
    State(state): State<Arc<ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    Path(petal_id): Path<String>,
    Query(q): Query<KindQuery>,
) -> impl IntoResponse {
    if require_role(&claims, "viewer").is_err() {
        return Json(ApiResponse::<Vec<serde_json::Value>>::error(
            "insufficient permissions",
        ));
    }
    if !is_valid_ulid(&petal_id) {
        return Json(ApiResponse::<Vec<serde_json::Value>>::error(
            "invalid petal_id",
        ));
    }
    let Some(scope) = resolve_petal_scope(&state, &petal_id).await else {
        return Json(ApiResponse::<Vec<serde_json::Value>>::error(
            "petal not found",
        ));
    };
    if require_scope(&claims, &scope).is_err() {
        return Json(ApiResponse::<Vec<serde_json::Value>>::error(
            "insufficient scope",
        ));
    }
    let Some(kind) = fe_query::NodeKind::from_tag(&q.kind) else {
        return Json(ApiResponse::<Vec<serde_json::Value>>::error(
            "unknown node kind",
        ));
    };

    // "all stamps on path X" uses the path-scoped builder; otherwise all of a
    // kind in the petal. Both are fe-query generic-node SQL, tombstone-filtered.
    // Ids ride in `binds` ($petal_id/$path_id), never interpolated (injection-safe).
    let (sql, binds) = match (kind, q.path_id.as_deref()) {
        (fe_query::NodeKind::Stamp, Some(path_id)) => fe_query::stamps_on_path_sql(path_id),
        _ => fe_query::nodes_of_kind_sql(&petal_id, kind),
    };

    run_capped_select(&state, sql, binds).await
}

/// GET /api/v1/petals/:petal_id/earthwork/summary — aggregate real-unit cut/fill
/// across the petal's earthwork regions (FR-6: "total cut/fill in petal P").
pub async fn earthwork_summary(
    State(state): State<Arc<ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    Path(petal_id): Path<String>,
) -> impl IntoResponse {
    if require_role(&claims, "viewer").is_err() {
        return Json(ApiResponse::<Vec<serde_json::Value>>::error(
            "insufficient permissions",
        ));
    }
    if !is_valid_ulid(&petal_id) {
        return Json(ApiResponse::<Vec<serde_json::Value>>::error(
            "invalid petal_id",
        ));
    }
    let Some(scope) = resolve_petal_scope(&state, &petal_id).await else {
        return Json(ApiResponse::<Vec<serde_json::Value>>::error(
            "petal not found",
        ));
    };
    if require_scope(&claims, &scope).is_err() {
        return Json(ApiResponse::<Vec<serde_json::Value>>::error(
            "insufficient scope",
        ));
    }
    let (sql, binds) = fe_query::earthwork_volume_sql(&petal_id);
    run_capped_select(&state, sql, binds).await
}

/// Execute an internally-built SELECT under the shared read guards (row cap +
/// byte ceiling from `limits`). The SQL template is engine-built (not user
/// input); caller ids ride in `binds` as `$name` params (never interpolated),
/// so only the cost caps apply — not the SELECT-only validator.
async fn run_capped_select(
    state: &ApiState,
    sql: String,
    binds: Vec<(String, String)>,
) -> Json<ApiResponse<Vec<serde_json::Value>>> {
    let Some(ref db) = state.db_reader else {
        return Json(ApiResponse::error(
            "read endpoint not available (no db_reader)",
        ));
    };
    let guarded = crate::query_guard::GuardedQuery { sql };
    // Bind every id positionally ($petal_id/$path_id) — mirrors `load_node`'s
    // `.bind(...)`; `run_guarded_query` feeds these to the query builder.
    let vars: std::collections::HashMap<String, serde_json::Value> = binds
        .into_iter()
        .map(|(k, v)| (k, serde_json::Value::String(v)))
        .collect();
    match crate::query_guard::run_guarded_query(db, &guarded, &vars, crate::limits::QUERY_ROW_CAP)
        .await
    {
        Ok(data) => {
            if let Err(e) = crate::query_guard::enforce_byte_ceiling(
                &data,
                crate::limits::QUERY_MAX_RESPONSE_BYTES,
                crate::limits::QUERY_MAX_RESPONSE_LABEL,
            ) {
                return Json(ApiResponse::error(e));
            }
            Json(ApiResponse::success(data))
        }
        Err(e) => Json(ApiResponse::error(e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addressable_id_accepts_ulid_and_promoted_instance() {
        let ulid = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
        assert!(is_addressable_node_id(ulid));
        assert!(is_addressable_node_id(&format!("{ulid}#inst-0")));
        assert!(is_addressable_node_id(&format!("{ulid}#inst-42")));
    }

    #[test]
    fn addressable_id_rejects_junk() {
        assert!(!is_addressable_node_id(""));
        assert!(!is_addressable_node_id("not-a-ulid"));
        assert!(!is_addressable_node_id("01ARZ3NDEKTSV4RRFFQ69G5FAV#inst-"));
        assert!(!is_addressable_node_id("01ARZ3NDEKTSV4RRFFQ69G5FAV#inst-x"));
        assert!(!is_addressable_node_id("01ARZ3NDEKTSV4RRFFQ69G5FAV#frag-1"));
    }

    #[test]
    fn caller_auth_is_identified_never_local() {
        let claims = ApiClaims {
            sub: "did:key:z6MkA".into(),
            token_type: "api".into(),
            scope: "VERSE#v1".into(),
            max_role: "Editor".into(),
            iat: 0,
            exp: 0,
            jti: "j1".into(),
        };
        match caller_auth(&claims) {
            CallerAuth::Identified { did, role } => {
                assert_eq!(did, "did:key:z6MkA");
                assert_eq!(role, "editor", "role lowercased for policy mapping");
            }
            other => panic!("API callers must be Identified, got {other:?}"),
        }
    }
}
