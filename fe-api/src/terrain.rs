use std::sync::Arc;

use axum::extract::{Json, Path, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Extension;
use axum_extra::extract::Multipart;
use fe_identity::api_token::ApiClaims;
use fe_terrain::config::TerrainConfig;
use serde::Deserialize;

use crate::auth::{require_role, require_scope};
use crate::types::{is_valid_ulid, ApiResponse};

/// PUT /api/v1/petals/:petal_id/terrain — set terrain configuration.
///
/// RBAC: Editor+ required.
/// Stores the terrain config as a JSON field on the petal record.
pub async fn set_terrain_config(
    State(state): State<Arc<crate::server::ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    Path(petal_id): Path<String>,
    Json(config): Json<TerrainConfig>,
) -> impl IntoResponse {
    if require_role(&claims, "editor").is_err() {
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

    let config_json = match serde_json::to_value(&config) {
        Ok(v) => v,
        Err(e) => {
            return Json(ApiResponse::<serde_json::Value>::error(format!(
                "invalid config: {e}"
            )));
        }
    };

    match db
        .query("UPDATE petal SET terrain = $config WHERE petal_id = $pid")
        .bind(("config", config_json.clone()))
        .bind(("pid", petal_id.clone()))
        .await
    {
        Ok(_) => Json(ApiResponse::success(serde_json::json!({
            "petal_id": petal_id,
            "terrain": config_json,
        }))),
        Err(e) => Json(ApiResponse::<serde_json::Value>::error(format!(
            "failed to save terrain config: {e}"
        ))),
    }
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
/// RBAC: Editor+ required.
pub async fn delete_terrain_config(
    State(state): State<Arc<crate::server::ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    Path(petal_id): Path<String>,
) -> impl IntoResponse {
    if require_role(&claims, "editor").is_err() {
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

    match db
        .query("UPDATE petal SET terrain = NONE WHERE petal_id = $pid")
        .bind(("pid", petal_id.clone()))
        .await
    {
        Ok(_) => Json(ApiResponse::success(serde_json::json!({
            "petal_id": petal_id,
            "deleted": true,
        }))),
        Err(e) => Json(ApiResponse::<serde_json::Value>::error(format!(
            "failed to delete terrain config: {e}"
        ))),
    }
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

// ---------------------------------------------------------------------------
// Public tile serving endpoints (no auth required)
// ---------------------------------------------------------------------------

/// GET /api/v1/tiles/:tileset_id/:z/:x/:y_png — serve an elevation tile as PNG.
///
/// The `y_png` path segment must have a `.png` suffix (e.g. `340.png`).
/// Returns raw PNG bytes with `Content-Type: image/png` and a 24 h cache header.
pub async fn get_elevation_tile(
    State(state): State<Arc<crate::server::ApiState>>,
    Path((tileset_id, z, x, y_png)): Path<(String, u8, u32, String)>,
) -> Response {
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
                HeaderValue::from_static("public, max-age=86400"),
            );
            (StatusCode::OK, headers, bytes).into_response()
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// GET /api/v1/tiles/:tileset_id/:z/:x/:y_jpg — serve a satellite tile as JPEG.
///
/// The `y_jpg` path segment must have a `.jpg` suffix (e.g. `340.jpg`).
/// Returns raw JPEG bytes with `Content-Type: image/jpeg` and a 24 h cache header.
pub async fn get_satellite_tile(
    State(state): State<Arc<crate::server::ApiState>>,
    Path((tileset_id, z, x, y_jpg)): Path<(String, u8, u32, String)>,
) -> Response {
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
                HeaderValue::from_static("public, max-age=86400"),
            );
            (StatusCode::OK, headers, bytes).into_response()
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// GET /api/v1/tilesets — list all available tilesets.
///
/// Returns a JSON array of [`TilesetInfo`] objects.
pub async fn list_available_tilesets(
    State(state): State<Arc<crate::server::ApiState>>,
) -> impl IntoResponse {
    let Some(ref registry) = state.tileset_registry else {
        return Json(ApiResponse::success(serde_json::json!([])));
    };
    let tilesets = registry.list_tilesets();
    Json(ApiResponse::success(
        serde_json::to_value(tilesets).unwrap_or_default(),
    ))
}

/// GET /api/v1/tilesets/:tileset_id/meta — return full metadata for a tileset.
///
/// Returns 404 (inside the success wrapper) when the tileset is unknown.
pub async fn get_tileset_meta(
    State(state): State<Arc<crate::server::ApiState>>,
    Path(tileset_id): Path<String>,
) -> impl IntoResponse {
    let Some(ref registry) = state.tileset_registry else {
        return Json(ApiResponse::<serde_json::Value>::error(
            "tileset registry not available",
        ));
    };
    match registry.get_meta(&tileset_id) {
        Some(meta) => Json(ApiResponse::success(
            serde_json::to_value(meta).unwrap_or_default(),
        )),
        None => Json(ApiResponse::<serde_json::Value>::error("tileset not found")),
    }
}

// ---------------------------------------------------------------------------
// Authenticated hexon management endpoints (Editor+)
// ---------------------------------------------------------------------------

/// POST /api/v1/hexons/tilesets/install — install a `.hexon` tileset archive.
///
/// Accepts a `multipart/form-data` request with one file field containing the
/// raw `.hexon` bytes.  Returns the [`InstalledTileset`] record on success.
///
/// RBAC: Editor+ required.
pub async fn install_hexon_tileset(
    State(state): State<Arc<crate::server::ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    mut multipart: Multipart,
) -> impl IntoResponse {
    if require_role(&claims, "editor").is_err() {
        return Json(ApiResponse::<serde_json::Value>::error(
            "insufficient permissions",
        ));
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
) -> impl IntoResponse {
    if require_role(&claims, "editor").is_err() {
        return Json(ApiResponse::<serde_json::Value>::error(
            "insufficient permissions",
        ));
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
    Json(body): Json<SeedingBody>,
) -> impl IntoResponse {
    if require_role(&claims, "editor").is_err() {
        return Json(ApiResponse::<serde_json::Value>::error(
            "insufficient permissions",
        ));
    }

    let Some(ref registry) = state.tileset_registry else {
        return Json(ApiResponse::<serde_json::Value>::error(
            "tileset registry not available",
        ));
    };

    match registry.set_seeding(&hexon_id, body.enabled) {
        Ok(()) => Json(ApiResponse::success(serde_json::json!({
            "hexon_id": hexon_id,
            "seeding_enabled": body.enabled,
        }))),
        Err(e) => Json(ApiResponse::<serde_json::Value>::error(format!(
            "set_seeding failed: {e}"
        ))),
    }
}

/// GET /api/v1/hexons/tilesets — list all installed hexon tilesets.
///
/// Returns [`InstalledTileset`] records plus overall disk usage.
///
/// RBAC: Editor+ required.
pub async fn list_installed_hexons(
    State(state): State<Arc<crate::server::ApiState>>,
    Extension(claims): Extension<ApiClaims>,
) -> impl IntoResponse {
    if require_role(&claims, "editor").is_err() {
        return Json(ApiResponse::<serde_json::Value>::error(
            "insufficient permissions",
        ));
    }

    let Some(ref registry) = state.tileset_registry else {
        return Json(ApiResponse::success(serde_json::json!({
            "tilesets": [],
            "total_disk_usage_bytes": 0u64,
        })));
    };

    let store = registry.store();
    let tilesets = store.list_installed();
    let total = store.total_disk_usage();

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
) -> impl IntoResponse {
    if require_role(&claims, "editor").is_err() {
        return Json(ApiResponse::<serde_json::Value>::error(
            "insufficient permissions",
        ));
    }

    let Some(ref registry) = state.tileset_registry else {
        return Json(ApiResponse::success(serde_json::json!({
            "base_dir": "",
            "total_disk_usage_bytes": 0u64,
            "tileset_count": 0u32,
        })));
    };

    let store = registry.store();
    let base_dir = store.base_dir().to_string_lossy().to_string();
    let total = store.total_disk_usage();
    let count = store.list_installed().len();

    Json(ApiResponse::success(serde_json::json!({
        "base_dir": base_dir,
        "total_disk_usage_bytes": total,
        "tileset_count": count,
    })))
}
