use std::sync::Arc;

use axum::extract::{Json, Path, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Extension;
use axum_extra::extract::Multipart;
use fe_identity::api_token::ApiClaims;
use fe_policy::Action;
use fe_terrain::config::TerrainConfig;
use serde::Deserialize;

use crate::auth::{require_role, require_scope};
use crate::types::{is_valid_ulid, ApiResponse};

/// PUT /api/v1/petals/:petal_id/terrain — set terrain configuration.
///
/// RBAC: Editor+ required. Temporarily unavailable until the gateway can
/// correlate the durable `SetPetalTerrain` materialization result.
pub async fn set_terrain_config(
    State(state): State<Arc<crate::server::ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    Path(petal_id): Path<String>,
    Json(_config): Json<TerrainConfig>,
) -> Response {
    if require_role(&claims, "editor").is_err() {
        return Json(ApiResponse::<serde_json::Value>::error(
            "insufficient permissions",
        ))
        .into_response();
    }
    if !is_valid_ulid(&petal_id) {
        return Json(ApiResponse::<serde_json::Value>::error("invalid petal_id")).into_response();
    }

    let Some(scope) = resolve_petal_scope(&state, &petal_id).await else {
        return Json(ApiResponse::<serde_json::Value>::error(
            "could not resolve petal scope",
        ))
        .into_response();
    };
    if require_scope(&claims, &scope).is_err() {
        return Json(ApiResponse::<serde_json::Value>::error(
            "insufficient scope",
        ))
        .into_response();
    }

    terrain_mutation_unavailable()
}

/// GET /api/v1/petals/:petal_id/terrain — read terrain configuration.
///
/// RBAC: Viewer+ required.
pub async fn get_terrain_config(
    State(state): State<Arc<crate::server::ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    Path(petal_id): Path<String>,
) -> impl IntoResponse {
    if require_role(&claims, "viewer").is_err() {
        return Json(ApiResponse::<serde_json::Value>::error(
            "insufficient permissions",
        ));
    }
    if !is_valid_ulid(&petal_id) {
        return Json(ApiResponse::<serde_json::Value>::error("invalid petal_id"));
    }

    let Some(scope) = resolve_petal_scope(&state, &petal_id).await else {
        return Json(ApiResponse::<serde_json::Value>::error(
            "could not resolve petal scope",
        ));
    };
    if require_scope(&claims, &scope).is_err() {
        return Json(ApiResponse::<serde_json::Value>::error(
            "insufficient scope",
        ));
    }

    let Some(ref db) = state.db_reader else {
        return Json(ApiResponse::<serde_json::Value>::error(
            "direct DB access not available",
        ));
    };

    let mut res = match db
        .query("SELECT terrain FROM petal WHERE petal_id = $pid LIMIT 1")
        .bind(("pid", petal_id.clone()))
        .await
    {
        Ok(r) => r,
        Err(e) => {
            return Json(ApiResponse::<serde_json::Value>::error(format!(
                "query failed: {e}"
            )));
        }
    };

    let rows: Vec<serde_json::Value> = res.take(0).unwrap_or_default();
    let terrain = rows
        .first()
        .and_then(|r| r.get("terrain").cloned())
        .unwrap_or(serde_json::Value::Null);

    if terrain.is_null() {
        Json(ApiResponse::success(serde_json::json!({
            "petal_id": petal_id,
            "terrain": null,
        })))
    } else {
        Json(ApiResponse::success(serde_json::json!({
            "petal_id": petal_id,
            "terrain": terrain,
        })))
    }
}

/// DELETE /api/v1/petals/:petal_id/terrain — remove terrain binding.
///
/// RBAC: Editor+ required. Temporarily unavailable until the gateway can
/// correlate the durable `SetPetalTerrain` materialization result.
pub async fn delete_terrain_config(
    State(state): State<Arc<crate::server::ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    Path(petal_id): Path<String>,
) -> Response {
    if require_role(&claims, "editor").is_err() {
        return Json(ApiResponse::<serde_json::Value>::error(
            "insufficient permissions",
        ))
        .into_response();
    }
    if !is_valid_ulid(&petal_id) {
        return Json(ApiResponse::<serde_json::Value>::error("invalid petal_id")).into_response();
    }

    let Some(scope) = resolve_petal_scope(&state, &petal_id).await else {
        return Json(ApiResponse::<serde_json::Value>::error(
            "could not resolve petal scope",
        ))
        .into_response();
    };
    if require_scope(&claims, &scope).is_err() {
        return Json(ApiResponse::<serde_json::Value>::error(
            "insufficient scope",
        ))
        .into_response();
    }

    terrain_mutation_unavailable()
}

/// Reject mutation until the runtime can route the authoritative DB result to this request.
fn terrain_mutation_unavailable() -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(ApiResponse::<serde_json::Value>::error(
            "terrain configuration mutations are temporarily unavailable until durable command replies are correlated",
        )),
    )
        .into_response()
}

async fn resolve_petal_scope(state: &crate::server::ApiState, petal_id: &str) -> Option<String> {
    if let Some(ref db) = state.db_reader {
        return crate::rest::direct_resolve_petal_scope(db, petal_id).await;
    }
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    state
        .api_cmd_tx
        .send(fe_runtime::messages::ApiCommand::DbRequest {
            cmd: fe_runtime::messages::DbCommand::ResolvePetalScope {
                petal_id: petal_id.to_string(),
            },
            reply_tx,
        })
        .ok()?;
    match tokio::time::timeout(std::time::Duration::from_secs(3), reply_rx).await {
        Ok(Ok(fe_runtime::messages::DbResult::ScopeResolved { scope })) => scope,
        _ => None,
    }
}

/// Load the tileset identifiers bound to one petal's terrain configuration.
async fn assigned_tilesets(
    state: &crate::server::ApiState,
    petal_id: &str,
) -> Result<Vec<String>, &'static str> {
    let terrain = if let Some(ref db) = state.db_reader {
        let mut result = db
            .query("SELECT terrain FROM petal WHERE petal_id = $pid LIMIT 1")
            .bind(("pid", petal_id.to_string()))
            .await
            .map_err(|_| "failed to load petal terrain")?;
        let rows: Vec<serde_json::Value> =
            result.take(0).map_err(|_| "failed to load petal terrain")?;
        rows.first().and_then(|row| row.get("terrain")).cloned()
    } else {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        state
            .api_cmd_tx
            .send(fe_runtime::messages::ApiCommand::DbRequest {
                cmd: fe_runtime::messages::DbCommand::GetPetalTerrain {
                    petal_id: petal_id.to_string(),
                },
                reply_tx,
            })
            .map_err(|_| "failed to load petal terrain")?;
        match tokio::time::timeout(std::time::Duration::from_secs(3), reply_rx).await {
            Ok(Ok(fe_runtime::messages::DbResult::PetalTerrainLoaded { terrain, .. })) => terrain,
            _ => return Err("failed to load petal terrain"),
        }
    };

    Ok(terrain
        .and_then(|value| value.get("tileset_hexon_uris").cloned())
        .and_then(|value| value.as_array().cloned())
        .map(|values| {
            values
                .iter()
                .filter_map(|value| value.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default())
}

/// Check whether a tileset is bound to the authorized petal.
fn is_assigned_tileset(assigned: &[String], hexon_id: &str) -> bool {
    assigned.iter().any(|id| id == hexon_id)
}

/// Check whether another petal references a tileset before a local-store mutation.
async fn is_referenced_by_another_petal(
    state: &crate::server::ApiState,
    petal_id: &str,
    hexon_id: &str,
) -> Result<bool, &'static str> {
    let Some(db) = state.db_reader.as_ref() else {
        return Err("direct DB access is required to verify tileset ownership");
    };
    let mut result = db
        .query("SELECT petal_id, terrain FROM petal")
        .await
        .map_err(|_| "failed to inspect tileset bindings")?;
    let rows: Vec<serde_json::Value> = result
        .take(0)
        .map_err(|_| "failed to inspect tileset bindings")?;

    Ok(rows.iter().any(|row| {
        row.get("petal_id")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|candidate_id| candidate_id != petal_id)
            && row
                .get("terrain")
                .and_then(|terrain| terrain.get("tileset_hexon_uris"))
                .and_then(serde_json::Value::as_array)
                .is_some_and(|ids| {
                    ids.iter()
                        .filter_map(serde_json::Value::as_str)
                        .any(|id| id == hexon_id)
                })
    }))
}

/// Validate the requested petal is the tileset mutation target's sole binding.
fn verify_exclusive_petal_tileset_binding(
    assigned: &[String],
    referenced_by_another_petal: bool,
    hexon_id: &str,
) -> Result<(), &'static str> {
    if !is_assigned_tileset(assigned, hexon_id) {
        return Err("tileset is not assigned to petal");
    }
    if referenced_by_another_petal {
        return Err("tileset remains assigned to another petal");
    }
    Ok(())
}

/// Resolve the sole-petal ownership relation required for a store mutation.
async fn require_exclusive_petal_tileset_binding(
    state: &crate::server::ApiState,
    petal_id: &str,
    hexon_id: &str,
) -> Result<(), &'static str> {
    let assigned = assigned_tilesets(state, petal_id).await?;
    let referenced_by_another_petal =
        is_referenced_by_another_petal(state, petal_id, hexon_id).await?;
    verify_exclusive_petal_tileset_binding(&assigned, referenced_by_another_petal, hexon_id)
}

/// Read a terrain archive's declared identifier before allowing any store write.
fn uploaded_terrain_tileset_id(bytes: &[u8]) -> Result<String, &'static str> {
    let archive = fe_format::HexonArchive::import(bytes).map_err(|_| "invalid tileset archive")?;
    let hexon_id = archive.manifest.hexon_id.clone();
    match &archive.manifest.hexon_type {
        fe_format::HexonType::TerrainTileset => Ok(hexon_id),
        _ => Err("archive is not a terrain tileset"),
    }
}

fn tile_access_status(message: &str) -> StatusCode {
    match message {
        "invalid petal_id" => StatusCode::BAD_REQUEST,
        "could not resolve petal scope" => StatusCode::NOT_FOUND,
        _ => StatusCode::FORBIDDEN,
    }
}

/// Resolve an authorized petal and return only the tilesets its terrain binds.
async fn authorized_petal_tilesets(
    state: &crate::server::ApiState,
    claims: &ApiClaims,
    petal_id: &str,
) -> Result<Vec<String>, StatusCode> {
    crate::hexon::require_hexon_petal_access(state, claims, petal_id, "viewer", Action::Read)
        .await
        .map_err(tile_access_status)?;
    assigned_tilesets(state, petal_id)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)
}

/// Verify that a requested tileset is bound to the caller-authorized petal.
async fn authorize_tileset_read(
    state: &crate::server::ApiState,
    claims: &ApiClaims,
    petal_id: &str,
    tileset_id: &str,
) -> Result<(), StatusCode> {
    let assigned = authorized_petal_tilesets(state, claims, petal_id).await?;
    if is_assigned_tileset(&assigned, tileset_id) {
        Ok(())
    } else {
        // Do not reveal whether an unbound tileset exists in the local store.
        Err(StatusCode::NOT_FOUND)
    }
}

// ---------------------------------------------------------------------------
// Authenticated, petal-scoped tile serving endpoints
// ---------------------------------------------------------------------------

/// GET /api/v1/tiles/:tileset_id/:z/:x/:y_png?petal_id=... — serve an elevation tile as PNG.
///
/// The `y_png` path segment must have a `.png` suffix (e.g. `340.png`).
/// The caller needs Viewer+ access to the petal that binds the tileset.
pub async fn get_elevation_tile(
    State(state): State<Arc<crate::server::ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    Path((tileset_id, z, x, y_png)): Path<(String, u8, u32, String)>,
    Query(query): Query<PetalScopeQuery>,
) -> Response {
    if let Err(status) = authorize_tileset_read(&state, &claims, &query.petal_id, &tileset_id).await
    {
        return status.into_response();
    }
    let Some(ref registry) = state.tileset_registry else {
        return StatusCode::NOT_FOUND.into_response();
    };

    let y_str = y_png.trim_end_matches(".png");
    let y: u32 = match y_str.parse() {
        Ok(v) => v,
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
    };

    match registry.get_tile(&tileset_id, z, x, y) {
        Some(bytes) => {
            let mut headers = HeaderMap::new();
            headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("image/png"));
            headers.insert(
                header::CACHE_CONTROL,
                HeaderValue::from_static("private, no-store"),
            );
            (StatusCode::OK, headers, bytes).into_response()
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// GET /api/v1/tiles/:tileset_id/:z/:x/:y_jpg?petal_id=... — serve a satellite tile as JPEG.
///
/// The `y_jpg` path segment must have a `.jpg` suffix (e.g. `340.jpg`).
/// The caller needs Viewer+ access to the petal that binds the tileset.
pub async fn get_satellite_tile(
    State(state): State<Arc<crate::server::ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    Path((tileset_id, z, x, y_jpg)): Path<(String, u8, u32, String)>,
    Query(query): Query<PetalScopeQuery>,
) -> Response {
    if let Err(status) = authorize_tileset_read(&state, &claims, &query.petal_id, &tileset_id).await
    {
        return status.into_response();
    }
    let Some(ref registry) = state.tileset_registry else {
        return StatusCode::NOT_FOUND.into_response();
    };

    let y_str = y_jpg.trim_end_matches(".jpg");
    let y: u32 = match y_str.parse() {
        Ok(v) => v,
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
    };

    match registry.get_satellite_tile(&tileset_id, z, x, y) {
        Some(bytes) => {
            let mut headers = HeaderMap::new();
            headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("image/jpeg"));
            headers.insert(
                header::CACHE_CONTROL,
                HeaderValue::from_static("private, no-store"),
            );
            (StatusCode::OK, headers, bytes).into_response()
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// GET /api/v1/tilesets?petal_id=... — list tilesets bound to an authorized petal.
///
/// Returns a JSON array of [`TilesetInfo`] objects without exposing other local tilesets.
pub async fn list_available_tilesets(
    State(state): State<Arc<crate::server::ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    Query(query): Query<PetalScopeQuery>,
) -> Response {
    let assigned = match authorized_petal_tilesets(&state, &claims, &query.petal_id).await {
        Ok(assigned) => assigned,
        Err(status) => return status.into_response(),
    };
    let Some(ref registry) = state.tileset_registry else {
        return Json(ApiResponse::success(serde_json::json!([]))).into_response();
    };
    let tilesets: Vec<_> = registry
        .list_tilesets()
        .into_iter()
        .filter(|tileset| is_assigned_tileset(&assigned, &tileset.tileset_id))
        .collect();
    Json(ApiResponse::success(
        serde_json::to_value(tilesets).unwrap_or_default(),
    ))
    .into_response()
}

/// GET /api/v1/tilesets/:tileset_id/meta?petal_id=... — return authorized tileset metadata.
///
/// Returns 404 when the tileset is not bound to the authorized petal or unknown.
pub async fn get_tileset_meta(
    State(state): State<Arc<crate::server::ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    Path(tileset_id): Path<String>,
    Query(query): Query<PetalScopeQuery>,
) -> Response {
    if let Err(status) = authorize_tileset_read(&state, &claims, &query.petal_id, &tileset_id).await
    {
        return status.into_response();
    }
    let Some(ref registry) = state.tileset_registry else {
        return StatusCode::NOT_FOUND.into_response();
    };
    match registry.get_meta(&tileset_id) {
        Some(meta) => Json(ApiResponse::success(
            serde_json::to_value(meta).unwrap_or_default(),
        ))
        .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

// ---------------------------------------------------------------------------
// Authenticated hexon management endpoints (Editor+)
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct PetalScopeQuery {
    petal_id: String,
}

/// POST /api/v1/hexons/tilesets/install — install a `.hexon` tileset archive.
///
/// Accepts a `multipart/form-data` request with one file field containing the
/// raw `.hexon` bytes.  Returns the [`InstalledTileset`] record on success.
///
/// RBAC: Editor+ required.
pub async fn install_hexon_tileset(
    State(state): State<Arc<crate::server::ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    Query(query): Query<PetalScopeQuery>,
    mut multipart: Multipart,
) -> impl IntoResponse {
    if let Err(message) = crate::hexon::require_hexon_petal_access(
        &state,
        &claims,
        &query.petal_id,
        "editor",
        Action::Install,
    )
    .await
    {
        return Json(ApiResponse::<serde_json::Value>::error(message));
    }

    let Some(ref registry) = state.tileset_registry else {
        return Json(ApiResponse::<serde_json::Value>::error(
            "tileset registry not available",
        ));
    };

    // Read the first multipart field as the hexon bytes.
    let field = match multipart.next_field().await {
        Ok(Some(f)) => f,
        Ok(None) => {
            return Json(ApiResponse::<serde_json::Value>::error(
                "no file field in multipart body",
            ));
        }
        Err(e) => {
            return Json(ApiResponse::<serde_json::Value>::error(format!(
                "multipart error: {e}"
            )));
        }
    };

    let bytes = match field.bytes().await {
        Ok(b) => b,
        Err(e) => {
            return Json(ApiResponse::<serde_json::Value>::error(format!(
                "failed to read upload: {e}"
            )));
        }
    };

    // Verify exclusive petal ownership before the registry can replace this ID.
    let hexon_id = match uploaded_terrain_tileset_id(&bytes) {
        Ok(hexon_id) => hexon_id,
        Err(message) => return Json(ApiResponse::<serde_json::Value>::error(message)),
    };
    if let Err(message) =
        require_exclusive_petal_tileset_binding(&state, &query.petal_id, &hexon_id).await
    {
        return Json(ApiResponse::<serde_json::Value>::error(message));
    }

    match registry.install(&bytes) {
        Ok(installed) => Json(ApiResponse::success(
            serde_json::to_value(installed).unwrap_or_default(),
        )),
        Err(e) => Json(ApiResponse::<serde_json::Value>::error(format!(
            "install failed: {e}"
        ))),
    }
}

/// DELETE /api/v1/hexons/tilesets/:hexon_id — remove an installed hexon tileset.
///
/// RBAC: Editor+ required.
pub async fn remove_hexon_tileset(
    State(state): State<Arc<crate::server::ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    Path(hexon_id): Path<String>,
    Query(query): Query<PetalScopeQuery>,
) -> impl IntoResponse {
    if let Err(message) = crate::hexon::require_hexon_petal_access(
        &state,
        &claims,
        &query.petal_id,
        "editor",
        Action::Write,
    )
    .await
    {
        return Json(ApiResponse::<serde_json::Value>::error(message));
    }

    if let Err(message) =
        require_exclusive_petal_tileset_binding(&state, &query.petal_id, &hexon_id).await
    {
        return Json(ApiResponse::<serde_json::Value>::error(message));
    }

    let Some(ref registry) = state.tileset_registry else {
        return Json(ApiResponse::<serde_json::Value>::error(
            "tileset registry not available",
        ));
    };

    match registry.remove(&hexon_id) {
        Ok(()) => Json(ApiResponse::success(serde_json::json!({
            "hexon_id": hexon_id,
            "removed": true,
        }))),
        Err(e) => Json(ApiResponse::<serde_json::Value>::error(format!(
            "remove failed: {e}"
        ))),
    }
}

#[derive(Deserialize)]
pub struct SeedingBody {
    pub enabled: bool,
}

/// PATCH /api/v1/hexons/tilesets/:hexon_id/seeding — enable or disable P2P seeding.
///
/// Body: `{ "enabled": bool }`
///
/// RBAC: Editor+ required.
pub async fn toggle_seeding(
    State(state): State<Arc<crate::server::ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    Path(hexon_id): Path<String>,
    Query(query): Query<PetalScopeQuery>,
    Json(body): Json<SeedingBody>,
) -> impl IntoResponse {
    if let Err(message) = crate::hexon::require_hexon_petal_access(
        &state,
        &claims,
        &query.petal_id,
        "editor",
        Action::Write,
    )
    .await
    {
        return Json(ApiResponse::<serde_json::Value>::error(message));
    }

    let assigned = match assigned_tilesets(&state, &query.petal_id).await {
        Ok(assigned) => assigned,
        Err(message) => return Json(ApiResponse::<serde_json::Value>::error(message)),
    };
    if !is_assigned_tileset(&assigned, &hexon_id) {
        return Json(ApiResponse::<serde_json::Value>::error(
            "tileset is not assigned to petal",
        ));
    }

    tracing::warn!(
        hexon_id,
        requested = body.enabled,
        "rejecting API tileset seeding while P2P distribution is disabled"
    );
    Json(ApiResponse::<serde_json::Value>::error(
        "P2P tileset seeding is unavailable",
    ))
}

/// GET /api/v1/hexons/tilesets — list all installed hexon tilesets.
///
/// Returns [`InstalledTileset`] records plus overall disk usage.
///
/// RBAC: Editor+ required.
pub async fn list_installed_hexons(
    State(state): State<Arc<crate::server::ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    Query(query): Query<PetalScopeQuery>,
) -> impl IntoResponse {
    if let Err(message) = crate::hexon::require_hexon_petal_access(
        &state,
        &claims,
        &query.petal_id,
        "editor",
        Action::Read,
    )
    .await
    {
        return Json(ApiResponse::<serde_json::Value>::error(message));
    }

    let assigned = match assigned_tilesets(&state, &query.petal_id).await {
        Ok(assigned) => assigned,
        Err(message) => return Json(ApiResponse::<serde_json::Value>::error(message)),
    };

    let Some(ref registry) = state.tileset_registry else {
        return Json(ApiResponse::success(serde_json::json!({
            "tilesets": [],
            "total_disk_usage_bytes": 0u64,
        })));
    };

    let store = registry.store();
    let tilesets: Vec<_> = store
        .list_installed()
        .into_iter()
        .filter(|tileset| is_assigned_tileset(&assigned, &tileset.hexon_id))
        .collect();
    let total: u64 = tilesets.iter().map(|tileset| tileset.size_bytes).sum();

    Json(ApiResponse::success(serde_json::json!({
        "tilesets": tilesets,
        "total_disk_usage_bytes": total,
    })))
}

/// GET /api/v1/hexons/storage — summary of the hexon store on disk.
///
/// Returns `{ "base_dir": "...", "total_disk_usage_bytes": N, "tileset_count": N }`.
///
/// RBAC: Editor+ required.
pub async fn get_storage_info(
    State(state): State<Arc<crate::server::ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    Query(query): Query<PetalScopeQuery>,
) -> impl IntoResponse {
    if let Err(message) = crate::hexon::require_hexon_petal_access(
        &state,
        &claims,
        &query.petal_id,
        "editor",
        Action::Read,
    )
    .await
    {
        return Json(ApiResponse::<serde_json::Value>::error(message));
    }

    let assigned = match assigned_tilesets(&state, &query.petal_id).await {
        Ok(assigned) => assigned,
        Err(message) => return Json(ApiResponse::<serde_json::Value>::error(message)),
    };

    let Some(ref registry) = state.tileset_registry else {
        return Json(ApiResponse::success(serde_json::json!({
            "base_dir": "",
            "total_disk_usage_bytes": 0u64,
            "tileset_count": 0u32,
        })));
    };

    let store = registry.store();
    let base_dir = store.base_dir().to_string_lossy().to_string();
    let scoped_tilesets: Vec<_> = store
        .list_installed()
        .into_iter()
        .filter(|tileset| is_assigned_tileset(&assigned, &tileset.hexon_id))
        .collect();
    let total: u64 = scoped_tilesets
        .iter()
        .map(|tileset| tileset.size_bytes)
        .sum();
    let count = scoped_tilesets.len();

    Json(ApiResponse::success(serde_json::json!({
        "base_dir": base_dir,
        "total_disk_usage_bytes": total,
        "tileset_count": count,
    })))
}

#[cfg(test)]
mod tests {
    use super::{
        is_assigned_tileset, terrain_mutation_unavailable, tile_access_status,
        verify_exclusive_petal_tileset_binding,
    };
    use axum::http::StatusCode;

    #[test]
    fn tileset_binding_is_exact() {
        let assigned = vec!["tileset-a".to_string(), "tileset-b".to_string()];
        assert!(is_assigned_tileset(&assigned, "tileset-a"));
        assert!(!is_assigned_tileset(&assigned, "tileset-c"));
    }

    #[test]
    fn store_mutation_requires_the_requested_petals_exclusive_binding() {
        let assigned = vec!["tileset-a".to_string()];

        assert_eq!(
            verify_exclusive_petal_tileset_binding(&assigned, false, "tileset-b"),
            Err("tileset is not assigned to petal")
        );
        assert_eq!(
            verify_exclusive_petal_tileset_binding(&assigned, true, "tileset-a"),
            Err("tileset remains assigned to another petal")
        );
        assert_eq!(
            verify_exclusive_petal_tileset_binding(&assigned, false, "tileset-a"),
            Ok(())
        );
    }

    #[test]
    fn tile_scope_errors_fail_closed() {
        assert_eq!(
            tile_access_status("invalid petal_id"),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            tile_access_status("could not resolve petal scope"),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            tile_access_status("insufficient scope"),
            StatusCode::FORBIDDEN
        );
    }

    #[test]
    fn terrain_mutations_fail_explicitly_until_replies_are_correlated() {
        assert_eq!(
            terrain_mutation_unavailable().status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
    }
}
