//! Asset delivery endpoints -- serve content-addressed blobs by hash, asset_id, or node_id.
//!
//! Rationale + the directory-asset extension point: see `fe-api/AGENTS.md` §assets.

use std::path::Path as FsPath;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::header::{CONTENT_DISPOSITION, CONTENT_LENGTH, CONTENT_TYPE};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use fe_identity::api_token::ApiClaims;
use fe_runtime::blob_store::{hash_from_hex, BlobStoreHandle};

use crate::auth::{require_role, require_scope};
use crate::server::ApiState;
use crate::types::is_valid_ulid;

/// SurrealDB connection type used by the direct (channel-bypassing) read helpers below.
type Db = surrealdb::Surreal<surrealdb::engine::local::Db>;

/// Placeholder content-type for the planned "directory of files" asset shape
/// (P2P bucket / "3D visual IPFS" — see AGENTS.md). Not servable as a single
/// byte stream yet; the handler responds 501 instead of guessing.
const DIRECTORY_ASSET_CONTENT_TYPE: &str = "application/x-fe-directory";

/// Safety cap on in-memory blob reads. Mirrors the existing full-read pattern
/// used by `get_asset` below, just bounded so a pathological/huge blob can't
/// exhaust the API thread's memory in one request.
const MAX_ASSET_RESPONSE_BYTES: u64 = 1024 * 1024 * 1024; // 1 GiB

// ---------------------------------------------------------------------------
// GET /api/v1/assets/:content_hash — raw content-addressed blob (unchanged).
// ---------------------------------------------------------------------------

/// GET /api/v1/assets/:content_hash
///
/// Serves a blob from the content-addressed store with immutable caching headers.
/// Returns 404 if the hash is not found in the blob store.
pub async fn get_asset(
    State(state): State<Arc<ApiState>>,
    Path(content_hash): Path<String>,
) -> impl IntoResponse {
    let Some(ref blob_store) = state.blob_store else {
        return (StatusCode::SERVICE_UNAVAILABLE, HeaderMap::new(), Vec::new());
    };

    let hash = match fe_runtime::blob_store::hash_from_hex(&content_hash) {
        Ok(h) => h,
        Err(_) => return (StatusCode::BAD_REQUEST, HeaderMap::new(), Vec::new()),
    };

    let Some(path) = blob_store.get_blob_path(&hash) else {
        return (StatusCode::NOT_FOUND, HeaderMap::new(), Vec::new());
    };

    match std::fs::read(&path) {
        Ok(data) => {
            let mut headers = HeaderMap::new();
            headers.insert(
                "content-type",
                HeaderValue::from_static("application/octet-stream"),
            );
            headers.insert(
                "etag",
                HeaderValue::from_str(&format!("\"blake3:{content_hash}\""))
                    .unwrap_or(HeaderValue::from_static("\"unknown\"")),
            );
            headers.insert(
                "cache-control",
                HeaderValue::from_static("public, max-age=31536000, immutable"),
            );
            (StatusCode::OK, headers, data)
        }
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, HeaderMap::new(), Vec::new()),
    }
}

// ---------------------------------------------------------------------------
// GET /api/v1/nodes/:node_id/asset — download by node, with real metadata.
// ---------------------------------------------------------------------------

/// GET /api/v1/nodes/:node_id/asset — resolve node -> asset -> blob, and stream
/// the asset back with its real `content_type` / filename / length.
///
/// RBAC: Viewer+ required, scope resolved from the node's parent chain (same
/// helper used by `/nodes/:node_id/transform` etc).
pub async fn get_node_asset(
    State(state): State<Arc<ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    Path(node_id): Path<String>,
) -> Response {
    if require_role(&claims, "viewer").is_err() {
        return error_response(StatusCode::FORBIDDEN, "insufficient permissions");
    }
    if !is_valid_ulid(&node_id) {
        return error_response(StatusCode::BAD_REQUEST, "invalid node_id");
    }

    // Scope RBAC goes through the existing channel-or-direct resolver, so this
    // still works even without a `db_reader` configured.
    let Some(scope) = crate::rest::resolve_node_scope(&state, &node_id).await else {
        return error_response(StatusCode::NOT_FOUND, "node not found");
    };
    if require_scope(&claims, &scope).is_err() {
        return error_response(StatusCode::FORBIDDEN, "insufficient scope");
    }

    let Some(ref db) = state.db_reader else {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "asset lookup requires a direct db_reader connection (not configured on this server)",
        );
    };
    let Some(ref blob_store) = state.blob_store else {
        return error_response(StatusCode::SERVICE_UNAVAILABLE, "blob store not configured");
    };

    let asset_id = match direct_resolve_node_asset_id(db, &node_id).await {
        Ok(NodeAssetLookup::NodeNotFound) => {
            return error_response(StatusCode::NOT_FOUND, "node not found")
        }
        Ok(NodeAssetLookup::NoAsset) => {
            return error_response(StatusCode::NOT_FOUND, "node has no associated asset")
        }
        Ok(NodeAssetLookup::Asset(id)) => id,
        Err(e) => {
            tracing::error!("node asset lookup failed for node_id={node_id}: {e}");
            return error_response(StatusCode::BAD_GATEWAY, "node asset lookup failed");
        }
    };

    serve_asset_by_id(db, blob_store, &asset_id).await
}

// ---------------------------------------------------------------------------
// GET /api/v1/assets/by-id/:asset_id — download by asset_id directly.
// ---------------------------------------------------------------------------

/// GET /api/v1/assets/by-id/:asset_id — resolve asset -> blob directly, for
/// callers that already hold an `asset_id` (e.g. from a prior hierarchy fetch).
///
/// Assets carry no scope of their own, so RBAC is resolved via the first node
/// that references this asset (see `direct_resolve_asset_scope` and
/// `fe-api/AGENTS.md` §assets for the rationale/limits of that choice).
pub async fn get_asset_by_id(
    State(state): State<Arc<ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    Path(asset_id): Path<String>,
) -> Response {
    if require_role(&claims, "viewer").is_err() {
        return error_response(StatusCode::FORBIDDEN, "insufficient permissions");
    }
    if !is_valid_ulid(&asset_id) {
        return error_response(StatusCode::BAD_REQUEST, "invalid asset_id");
    }

    let Some(ref db) = state.db_reader else {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "asset lookup requires a direct db_reader connection (not configured on this server)",
        );
    };
    let Some(ref blob_store) = state.blob_store else {
        return error_response(StatusCode::SERVICE_UNAVAILABLE, "blob store not configured");
    };

    let scope = match direct_resolve_asset_scope(db, &asset_id).await {
        Some(s) => s,
        None => {
            return error_response(
                StatusCode::NOT_FOUND,
                "asset not found (or not referenced by any node)",
            )
        }
    };
    if require_scope(&claims, &scope).is_err() {
        return error_response(StatusCode::FORBIDDEN, "insufficient scope");
    }

    serve_asset_by_id(db, blob_store, &asset_id).await
}

// ---------------------------------------------------------------------------
// Shared serving logic
// ---------------------------------------------------------------------------

/// Metadata row from the `asset` table needed to build a correct download response.
struct AssetMeta {
    asset_id: String,
    name: String,
    content_type: String,
    content_hash: String,
}

/// Look up asset metadata and stream its blob bytes back with proper headers.
/// Shared by both `get_node_asset` and `get_asset_by_id` once the caller has
/// already authorized the request.
async fn serve_asset_by_id(db: &Db, blob_store: &BlobStoreHandle, asset_id: &str) -> Response {
    let meta = match direct_load_asset_meta(db, asset_id).await {
        Ok(Some(m)) => m,
        Ok(None) => return error_response(StatusCode::NOT_FOUND, "asset record not found"),
        Err(e) => {
            tracing::error!("asset metadata query failed for asset_id={asset_id}: {e}");
            return error_response(StatusCode::BAD_GATEWAY, "asset metadata query failed");
        }
    };

    // Future-proofing: a directory-shaped asset (many files + a placeholder
    // record) isn't representable as a single byte stream yet. Fail loud and
    // structured instead of serving garbage bytes.
    if meta.content_type == DIRECTORY_ASSET_CONTENT_TYPE {
        return (
            StatusCode::NOT_IMPLEMENTED,
            Json(serde_json::json!({
                "ok": false,
                "error": "directory assets are not yet downloadable as a single file",
                "asset_id": meta.asset_id,
                "content_type": meta.content_type,
            })),
        )
            .into_response();
    }

    let hash = match hash_from_hex(&meta.content_hash) {
        Ok(h) => h,
        Err(e) => {
            tracing::error!(
                "asset asset_id={} has malformed content_hash {:?}: {e}",
                meta.asset_id,
                meta.content_hash
            );
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "corrupt asset record");
        }
    };

    let Some(path) = blob_store.get_blob_path(&hash) else {
        tracing::warn!(
            "blob missing for asset_id={} content_hash={}",
            meta.asset_id,
            meta.content_hash
        );
        return error_response(StatusCode::NOT_FOUND, "asset blob not found in store");
    };

    let data = match read_blob_capped(&path) {
        Ok(d) => d,
        Err(BlobReadError::TooLarge(len)) => {
            tracing::warn!("asset_id={} blob too large to serve ({len} bytes)", meta.asset_id);
            return error_response(
                StatusCode::PAYLOAD_TOO_LARGE,
                "asset exceeds maximum servable size",
            );
        }
        Err(BlobReadError::Io(e)) => {
            tracing::error!("failed reading blob for asset_id={}: {e}", meta.asset_id);
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to read asset blob");
        }
    };

    let mut headers = HeaderMap::new();
    let content_type = HeaderValue::from_str(&meta.content_type)
        .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream"));
    headers.insert(CONTENT_TYPE, content_type);
    if let Ok(len_val) = HeaderValue::from_str(&data.len().to_string()) {
        headers.insert(CONTENT_LENGTH, len_val);
    }
    headers.insert(CONTENT_DISPOSITION, disposition_header_value(&meta.name));

    (StatusCode::OK, headers, data).into_response()
}

/// Build a structured JSON error body with the given HTTP status.
fn error_response(status: StatusCode, msg: &str) -> Response {
    (
        status,
        Json(serde_json::json!({ "ok": false, "error": msg })),
    )
        .into_response()
}

/// Build a safe `Content-Disposition: attachment; filename="..."` header value
/// from a (DB-sourced, not directly user-controlled) asset name.
fn disposition_header_value(name: &str) -> HeaderValue {
    let safe = sanitize_filename(name);
    HeaderValue::from_str(&format!("attachment; filename=\"{safe}\""))
        .unwrap_or_else(|_| HeaderValue::from_static("attachment"))
}

/// Strip header-breaking characters and cap length; never touches the filesystem.
fn sanitize_filename(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .filter(|c| !matches!(c, '"' | '\\' | '\r' | '\n'))
        .take(255)
        .collect();
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        "asset".to_string()
    } else {
        trimmed.to_string()
    }
}

enum BlobReadError {
    TooLarge(u64),
    Io(std::io::Error),
}

impl From<std::io::Error> for BlobReadError {
    fn from(e: std::io::Error) -> Self {
        BlobReadError::Io(e)
    }
}

/// Read a blob file fully into memory, refusing anything over
/// `MAX_ASSET_RESPONSE_BYTES`. Never builds a path from user input — `path`
/// always comes from `BlobStore::get_blob_path`, which is keyed by hash only.
fn read_blob_capped(path: &FsPath) -> Result<Vec<u8>, BlobReadError> {
    let metadata = std::fs::metadata(path)?;
    if metadata.len() > MAX_ASSET_RESPONSE_BYTES {
        return Err(BlobReadError::TooLarge(metadata.len()));
    }
    Ok(std::fs::read(path)?)
}

// ---------------------------------------------------------------------------
// Direct DB read helpers (bypass crossbeam channel; mirror rest.rs pattern)
// ---------------------------------------------------------------------------

/// Outcome of resolving a node's `asset_id`, distinguishing "node missing"
/// from "node exists but carries no asset".
enum NodeAssetLookup {
    NodeNotFound,
    NoAsset,
    Asset(String),
}

async fn direct_resolve_node_asset_id(db: &Db, node_id: &str) -> anyhow::Result<NodeAssetLookup> {
    let mut res = db
        .query("SELECT asset_id FROM node WHERE node_id = $nid LIMIT 1")
        .bind(("nid", node_id.to_string()))
        .await
        .map_err(|e| anyhow::anyhow!("query failed: {e}"))?;
    let rows: Vec<serde_json::Value> = res.take(0).map_err(|e| anyhow::anyhow!("take failed: {e}"))?;
    let Some(row) = rows.first() else {
        return Ok(NodeAssetLookup::NodeNotFound);
    };
    match row.get("asset_id").and_then(|v| v.as_str()) {
        Some(aid) => Ok(NodeAssetLookup::Asset(aid.to_string())),
        None => Ok(NodeAssetLookup::NoAsset),
    }
}

/// Load `asset` table metadata by `asset_id`. Returns `Ok(None)` if absent.
async fn direct_load_asset_meta(db: &Db, asset_id: &str) -> anyhow::Result<Option<AssetMeta>> {
    let mut res = db
        .query(
            "SELECT asset_id, name, content_type, content_hash FROM asset WHERE asset_id = $aid LIMIT 1",
        )
        .bind(("aid", asset_id.to_string()))
        .await
        .map_err(|e| anyhow::anyhow!("query failed: {e}"))?;
    let rows: Vec<serde_json::Value> = res.take(0).map_err(|e| anyhow::anyhow!("take failed: {e}"))?;
    let Some(row) = rows.first() else {
        return Ok(None);
    };
    Ok(Some(AssetMeta {
        asset_id: row["asset_id"].as_str().unwrap_or(asset_id).to_string(),
        name: row["name"].as_str().unwrap_or("asset").to_string(),
        content_type: row["content_type"]
            .as_str()
            .unwrap_or("application/octet-stream")
            .to_string(),
        content_hash: row["content_hash"].as_str().unwrap_or_default().to_string(),
    }))
}

/// Resolve an asset's RBAC scope via the first node referencing it. Assets
/// have no scope column of their own (see AGENTS.md §assets); this is the
/// best available authority until that changes.
async fn direct_resolve_asset_scope(db: &Db, asset_id: &str) -> Option<String> {
    let mut res = db
        .query("SELECT petal_id FROM node WHERE asset_id = $aid LIMIT 1")
        .bind(("aid", asset_id.to_string()))
        .await
        .ok()?;
    let rows: Vec<serde_json::Value> = res.take(0).ok()?;
    let petal_id = rows.first()?.get("petal_id")?.as_str()?;
    crate::rest::direct_resolve_petal_scope(db, petal_id).await
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::sync::Arc;

    /// Same in-memory SurrealDB + schema bootstrap pattern used in `rest.rs` tests.
    async fn setup_test_db() -> surrealdb::Surreal<surrealdb::engine::local::Db> {
        let db = surrealdb::Surreal::new::<surrealdb::engine::local::Mem>(())
            .await
            .expect("in-memory SurrealDB");
        db.use_ns("test").use_db("test").await.expect("ns/db");
        fe_database::schema::apply_all(&db).await.expect("apply schema");
        db
    }

    fn test_claims(scope: &str) -> ApiClaims {
        ApiClaims {
            sub: "did:key:z6MkUser".to_string(),
            scope: scope.to_string(),
            max_role: "editor".to_string(),
            token_type: "api".to_string(),
            iat: 0,
            exp: u64::MAX,
            jti: "jti-test".to_string(),
        }
    }

    fn test_state(db: surrealdb::Surreal<surrealdb::engine::local::Db>) -> Arc<ApiState> {
        let (api_cmd_tx, _) = crossbeam::channel::bounded(1);
        let (transform_broadcast_tx, _) = tokio::sync::broadcast::channel(1);
        let (entity_change_tx, _) = tokio::sync::broadcast::channel(1);
        let verifying_key = ed25519_dalek::VerifyingKey::from_bytes(&[0u8; 32]).unwrap();
        let blob_store: BlobStoreHandle = Arc::new(fe_runtime::blob_store::mock::MockBlobStore::new());

        Arc::new(ApiState {
            api_cmd_tx,
            transform_broadcast_tx,
            entity_change_tx,
            verifying_key,
            revoked_jtis: Arc::new(tokio::sync::RwLock::new(HashSet::new())),
            blob_store: Some(blob_store),
            cors_origins: vec![],
            db_reader: Some(Arc::new(db)),
            query_rate_limiter: tokio::sync::Mutex::new(std::collections::HashMap::new()),
            entity_store: None,
            tileset_registry: None,
            hexon_registry: None,
            announcement_store: None,
        })
    }

    /// Seed a verse/fractal/petal/node/asset chain and return
    /// (node_id, asset_id, content_hash) for the given bytes.
    async fn seed_node_with_asset(
        db: &surrealdb::Surreal<surrealdb::engine::local::Db>,
        blob_store: &BlobStoreHandle,
        bytes: &[u8],
        name: &str,
        content_type: &str,
    ) -> (String, String, String) {
        let now = chrono::Utc::now().to_rfc3339();
        db.query(
            "CREATE verse CONTENT {
                verse_id: 'v1', name: 'V', created_by: 'did:key:z6MkOwner', created_at: $now
            }",
        )
        .bind(("now", now.clone()))
        .await
        .unwrap()
        .check()
        .unwrap();
        db.query(
            "CREATE fractal CONTENT {
                fractal_id: 'f1', verse_id: 'v1', owner_did: 'did:key:z6MkOwner', name: 'F', created_at: $now
            }",
        )
        .bind(("now", now.clone()))
        .await
        .unwrap()
        .check()
        .unwrap();
        db.query(
            "CREATE petal CONTENT {
                petal_id: 'p1', fractal_id: 'f1', name: 'P', node_id: 'anchor-node', created_at: $now
            }",
        )
        .bind(("now", now.clone()))
        .await
        .unwrap()
        .check()
        .unwrap();

        let hash = blob_store.add_blob(bytes).expect("add_blob");
        let content_hash = fe_runtime::blob_store::hash_to_hex(&hash);
        // Real ULIDs (not short literals) so `is_valid_ulid` accepts them in the handler.
        let asset_id = ulid::Ulid::new().to_string();

        db.query(
            "CREATE asset CONTENT {
                asset_id: $asset_id, name: $name, content_type: $content_type,
                size_bytes: $size_bytes, data: NONE, content_hash: $content_hash, created_at: $now
            }",
        )
        .bind(("asset_id", asset_id.clone()))
        .bind(("name", name.to_string()))
        .bind(("content_type", content_type.to_string()))
        .bind(("size_bytes", bytes.len() as i64))
        .bind(("content_hash", content_hash.clone()))
        .bind(("now", now.clone()))
        .await
        .unwrap()
        .check()
        .unwrap();

        let node_id = ulid::Ulid::new().to_string();
        db.query(
            "CREATE node CONTENT {
                node_id: $node_id, petal_id: 'p1', display_name: $name, asset_id: $asset_id,
                position: <geometry<point>> [0.0, 0.0], elevation: 0.0,
                rotation: [0.0, 0.0, 0.0, 1.0], scale: [1.0, 1.0, 1.0],
                interactive: true, created_at: $now
            }",
        )
        .bind(("node_id", node_id.clone()))
        .bind(("name", name.to_string()))
        .bind(("asset_id", asset_id.clone()))
        .bind(("now", now))
        .await
        .unwrap()
        .check()
        .unwrap();

        (node_id, asset_id, content_hash)
    }

    #[tokio::test]
    async fn get_node_asset_round_trips_bytes_and_headers() {
        let db = setup_test_db().await;
        let bytes = b"hello glb bytes".to_vec();
        let blob_store: BlobStoreHandle = Arc::new(fe_runtime::blob_store::mock::MockBlobStore::new());
        let (node_id, _asset_id, _hash) =
            seed_node_with_asset(&db, &blob_store, &bytes, "model.glb", "model/gltf-binary").await;

        let (api_cmd_tx, _) = crossbeam::channel::bounded(1);
        let (transform_broadcast_tx, _) = tokio::sync::broadcast::channel(1);
        let (entity_change_tx, _) = tokio::sync::broadcast::channel(1);
        let verifying_key = ed25519_dalek::VerifyingKey::from_bytes(&[0u8; 32]).unwrap();
        let state = Arc::new(ApiState {
            api_cmd_tx,
            transform_broadcast_tx,
            entity_change_tx,
            verifying_key,
            revoked_jtis: Arc::new(tokio::sync::RwLock::new(HashSet::new())),
            blob_store: Some(blob_store),
            cors_origins: vec![],
            db_reader: Some(Arc::new(db)),
            query_rate_limiter: tokio::sync::Mutex::new(std::collections::HashMap::new()),
            entity_store: None,
            tileset_registry: None,
            hexon_registry: None,
            announcement_store: None,
        });

        let claims = test_claims("VERSE#v1");
        let resp = get_node_asset(
            State(state),
            Extension(claims),
            Path(node_id),
        )
        .await;

        assert_eq!(resp.status(), StatusCode::OK);
        let headers = resp.headers().clone();
        assert_eq!(
            headers.get(CONTENT_TYPE).unwrap(),
            "model/gltf-binary"
        );
        assert_eq!(
            headers.get(CONTENT_DISPOSITION).unwrap(),
            "attachment; filename=\"model.glb\""
        );
        assert_eq!(headers.get(CONTENT_LENGTH).unwrap(), &bytes.len().to_string());

        let body = axum::body::to_bytes(resp.into_body(), 10_000).await.unwrap();
        assert_eq!(body.as_ref(), bytes.as_slice(), "byte-for-byte round trip");
    }

    #[tokio::test]
    async fn get_node_asset_404_for_missing_node() {
        let db = setup_test_db().await;
        let state = test_state(db);
        let claims = test_claims("VERSE#v1");

        let resp = get_node_asset(
            State(state),
            Extension(claims),
            Path("01ARZ3NDEKTSV4RRFFQ69G5FAV".to_string()),
        )
        .await;

        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn get_node_asset_rejects_invalid_node_id() {
        let db = setup_test_db().await;
        let state = test_state(db);
        let claims = test_claims("VERSE#v1");

        let resp = get_node_asset(
            State(state),
            Extension(claims),
            Path("not-a-ulid".to_string()),
        )
        .await;

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn get_asset_by_id_round_trips_and_resolves_scope() {
        let db = setup_test_db().await;
        let bytes = b"asset-by-id bytes".to_vec();
        let blob_store: BlobStoreHandle = Arc::new(fe_runtime::blob_store::mock::MockBlobStore::new());
        let (_node_id, asset_id, _hash) =
            seed_node_with_asset(&db, &blob_store, &bytes, "texture.png", "image/png").await;

        let (api_cmd_tx, _) = crossbeam::channel::bounded(1);
        let (transform_broadcast_tx, _) = tokio::sync::broadcast::channel(1);
        let (entity_change_tx, _) = tokio::sync::broadcast::channel(1);
        let verifying_key = ed25519_dalek::VerifyingKey::from_bytes(&[0u8; 32]).unwrap();
        let state = Arc::new(ApiState {
            api_cmd_tx,
            transform_broadcast_tx,
            entity_change_tx,
            verifying_key,
            revoked_jtis: Arc::new(tokio::sync::RwLock::new(HashSet::new())),
            blob_store: Some(blob_store),
            cors_origins: vec![],
            db_reader: Some(Arc::new(db)),
            query_rate_limiter: tokio::sync::Mutex::new(std::collections::HashMap::new()),
            entity_store: None,
            tileset_registry: None,
            hexon_registry: None,
            announcement_store: None,
        });

        let claims = test_claims("VERSE#v1");
        let resp = get_asset_by_id(State(state), Extension(claims), Path(asset_id)).await;

        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(resp.headers().get(CONTENT_TYPE).unwrap(), "image/png");
        let body = axum::body::to_bytes(resp.into_body(), 10_000).await.unwrap();
        assert_eq!(body.as_ref(), bytes.as_slice());
    }

    #[tokio::test]
    async fn get_asset_by_id_404_for_unknown_asset() {
        let db = setup_test_db().await;
        let state = test_state(db);
        let claims = test_claims("VERSE#v1");

        let resp = get_asset_by_id(
            State(state),
            Extension(claims),
            Path("01ARZ3NDEKTSV4RRFFQ69G5FAV".to_string()),
        )
        .await;

        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn directory_placeholder_content_type_returns_501() {
        let db = setup_test_db().await;
        let blob_store: BlobStoreHandle = Arc::new(fe_runtime::blob_store::mock::MockBlobStore::new());
        let (node_id, _asset_id, _hash) = seed_node_with_asset(
            &db,
            &blob_store,
            b"unused",
            "bucket",
            DIRECTORY_ASSET_CONTENT_TYPE,
        )
        .await;

        let (api_cmd_tx, _) = crossbeam::channel::bounded(1);
        let (transform_broadcast_tx, _) = tokio::sync::broadcast::channel(1);
        let (entity_change_tx, _) = tokio::sync::broadcast::channel(1);
        let verifying_key = ed25519_dalek::VerifyingKey::from_bytes(&[0u8; 32]).unwrap();
        let state = Arc::new(ApiState {
            api_cmd_tx,
            transform_broadcast_tx,
            entity_change_tx,
            verifying_key,
            revoked_jtis: Arc::new(tokio::sync::RwLock::new(HashSet::new())),
            blob_store: Some(blob_store),
            cors_origins: vec![],
            db_reader: Some(Arc::new(db)),
            query_rate_limiter: tokio::sync::Mutex::new(std::collections::HashMap::new()),
            entity_store: None,
            tileset_registry: None,
            hexon_registry: None,
            announcement_store: None,
        });

        let claims = test_claims("VERSE#v1");
        let resp = get_node_asset(State(state), Extension(claims), Path(node_id)).await;

        assert_eq!(resp.status(), StatusCode::NOT_IMPLEMENTED);
    }
}
