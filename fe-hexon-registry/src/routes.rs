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
use crate::limits;

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
    /// Serializes publish's check → write → reindex (no continuity TOCTOU).
    publish_lock: tokio::sync::Mutex<()>,
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
            publish_lock: tokio::sync::Mutex::new(()),
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
        .route("/hexons/{uri}/meta", get(meta))
        .route("/hexons/{uri}/entries", get(entries))
        .route("/hexons/{uri}/entries/{entry_id}/asset", get(asset))
        .route("/hexons/{uri}/chunk/{seq}", get(chunk))
        .route("/hexons/{uri}/member/{*path}", get(member))
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
        Err(e) => json_err(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")),
    }
}

/// GET /api/v1/hexons/{uri}/entries — raw entries.json content.
async fn entries(State(state): State<Arc<RegistryState>>, Path(uri): Path<String>) -> Response {
    let Some(entry) = resolve_entry(&state, &uri).await else {
        return json_err(StatusCode::NOT_FOUND, format!("hexon not found: {uri}"));
    };
    match zip_json(entry.path, "entries.json").await {
        Ok(c) => Json(ApiResponse::success(c)).into_response(),
        Err(e) => json_err(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")),
    }
}

/// GET /api/v1/hexons/{uri}/meta — the header-phase document (PAS-P5 §3.2):
/// raw manifest + header-member text, no body/tile bytes. Built from the lazy
/// `HexonArchiveIndex` reader (not `HexonArchive::import`, which is eager), so
/// a multi-hundred-MiB tileset costs one small capped read, not a full load.
async fn meta(State(state): State<Arc<RegistryState>>, Path(uri): Path<String>) -> Response {
    let Some(entry) = resolve_entry(&state, &uri).await else {
        return json_err(StatusCode::NOT_FOUND, format!("hexon not found: {uri}"));
    };
    let result = tokio::task::spawn_blocking(move || {
        fe_format::HexonArchiveIndex::open(&entry.path).map(|index| index.header_doc())
    })
    .await;
    match result {
        Ok(Ok(doc)) => Json(ApiResponse::success(doc)).into_response(),
        Ok(Err(e)) => json_err(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")),
        Err(e) => json_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

/// Octet-stream response for publisher-controlled archive bytes (`/chunk`,
/// `/member`): `nosniff` + `attachment` stop a member that happens to be
/// named e.g. `notes.html` from ever being rendered as same-origin HTML by a
/// browser that follows the link — the stored-XSS rule CDL-SPEC10 A4 already
/// applies to fe-api blobs (S3 critic amendment, step 6).
fn octet_response(bytes: Vec<u8>, filename: &str) -> Response {
    // The name is publisher-controlled: keep only a safe charset inside the quotes.
    let filename: String = filename
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect();
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .header(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{filename}\""),
        )
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
        .body(Body::from(bytes))
        .unwrap_or_else(|e| json_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}

/// GET /api/v1/hexons/{uri}/chunk/{seq} — one container chunk's raw `.hexon`
/// bytes; for a monolithic archive, `seq == 0` is the whole file (so a caller
/// never needs to special-case layout) and anything else is 404.
async fn chunk(
    State(state): State<Arc<RegistryState>>,
    Path((uri, seq)): Path<(String, u32)>,
) -> Response {
    let Some(entry) = resolve_entry(&state, &uri).await else {
        return json_err(StatusCode::NOT_FOUND, format!("hexon not found: {uri}"));
    };
    let path = entry.path.clone();
    let result = tokio::task::spawn_blocking(move || -> anyhow::Result<Option<Vec<u8>>> {
        let index = fe_format::HexonArchiveIndex::open(&path)?;
        match index.layout() {
            fe_format::Layout::Container { .. } => index.chunk(seq),
            fe_format::Layout::Monolithic if seq == 0 => Ok(Some(std::fs::read(&path)?)),
            fe_format::Layout::Monolithic => Ok(None),
        }
    })
    .await;
    match result {
        Ok(Ok(Some(bytes))) => octet_response(bytes, &format!("chunk-{seq}.hexon")),
        Ok(Ok(None)) => json_err(StatusCode::NOT_FOUND, format!("no chunk {seq} for {uri}")),
        Ok(Err(e)) => json_err(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")),
        Err(e) => json_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

/// GET /api/v1/hexons/{uri}/member/{*path} — one raw member, verified against
/// the signed coverage when the archive is sealed (`HexonArchiveIndex::read_member`).
/// Traversal is impossible BY CONSTRUCTION, not by charset-filtering `path`:
/// `path` is only ever compared against the zip's own central-directory member
/// names (`contains`/`read_member`), never joined onto a filesystem path, so a
/// request for e.g. `../../etc/passwd` just fails to match any member and 404s.
async fn member(
    State(state): State<Arc<RegistryState>>,
    Path((uri, path)): Path<(String, String)>,
) -> Response {
    let Some(entry) = resolve_entry(&state, &uri).await else {
        return json_err(StatusCode::NOT_FOUND, format!("hexon not found: {uri}"));
    };
    let file_path = entry.path.clone();
    let member_name = path.clone();
    let result = tokio::task::spawn_blocking(move || -> anyhow::Result<Option<Vec<u8>>> {
        let index = fe_format::HexonArchiveIndex::open(&file_path)?;
        if !index.contains(&member_name) {
            return Ok(None);
        }
        index.read_member(&member_name)
    })
    .await;
    match result {
        Ok(Ok(Some(bytes))) => {
            let filename = path.rsplit('/').next().unwrap_or(&path);
            octet_response(bytes, filename)
        }
        Ok(Ok(None)) => json_err(StatusCode::NOT_FOUND, format!("no member {path} in {uri}")),
        Ok(Err(e)) => json_err(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")),
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
        Err(e) => return json_err(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")),
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
        // A stored blob over the decompression limits is a server-side content fault, not a 404.
        Ok(Err(e)) if format!("{e:#}").contains(limits::DECOMPRESSION_LIMIT_ERROR) => {
            json_err(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}"))
        }
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

/// Create `dest` with `bytes`, refusing (AlreadyExists) to overwrite a file
/// that appeared after the duplicate check.
async fn write_new(dest: &std::path::Path, bytes: Bytes) -> std::io::Result<()> {
    let dest = dest.to_path_buf();
    tokio::task::spawn_blocking(move || {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&dest)?;
        file.write_all(&bytes)?;
        file.sync_all()
    })
    .await
    .map_err(std::io::Error::other)?
}

/// POST /api/v1/hexons/publish — validate, persist as `{id}@{version}.hexon`, reindex.
///
/// D-75/D-76: every publish must be a fully sealed, signed archive (`verify_archive`
/// under `RequireSealed`) — an unsealed archive, a bad signature, or any member
/// (including a self-certifying `assets/{hash}`) whose bytes no longer match
/// their committed digest all get 400. A `hexon_id` already indexed under a
/// different (cryptographically verified) publisher DID gets 409: a second
/// key may not "take over" someone else's id (AP-3.4 publish ownership).
async fn publish(State(state): State<Arc<RegistryState>>, body: Bytes) -> Response {
    if state.readonly {
        return json_err(StatusCode::FORBIDDEN, "registry is read-only");
    }
    if body.is_empty() {
        return json_err(StatusCode::BAD_REQUEST, "empty body");
    }

    // Validate the manifest and bound every entry's decompression (zip-bomb guard).
    let bytes = body.clone();
    let manifest = tokio::task::spawn_blocking(move || index::validate_package(&bytes)).await;
    let manifest = match manifest {
        Ok(Ok(m)) => m,
        Ok(Err(e)) => return json_err(StatusCode::BAD_REQUEST, format!("invalid .hexon: {e:#}")),
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

    // The one gate that lets everything below trust `publisher_did`: it is
    // self-certifying (R6 — the DID names the key, not the key's trustworthiness),
    // so only a passing signature check makes it safe to use for the continuity
    // decision just below.
    let verify_bytes = body.clone();
    let publisher_did = match tokio::task::spawn_blocking(move || {
        fe_format::verify_archive(
            &verify_bytes,
            &fe_format::IntegrityPolicy::RequireSealed { pin: None },
        )
    })
    .await
    {
        Ok(Ok(fe_format::IntegrityStatus::Sealed { publisher_did })) => publisher_did,
        Ok(Ok(fe_format::IntegrityStatus::Unsealed)) => {
            // Unreachable: `RequireSealed` never resolves `verify_sealed_header`
            // to `SealCheck::Unsealed`, so `verify_archive` never returns this
            // variant under this policy (fe-format/src/integrity.rs).
            return json_err(
                StatusCode::INTERNAL_SERVER_ERROR,
                "sealed status invariant broken",
            );
        }
        Ok(Err(e)) => return json_err(StatusCode::BAD_REQUEST, format!("{e}")),
        Err(e) => return json_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };

    // Held until the reindex below: two concurrent publishes of one id can
    // never both pass the duplicate/continuity check (see AGENTS.md §Publish validation).
    let _publishing = state.publish_lock.lock().await;
    let dest = state.dir.join(format!("{id}@{version}.hexon"));
    let conflict = {
        let index = state.index.read().await;
        if dest.exists()
            || index
                .iter()
                .any(|e| e.hexon_id == id && e.version == version)
        {
            Some(format!("already published: {id}@{version}"))
        } else {
            index
                .iter()
                .find(|e| e.hexon_id == id && e.publisher_did != publisher_did)
                .map(|other| {
                    format!(
                        "{id} is already published by {}, not {publisher_did}",
                        other.publisher_did
                    )
                })
        }
    };
    if let Some(msg) = conflict {
        return json_err(StatusCode::CONFLICT, msg);
    }
    if let Err(e) = write_new(&dest, body.clone()).await {
        if e.kind() == std::io::ErrorKind::AlreadyExists {
            return json_err(
                StatusCode::CONFLICT,
                format!("already published: {id}@{version}"),
            );
        }
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
