//! Axum router + handlers — see `AGENTS.md` §auth model, §routes.

use std::path::PathBuf;
use std::sync::Arc;

use axum::{
    body::{Body, Bytes},
    extract::{DefaultBodyLimit, Path, Query, Request, State},
    http::{header, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use tokio_util::io::ReaderStream;
use tower_http::trace::TraceLayer;

use crate::index::{self, HexonIndexEntry};

/// Max accepted publish body (1 GiB — tilesets are large).
const MAX_PUBLISH_BYTES: usize = 1 << 30;

/// Response envelope matching fe-api's `ApiResponse` shape.
#[derive(Debug, Serialize)]
pub struct ApiResponse<T: Serialize> {
    pub ok: bool,
    pub data: Option<T>,
    pub error: Option<String>,
}

impl<T: Serialize> ApiResponse<T> {
    pub fn success(data: T) -> Self {
        Self {
            ok: true,
            data: Some(data),
            error: None,
        }
    }

    pub fn error(msg: impl Into<String>) -> Self {
        Self {
            ok: false,
            data: None,
            error: Some(msg.into()),
        }
    }
}

/// Shared handler state: config + rescannable in-memory index.
pub struct RegistryState {
    pub dir: PathBuf,
    pub token: Option<String>,
    pub readonly: bool,
    pub index: tokio::sync::RwLock<Vec<HexonIndexEntry>>,
}

impl RegistryState {
    pub fn new(
        dir: PathBuf,
        token: Option<String>,
        readonly: bool,
        entries: Vec<HexonIndexEntry>,
    ) -> Self {
        Self {
            dir,
            token,
            readonly,
            index: tokio::sync::RwLock::new(entries),
        }
    }
}

/// Build the registry router; `/health` is public, `/api/v1/*` is token-gated when configured.
pub fn build_router(state: Arc<RegistryState>) -> Router {
    let api = Router::new()
        .route("/hexons/search", get(search))
        .route(
            "/hexons/publish",
            post(publish).layer(DefaultBodyLimit::max(MAX_PUBLISH_BYTES)),
        )
        .route("/hexons/{uri}", get(manifest))
        .route("/hexons/{uri}/entries", get(entries))
        .route("/hexons/{uri}/entries/{entry_id}/asset", get(asset))
        .route("/hexons/{uri}/download", get(download))
        .layer(middleware::from_fn_with_state(state.clone(), require_token));

    Router::new()
        .route("/health", get(health))
        .nest("/api/v1", api)
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

/// Bearer-token gate for `/api/v1` routes; no-op when no token is configured.
async fn require_token(
    State(state): State<Arc<RegistryState>>,
    req: Request,
    next: Next,
) -> Response {
    if let Some(expected) = &state.token {
        let presented = req
            .headers()
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "));
        if presented != Some(expected.as_str()) {
            return json_err(StatusCode::UNAUTHORIZED, "missing or invalid bearer token");
        }
    }
    next.run(req).await
}

fn json_err(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(ApiResponse::<()>::error(msg))).into_response()
}

async fn health(State(state): State<Arc<RegistryState>>) -> Response {
    let count = state.index.read().await.len();
    Json(ApiResponse::success(
        serde_json::json!({ "status": "ok", "indexed": count }),
    ))
    .into_response()
}

#[derive(Debug, Deserialize)]
struct SearchParams {
    q: Option<String>,
    tags: Option<String>,
    #[serde(rename = "type")]
    hexon_type: Option<String>,
}

/// GET /api/v1/hexons/search?q=&tags=&type= — filter over the in-memory index.
async fn search(
    State(state): State<Arc<RegistryState>>,
    Query(params): Query<SearchParams>,
) -> Response {
    let q = params.q.as_deref().map(str::to_lowercase);
    let tags: Vec<String> = params
        .tags
        .as_deref()
        .map(|t| {
            t.split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default();

    let index = state.index.read().await;
    let hits: Vec<HexonIndexEntry> = index
        .iter()
        .filter(|e| {
            q.as_deref().is_none_or(|q| {
                e.hexon_id.to_lowercase().contains(q)
                    || e.name.to_lowercase().contains(q)
                    || e.description
                        .as_deref()
                        .unwrap_or("")
                        .to_lowercase()
                        .contains(q)
            })
        })
        .filter(|e| tags.iter().all(|t| e.tags.contains(t)))
        .filter(|e| {
            params
                .hexon_type
                .as_deref()
                .is_none_or(|t| e.hexon_type == t)
        })
        .cloned()
        .collect();
    Json(ApiResponse::success(hits)).into_response()
}

/// Resolve `uri` (axum has already percent-decoded the path segment) to an index entry.
async fn resolve_entry(state: &RegistryState, uri: &str) -> Option<HexonIndexEntry> {
    index::resolve(&state.index.read().await, uri).cloned()
}

/// Read a JSON file out of an indexed package's zip on a blocking thread.
async fn zip_json(path: PathBuf, name: &'static str) -> anyhow::Result<serde_json::Value> {
    tokio::task::spawn_blocking(move || index::read_zip_json(&path, name)).await?
}

/// GET /api/v1/hexons/{uri} — raw manifest.json (latest version when unversioned).
async fn manifest(State(state): State<Arc<RegistryState>>, Path(uri): Path<String>) -> Response {
    let Some(entry) = resolve_entry(&state, &uri).await else {
        return json_err(StatusCode::NOT_FOUND, format!("hexon not found: {uri}"));
    };
    match zip_json(entry.path, "manifest.json").await {
        Ok(m) => Json(ApiResponse::success(m)).into_response(),
        Err(e) => json_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

/// GET /api/v1/hexons/{uri}/entries — raw entries.json content.
async fn entries(State(state): State<Arc<RegistryState>>, Path(uri): Path<String>) -> Response {
    let Some(entry) = resolve_entry(&state, &uri).await else {
        return json_err(StatusCode::NOT_FOUND, format!("hexon not found: {uri}"));
    };
    match zip_json(entry.path, "entries.json").await {
        Ok(c) => Json(ApiResponse::success(c)).into_response(),
        Err(e) => json_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

/// GET /api/v1/hexons/{uri}/entries/{entry_id}/asset — stream the entry's blob.
async fn asset(
    State(state): State<Arc<RegistryState>>,
    Path((uri, entry_id)): Path<(String, String)>,
) -> Response {
    let Some(entry) = resolve_entry(&state, &uri).await else {
        return json_err(StatusCode::NOT_FOUND, format!("hexon not found: {uri}"));
    };
    let catalog = match zip_json(entry.path.clone(), "entries.json").await {
        Ok(c) => c,
        Err(e) => return json_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    let hash = catalog
        .get("entries")
        .and_then(|e| e.as_array())
        .and_then(|arr| {
            arr.iter()
                .find(|e| e.get("entry_id").and_then(|i| i.as_str()) == Some(&entry_id))
        })
        .and_then(|e| e.get("asset_hash"))
        .and_then(|h| h.as_str())
        .filter(|h| !h.is_empty())
        .map(str::to_string);
    let Some(hash) = hash else {
        return json_err(
            StatusCode::NOT_FOUND,
            format!("entry not found or has no asset: {entry_id}"),
        );
    };

    let path = entry.path.clone();
    let zip_path = format!("assets/{hash}");
    match tokio::task::spawn_blocking(move || index::read_zip_bytes(&path, &zip_path)).await {
        Ok(Ok(bytes)) => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "application/octet-stream")],
            Bytes::from(bytes),
        )
            .into_response(),
        Ok(Err(_)) => json_err(
            StatusCode::NOT_FOUND,
            format!("blob missing for entry: {entry_id}"),
        ),
        Err(e) => json_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

/// GET /api/v1/hexons/{uri}/download — stream the whole `.hexon` file.
async fn download(State(state): State<Arc<RegistryState>>, Path(uri): Path<String>) -> Response {
    let Some(entry) = resolve_entry(&state, &uri).await else {
        return json_err(StatusCode::NOT_FOUND, format!("hexon not found: {uri}"));
    };
    let file = match tokio::fs::File::open(&entry.path).await {
        Ok(f) => f,
        Err(e) => return json_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    let len = match file.metadata().await {
        Ok(m) => m.len(),
        Err(e) => return json_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/x-hexon+zip")
        .header(header::CONTENT_LENGTH, len)
        .body(Body::from_stream(ReaderStream::new(file)))
        .unwrap_or_else(|e| json_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}

/// Chars allowed in on-disk package names (spec `hexon_id` charset + `+` for semver build meta).
fn safe_component(s: &str, extra: &[char]) -> bool {
    !s.is_empty()
        && s.chars().all(|c| {
            c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '-' || extra.contains(&c)
        })
}

/// POST /api/v1/hexons/publish — validate, persist as `{id}@{version}.hexon`, reindex.
async fn publish(State(state): State<Arc<RegistryState>>, body: Bytes) -> Response {
    if state.readonly {
        return json_err(StatusCode::FORBIDDEN, "registry is read-only");
    }
    if body.is_empty() {
        return json_err(StatusCode::BAD_REQUEST, "empty body");
    }

    // Validate by reading manifest.json from the uploaded zip.
    let bytes = body.clone();
    let manifest = tokio::task::spawn_blocking(move || -> anyhow::Result<serde_json::Value> {
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes.as_ref()))?;
        let mut file = archive.by_name("manifest.json")?;
        let mut buf = String::new();
        std::io::Read::read_to_string(&mut file, &mut buf)?;
        Ok(serde_json::from_str(&buf)?)
    })
    .await;
    let manifest = match manifest {
        Ok(Ok(m)) => m,
        Ok(Err(e)) => return json_err(StatusCode::BAD_REQUEST, format!("invalid .hexon: {e}")),
        Err(e) => return json_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };

    let field = |k: &str| manifest.get(k).and_then(|v| v.as_str()).map(str::to_string);
    let (Some(id), Some(version)) = (field("hexon_id"), field("version")) else {
        return json_err(
            StatusCode::BAD_REQUEST,
            "manifest missing hexon_id or version",
        );
    };
    if field("hexon_type").is_none() || field("publisher_did").is_none() {
        return json_err(
            StatusCode::BAD_REQUEST,
            "manifest missing hexon_type or publisher_did",
        );
    }
    if !safe_component(&id, &[]) || !safe_component(&version, &['+']) {
        return json_err(
            StatusCode::BAD_REQUEST,
            "invalid hexon_id or version characters",
        );
    }

    let dest = state.dir.join(format!("{id}@{version}.hexon"));
    let duplicate = dest.exists()
        || state
            .index
            .read()
            .await
            .iter()
            .any(|e| e.hexon_id == id && e.version == version);
    if duplicate {
        return json_err(
            StatusCode::CONFLICT,
            format!("already published: {id}@{version}"),
        );
    }
    if let Err(e) = tokio::fs::write(&dest, &body).await {
        return json_err(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("write failed: {e}"),
        );
    }

    // Full rescan keeps the index consistent with the directory contents.
    let dir = state.dir.clone();
    match tokio::task::spawn_blocking(move || index::scan_dir(&dir)).await {
        Ok(entries) => {
            tracing::info!(count = entries.len(), published = %format!("{id}@{version}"), "reindexed");
            *state.index.write().await = entries;
        }
        Err(e) => return json_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
    Json(ApiResponse::success(manifest)).into_response()
}
