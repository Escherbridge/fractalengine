//! Axum router + handlers — see `AGENTS.md` §auth model, §routes.

use std::path::PathBuf;
use std::sync::Arc;

use axum::{
    body::{Body, Bytes},
    extract::{rejection::BytesRejection, DefaultBodyLimit, Path, Query, Request, State},
    http::{header, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio_util::io::ReaderStream;
use tower_http::trace::TraceLayer;

use crate::config::DEFAULT_MAX_PUBLISH_BYTES;
use crate::guard::{ArchiveGuard, DEFAULT_ARCHIVE_CONCURRENCY};
use crate::index::{self, HexonIndexEntry};
use crate::limits;
use crate::store::{self, FsPackageStore, PackageStore, StoreError};

/// Whole-request deadline for non-publish routes (a streamed body is not cut once headers are out).
const READ_DEADLINE: std::time::Duration = std::time::Duration::from_secs(30);
/// Whole-request deadline for publish, body upload included.
const PUBLISH_DEADLINE: std::time::Duration = std::time::Duration::from_secs(300);
/// Requests in flight across the whole router before new ones get 503.
const MAX_INFLIGHT_REQUESTS: usize = 256;

/// Registry protocol version advertised by `/health` and `/config.json`.
pub const REGISTRY_PROTOCOL: u64 = 1;
/// Header carrying [`REGISTRY_PROTOCOL`] on every response (clients never require it; `config.json` is the authority).
const PROTOCOL_HEADER: &str = "x-hexon-registry-protocol";

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
    /// Where archives and catalogue documents live.
    pub store: Arc<dyn PackageStore>,
    pub token: Option<String>,
    /// Token gating only `publish`; when set, the read token does not authorize it.
    pub publish_token: Option<String>,
    pub readonly: bool,
    pub index: tokio::sync::RwLock<Vec<HexonIndexEntry>>,
    /// Operator opt-in: `/blobs` serves every stored archive, listed in the catalogue or not.
    pub public_blobs: bool,
    /// Serializes publish's check → write → reindex (no continuity TOCTOU).
    publish_lock: tokio::sync::Mutex<()>,
    /// Archive read permits + parsed-index cache (`AGENTS.md` §hardening).
    pub archive: ArchiveGuard,
    /// Largest accepted publish body, bytes.
    pub max_publish_bytes: usize,
    inflight: Arc<Semaphore>,
}

impl RegistryState {
    /// Filesystem-backed state over `dir` (the original constructor; callers keep working).
    pub fn new(
        dir: PathBuf,
        token: Option<String>,
        readonly: bool,
        entries: Vec<HexonIndexEntry>,
    ) -> Self {
        Self::with_store(Arc::new(FsPackageStore::new(dir)), token, readonly, entries)
    }

    /// State over any [`PackageStore`]; `entries` is its initial `list()`.
    pub fn with_store(
        store: Arc<dyn PackageStore>,
        token: Option<String>,
        readonly: bool,
        entries: Vec<HexonIndexEntry>,
    ) -> Self {
        Self {
            store,
            token,
            publish_token: None,
            readonly,
            index: tokio::sync::RwLock::new(entries),
            public_blobs: false,
            publish_lock: tokio::sync::Mutex::new(()),
            archive: ArchiveGuard::new(DEFAULT_ARCHIVE_CONCURRENCY),
            max_publish_bytes: DEFAULT_MAX_PUBLISH_BYTES,
            inflight: Arc::new(Semaphore::new(MAX_INFLIGHT_REQUESTS)),
        }
    }

    /// Gate `publish` with its own bearer token (see `AGENTS.md` §auth).
    pub fn with_publish_token(mut self, publish_token: Option<String>) -> Self {
        self.publish_token = publish_token;
        self
    }

    /// Concurrent archive reads (default 4); drops the parsed-index cache.
    pub fn with_archive_concurrency(mut self, permits: usize) -> Self {
        self.archive = ArchiveGuard::new(permits);
        self
    }

    /// Largest accepted publish body in bytes (default 64 MiB).
    pub fn with_max_publish_bytes(mut self, bytes: usize) -> Self {
        self.max_publish_bytes = bytes;
        self
    }

    /// Serve every stored archive on `/blobs` (default off: only catalogue-listed builds are anonymous).
    pub fn with_public_blobs(mut self, public_blobs: bool) -> Self {
        self.public_blobs = public_blobs;
        self
    }

    /// True when the request carries this registry's bearer token (never true without one).
    fn bearer_matches(&self, headers: &header::HeaderMap) -> bool {
        self.token.as_deref().is_some_and(|expected| bearer_ok(headers, expected))
    }
}

/// Build the registry router; `/health` and the catalogue (`/config.json` alias, `/api/v1/catalog/*`)
/// are public, `/blobs/*` is public only for catalogue-listed builds, `/api/v1/hexons/*` reads are gated by
/// the read token and `publish` by the publish token (else the read token).
pub fn build_router(state: Arc<RegistryState>) -> Router {
    // Publish has its own gate, layered on the route so it runs before the body extractor reads a byte.
    let publish_route = Router::new()
        .route(
            "/hexons/publish",
            post(publish).layer(DefaultBodyLimit::max(state.max_publish_bytes)),
        )
        .layer(middleware::from_fn_with_state(state.clone(), require_publish))
        .layer(middleware::from_fn(publish_deadline));

    let reads = Router::new()
        .route("/hexons/search", get(search))
        .route("/hexons/{uri}", get(manifest))
        .route("/hexons/{uri}/meta", get(meta))
        .route("/hexons/{uri}/entries", get(entries))
        .route("/hexons/{uri}/entries/{entry_id}/asset", get(asset))
        .route("/hexons/{uri}/chunk/{seq}", get(chunk))
        .route("/hexons/{uri}/member/{*path}", get(member))
        .route("/hexons/{uri}/download", get(download))
        .layer(middleware::from_fn_with_state(state.clone(), require_token))
        .layer(middleware::from_fn(read_deadline));
    let api = reads.merge(publish_route);

    Router::new()
        .route("/health", get(health))
        .route("/config.json", get(config_doc))
        .route("/api/v1/catalog/{*path}", get(catalog_doc))
        .route("/blobs/{file}", get(blob))
        .nest("/api/v1", api)
        .layer(middleware::from_fn_with_state(state.clone(), limit_inflight))
        .layer(middleware::map_response(stamp_protocol))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

/// Stamp [`PROTOCOL_HEADER`] on every response, including token refusals.
async fn stamp_protocol<B>(mut response: Response<B>) -> Response<B> {
    response
        .headers_mut()
        .insert(PROTOCOL_HEADER, header::HeaderValue::from(REGISTRY_PROTOCOL));
    response
}

/// Global concurrency cap: 503 + `Retry-After` when too many requests are in flight.
async fn limit_inflight(
    State(state): State<Arc<RegistryState>>,
    req: Request,
    next: Next,
) -> Response {
    match Arc::clone(&state.inflight).try_acquire_owned() {
        Ok(_slot) => next.run(req).await,
        Err(_) => busy(),
    }
}

/// Run `next` under `deadline`; a late handler answers 408.
async fn with_deadline(deadline: std::time::Duration, req: Request, next: Next) -> Response {
    match tokio::time::timeout(deadline, next.run(req)).await {
        Ok(response) => response,
        Err(_) => json_err(StatusCode::REQUEST_TIMEOUT, "request_timeout"),
    }
}

async fn read_deadline(req: Request, next: Next) -> Response {
    with_deadline(READ_DEADLINE, req, next).await
}

async fn publish_deadline(req: Request, next: Next) -> Response {
    with_deadline(PUBLISH_DEADLINE, req, next).await
}

/// Bearer-token gate for `/api/v1` routes; no-op when no token is configured.
async fn require_token(
    State(state): State<Arc<RegistryState>>,
    req: Request,
    next: Next,
) -> Response {
    if let Some(expected) = &state.token {
        if !bearer_ok(req.headers(), expected) {
            return json_err(StatusCode::UNAUTHORIZED, "missing or invalid bearer token");
        }
    }
    next.run(req).await
}

/// Publish gate: read-only wins (403), then the publish token (else the read token) must match.
async fn require_publish(
    State(state): State<Arc<RegistryState>>,
    req: Request,
    next: Next,
) -> Response {
    if state.readonly {
        return json_err(StatusCode::FORBIDDEN, "registry is read-only");
    }
    if let Some(expected) = state.publish_token.as_ref().or(state.token.as_ref()) {
        if !bearer_ok(req.headers(), expected) {
            return json_err(StatusCode::UNAUTHORIZED, "missing or invalid bearer token");
        }
    }
    let declared = req
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());
    if declared.is_some_and(|len| len > state.max_publish_bytes as u64) {
        return json_err(StatusCode::PAYLOAD_TOO_LARGE, "payload_too_large");
    }
    next.run(req).await
}

/// True when `Authorization: Bearer <x>` carries exactly `expected` (constant-time compare).
fn bearer_ok(headers: &header::HeaderMap, expected: &str) -> bool {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .is_some_and(|presented| constant_time_eq(presented.as_bytes(), expected.as_bytes()))
}

/// Length-checked XOR fold: no early exit on the first differing byte.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn json_err(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(ApiResponse::<()>::error(msg))).into_response()
}

async fn health(State(state): State<Arc<RegistryState>>) -> Response {
    let count = state.index.read().await.len();
    Json(ApiResponse::success(serde_json::json!({
        "status": "ok",
        "indexed": count,
        "protocol_version": REGISTRY_PROTOCOL,
    })))
    .into_response()
}

/// Raw JSON document response (catalogue documents are served byte-for-byte, never re-serialised).
fn json_doc(bytes: Bytes) -> Response {
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
        .body(Body::from(bytes))
        .unwrap_or_else(|e| internal("response build failed", &e))
}

/// Contract v1 error envelope with a stable snake_case code.
fn coded_err(status: StatusCode, code: &'static str) -> Response {
    json_err(status, code)
}

fn store_failure(e: StoreError) -> Response {
    tracing::warn!(error = %e, "catalogue store read failed");
    coded_err(StatusCode::INTERNAL_SERVER_ERROR, "internal_error")
}

/// GET /config.json and /api/v1/catalog/config.json — the store's own `config.json` when it has one,
/// else a generated bootstrap that advertises catalogue features only while an `index.json` exists.
async fn config_doc(State(state): State<Arc<RegistryState>>) -> Response {
    match state.store.catalog_doc("config.json").await {
        Ok(Some(bytes)) => return json_doc(bytes),
        Ok(None) => {}
        Err(e) => return store_failure(e),
    }
    let has_index = match state.store.catalog_doc("index.json").await {
        Ok(doc) => doc.is_some(),
        Err(e) => return store_failure(e),
    };
    let features: &[&str] = if has_index {
        &["index", "detail", "attestations"]
    } else {
        &[]
    };
    Json(serde_json::json!({
        "schema_version": 1,
        "registry_protocol": REGISTRY_PROTOCOL,
        "kinds": ["extension", "web-bundle"],
        "features": features,
    }))
    .into_response()
}

/// GET /api/v1/catalog/{*path} — one catalogue document from the store, outside the token gate
/// (contract v1 rule 1.3: catalogue GETs carry no credentials).
async fn catalog_doc(
    State(state): State<Arc<RegistryState>>,
    Path(path): Path<String>,
) -> Response {
    if path == "config.json" {
        return config_doc(State(state)).await;
    }
    if !store::valid_catalog_path(&path) {
        return coded_err(StatusCode::NOT_FOUND, "not_found");
    }
    match state.store.catalog_doc(&path).await {
        Ok(Some(bytes)) => json_doc(bytes),
        Ok(None) => coded_err(StatusCode::NOT_FOUND, "not_found"),
        Err(e) => store_failure(e),
    }
}

/// GET /blobs/{content_hash}.hexon — anonymous only for a build the published catalogue lists (or
/// with the operator's `public_blobs` opt-in); otherwise the bearer token is required. Unknown,
/// unlisted and unauthorised hashes all get the same 404 (no existence oracle).
async fn blob(
    State(state): State<Arc<RegistryState>>,
    Path(file): Path<String>,
    headers: header::HeaderMap,
) -> Response {
    let not_found = || coded_err(StatusCode::NOT_FOUND, "not_found");
    let hash = file.strip_suffix(".hexon").unwrap_or("");
    let well_formed =
        hash.len() == 64 && hash.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
    if !well_formed {
        return not_found();
    }
    let found = state
        .index
        .read()
        .await
        .iter()
        .find(|e| e.content_hash == hash)
        .map(|e| (e.path.clone(), e.hexon_id.clone()));
    let Some((path, hexon_id)) = found else {
        return not_found();
    };
    let admitted = state.public_blobs
        || state.bearer_matches(&headers)
        || match catalog_lists_build(state.store.as_ref(), &hexon_id, hash).await {
            Ok(listed) => listed,
            Err(e) => return store_failure(e),
        };
    if admitted {
        stream_hexon(&path, &file).await
    } else {
        not_found()
    }
}

/// True when `index.json` lists `package_id` and its detail document lists a version with `content_hash`.
async fn catalog_lists_build(
    store: &dyn PackageStore,
    package_id: &str,
    content_hash: &str,
) -> Result<bool, StoreError> {
    let parse = |bytes: Option<Bytes>| {
        bytes.and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
    };
    let listed_in_index = parse(store.catalog_doc("index.json").await?)
        .and_then(|doc| doc.get("entries")?.as_array().cloned())
        .is_some_and(|entries| {
            entries
                .iter()
                .any(|e| e.get("package_id").and_then(|v| v.as_str()) == Some(package_id))
        });
    if !listed_in_index {
        return Ok(false);
    }
    let detail_path = format!("packages/{package_id}.json");
    if !store::valid_catalog_path(&detail_path) {
        return Ok(false);
    }
    Ok(parse(store.catalog_doc(&detail_path).await?)
        .and_then(|doc| doc.get("versions")?.as_array().cloned())
        .is_some_and(|versions| {
            versions
                .iter()
                .any(|v| v.get("content_hash").and_then(|h| h.as_str()) == Some(content_hash))
        }))
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

/// Reply to a failed archive read: details to the log, a stable code to the client.
fn read_failure(e: &anyhow::Error) -> Response {
    let text = format!("{e:#}");
    tracing::error!(error = %text, "archive read failed");
    if text.contains(limits::DECOMPRESSION_LIMIT_ERROR) {
        json_err(StatusCode::INTERNAL_SERVER_ERROR, limits::DECOMPRESSION_LIMIT_ERROR)
    } else {
        json_err(StatusCode::INTERNAL_SERVER_ERROR, "internal_error")
    }
}

/// Log `detail` and answer a stable `internal_error` (no paths or I/O text reach the client).
fn internal(context: &str, detail: &dyn std::fmt::Display) -> Response {
    tracing::error!(error = %detail, "{context}");
    json_err(StatusCode::INTERNAL_SERVER_ERROR, "internal_error")
}

/// 503 with `Retry-After`: every archive permit (or request slot) is in use.
fn busy() -> Response {
    let mut response = json_err(StatusCode::SERVICE_UNAVAILABLE, "busy");
    response
        .headers_mut()
        .insert(header::RETRY_AFTER, header::HeaderValue::from_static("2"));
    response
}

/// One archive permit, or the 503 to return.
async fn archive_permit(state: &RegistryState) -> Result<OwnedSemaphorePermit, Response> {
    state.archive.acquire().await.ok_or_else(busy)
}

/// Run blocking archive work holding `permit`; the permit comes back with the result so the
/// response body can keep it until the bytes are drained.
async fn run_blocking<T: Send + 'static>(
    permit: OwnedSemaphorePermit,
    work: impl FnOnce() -> anyhow::Result<T> + Send + 'static,
) -> anyhow::Result<(OwnedSemaphorePermit, T)> {
    match tokio::task::spawn_blocking(move || (permit, work())).await {
        Ok((permit, Ok(value))) => Ok((permit, value)),
        Ok((_, Err(e))) => Err(e),
        Err(e) => Err(anyhow::anyhow!("blocking task failed: {e}")),
    }
}

/// `AsyncRead` over buffered bytes that keeps its archive permit until the body is dropped.
struct HeldRead {
    data: std::io::Cursor<Vec<u8>>,
    _permit: OwnedSemaphorePermit,
}

impl tokio::io::AsyncRead for HeldRead {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        tokio::io::AsyncRead::poll_read(std::pin::Pin::new(&mut self.data), cx, buf)
    }
}

/// Buffered bytes as a body whose archive permit lives until the client has drained it.
fn held_response(
    permit: OwnedSemaphorePermit,
    bytes: Vec<u8>,
    content_type: &'static str,
    disposition: Option<String>,
) -> Response {
    let len = bytes.len();
    let body = Body::from_stream(ReaderStream::new(HeldRead {
        data: std::io::Cursor::new(bytes),
        _permit: permit,
    }));
    let mut builder = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CONTENT_LENGTH, len)
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff");
    if let Some(disposition) = disposition {
        builder = builder.header(header::CONTENT_DISPOSITION, disposition);
    }
    builder
        .body(body)
        .unwrap_or_else(|e| internal("response build failed", &e))
}

/// `{ok, data}` envelope serialised up front so the permit can ride the body.
fn json_held<T: Serialize>(permit: OwnedSemaphorePermit, data: T) -> Response {
    match serde_json::to_vec(&ApiResponse::success(data)) {
        Ok(bytes) => held_response(permit, bytes, "application/json", None),
        Err(e) => internal("json serialisation failed", &e),
    }
}

/// GET /api/v1/hexons/{uri} — raw manifest.json (latest version when unversioned).
async fn manifest(State(state): State<Arc<RegistryState>>, Path(uri): Path<String>) -> Response {
    json_member(&state, &uri, "manifest.json").await
}

/// GET /api/v1/hexons/{uri}/entries — raw entries.json content.
async fn entries(State(state): State<Arc<RegistryState>>, Path(uri): Path<String>) -> Response {
    json_member(&state, &uri, "entries.json").await
}

/// Serve one JSON member of an indexed package inside the `{ok,data}` envelope.
async fn json_member(state: &RegistryState, uri: &str, name: &'static str) -> Response {
    let Some(entry) = resolve_entry(state, uri).await else {
        return json_err(StatusCode::NOT_FOUND, format!("hexon not found: {uri}"));
    };
    let permit = match archive_permit(state).await {
        Ok(p) => p,
        Err(r) => return r,
    };
    match run_blocking(permit, move || index::read_zip_json(&entry.path, name)).await {
        Ok((permit, value)) => json_held(permit, value),
        Err(e) => read_failure(&e),
    }
}

/// GET /api/v1/hexons/{uri}/meta — the header-phase document (PAS-P5 §3.2):
/// raw manifest + header-member text, no body/tile bytes, from the cached lazy `HexonArchiveIndex`.
async fn meta(State(state): State<Arc<RegistryState>>, Path(uri): Path<String>) -> Response {
    let Some(entry) = resolve_entry(&state, &uri).await else {
        return json_err(StatusCode::NOT_FOUND, format!("hexon not found: {uri}"));
    };
    let permit = match archive_permit(&state).await {
        Ok(p) => p,
        Err(r) => return r,
    };
    let reader = Arc::clone(&state);
    match run_blocking(permit, move || {
        Ok(reader.archive.open(&entry.path)?.header_doc())
    })
    .await
    {
        Ok((permit, doc)) => json_held(permit, doc),
        Err(e) => read_failure(&e),
    }
}

/// Keep only a safe charset inside a `Content-Disposition` filename — the
/// name may be publisher- or caller-controlled (a `/chunk`/`/member` name, an
/// entries.json `asset_hash`, or `/download`'s requested `uri`), so anything
/// outside `[A-Za-z0-9._-]` is replaced (S8: cannot shape the header).
fn safe_filename(raw: &str) -> String {
    raw.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Octet-stream response for publisher-controlled archive bytes (`/chunk`,
/// `/member`, `/entries/{id}/asset`): `nosniff` + `attachment` stop a member
/// that happens to be named e.g. `notes.html` from ever being rendered as
/// same-origin HTML by a browser that follows the link — the stored-XSS rule
/// CDL-SPEC10 A4 already applies to fe-api blobs (S3 critic amendment, step 6).
fn octet_response(permit: OwnedSemaphorePermit, bytes: Vec<u8>, filename: &str) -> Response {
    let filename = safe_filename(filename);
    held_response(
        permit,
        bytes,
        "application/octet-stream",
        Some(format!("attachment; filename=\"{filename}\"")),
    )
}

/// What `/chunk/{seq}` resolved to.
enum ChunkOut {
    Bytes(Vec<u8>),
    /// Monolithic archive, `seq == 0`: the whole file, streamed.
    WholeFile,
    Missing,
}

/// GET /api/v1/hexons/{uri}/chunk/{seq} — one container chunk's raw `.hexon`
/// bytes; for a monolithic archive, `seq == 0` is the whole file (streamed, never buffered)
/// and anything else is 404.
async fn chunk(
    State(state): State<Arc<RegistryState>>,
    Path((uri, seq)): Path<(String, u32)>,
) -> Response {
    let Some(entry) = resolve_entry(&state, &uri).await else {
        return json_err(StatusCode::NOT_FOUND, format!("hexon not found: {uri}"));
    };
    let permit = match archive_permit(&state).await {
        Ok(p) => p,
        Err(r) => return r,
    };
    let path = entry.path.clone();
    let reader = Arc::clone(&state);
    let filename = format!("chunk-{seq}.hexon");
    match run_blocking(permit, move || {
        let index = reader.archive.open(&path)?;
        Ok(match index.layout() {
            fe_format::Layout::Container { .. } => index
                .chunk(seq)?
                .map_or(ChunkOut::Missing, ChunkOut::Bytes),
            fe_format::Layout::Monolithic if seq == 0 => ChunkOut::WholeFile,
            fe_format::Layout::Monolithic => ChunkOut::Missing,
        })
    })
    .await
    {
        Ok((permit, ChunkOut::Bytes(bytes))) => octet_response(permit, bytes, &filename),
        Ok((_, ChunkOut::WholeFile)) => {
            stream_file(&entry.path, &filename, "application/octet-stream").await
        }
        Ok((_, ChunkOut::Missing)) => {
            json_err(StatusCode::NOT_FOUND, format!("no chunk {seq} for {uri}"))
        }
        Err(e) => read_failure(&e),
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
    let permit = match archive_permit(&state).await {
        Ok(p) => p,
        Err(r) => return r,
    };
    let file_path = entry.path.clone();
    let member_name = path.clone();
    let reader = Arc::clone(&state);
    match run_blocking(permit, move || {
        let index = reader.archive.open(&file_path)?;
        if !index.contains(&member_name) {
            return Ok(None);
        }
        index.read_member(&member_name)
    })
    .await
    {
        Ok((permit, Some(bytes))) => {
            let filename = path.rsplit('/').next().unwrap_or(&path);
            octet_response(permit, bytes, filename)
        }
        Ok((_, None)) => json_err(StatusCode::NOT_FOUND, format!("no member {path} in {uri}")),
        Err(e) => read_failure(&e),
    }
}

/// What `/entries/{id}/asset` resolved to.
enum AssetOut {
    NoEntry,
    Blob(Vec<u8>),
    BlobMissing,
}

/// GET /api/v1/hexons/{uri}/entries/{entry_id}/asset — the entry's blob.
async fn asset(
    State(state): State<Arc<RegistryState>>,
    Path((uri, entry_id)): Path<(String, String)>,
) -> Response {
    let Some(entry) = resolve_entry(&state, &uri).await else {
        return json_err(StatusCode::NOT_FOUND, format!("hexon not found: {uri}"));
    };
    let permit = match archive_permit(&state).await {
        Ok(p) => p,
        Err(r) => return r,
    };
    let wanted = entry_id.clone();
    match run_blocking(permit, move || {
        let catalog = index::read_zip_json(&entry.path, "entries.json")?;
        let hash = catalog
            .get("entries")
            .and_then(|e| e.as_array())
            .and_then(|arr| {
                arr.iter()
                    .find(|e| e.get("entry_id").and_then(|i| i.as_str()) == Some(&wanted))
            })
            .and_then(|e| e.get("asset_hash"))
            .and_then(|h| h.as_str())
            .filter(|h| !h.is_empty())
            .map(str::to_string);
        let Some(hash) = hash else {
            return Ok(AssetOut::NoEntry);
        };
        match index::read_zip_bytes(&entry.path, &format!("assets/{hash}")) {
            Ok(bytes) => Ok(AssetOut::Blob(bytes)),
            // A stored blob over the decompression limits is a server-side content fault, not a 404.
            Err(e) if format!("{e:#}").contains(limits::DECOMPRESSION_LIMIT_ERROR) => Err(e),
            Err(_) => Ok(AssetOut::BlobMissing),
        }
    })
    .await
    {
        // W2b F6: the same nosniff+attachment protection `/chunk` and `/member` carry.
        Ok((permit, AssetOut::Blob(bytes))) => {
            octet_response(permit, bytes, &format!("asset-{entry_id}"))
        }
        Ok((_, AssetOut::NoEntry)) => json_err(
            StatusCode::NOT_FOUND,
            format!("entry not found or has no asset: {entry_id}"),
        ),
        Ok((_, AssetOut::BlobMissing)) => json_err(
            StatusCode::NOT_FOUND,
            format!("blob missing for entry: {entry_id}"),
        ),
        Err(e) => read_failure(&e),
    }
}

/// GET /api/v1/hexons/{uri}/download — stream the whole `.hexon` file.
async fn download(State(state): State<Arc<RegistryState>>, Path(uri): Path<String>) -> Response {
    let Some(entry) = resolve_entry(&state, &uri).await else {
        return json_err(StatusCode::NOT_FOUND, format!("hexon not found: {uri}"));
    };
    stream_hexon(&entry.path, &format!("{uri}.hexon")).await
}

/// Stream a `.hexon` file as a download (W2b F6; streamed, so nothing is buffered).
async fn stream_hexon(path: &std::path::Path, filename: &str) -> Response {
    stream_file(path, filename, "application/x-hexon+zip").await
}

/// Stream a file with nosniff + attachment headers (constant memory, no archive permit needed).
async fn stream_file(
    path: &std::path::Path,
    filename: &str,
    content_type: &'static str,
) -> Response {
    let file = match tokio::fs::File::open(path).await {
        Ok(f) => f,
        Err(e) => return internal("open failed", &e),
    };
    let len = match file.metadata().await {
        Ok(m) => m.len(),
        Err(e) => return internal("stat failed", &e),
    };
    let filename = safe_filename(filename);
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CONTENT_LENGTH, len)
        .header(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{filename}\""),
        )
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
        .body(Body::from_stream(ReaderStream::new(file)))
        .unwrap_or_else(|e| internal("response build failed", &e))
}

/// Chars allowed in on-disk package names (spec `hexon_id` charset + `+` for semver build meta).
fn safe_component(s: &str, extra: &[char]) -> bool {
    !s.is_empty()
        && s.chars().all(|c| {
            c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '-' || extra.contains(&c)
        })
}

/// Store-client contract v1 package id: `^[A-Za-z0-9][A-Za-z0-9_.-]{0,127}$`, not ending `.json`.
fn contract_package_id(id: &str) -> bool {
    (1..=128).contains(&id.len())
        && id.starts_with(|c: char| c.is_ascii_alphanumeric())
        && safe_component(id, &[])
        && !id.to_ascii_lowercase().ends_with(".json")
}

/// POST /api/v1/hexons/publish — validate, persist as `{id}@{version}.hexon`, reindex.
///
/// D-75/D-76: every publish must be a fully sealed, signed archive (`verify_archive`
/// under `RequireSealed`) — an unsealed archive, a bad signature, or any member
/// (including a self-certifying `assets/{hash}`) whose bytes no longer match
/// their committed digest all get 400. A `hexon_id` already indexed under a
/// different (cryptographically verified) publisher DID gets 409: a second
/// key may not "take over" someone else's id (AP-3.4 publish ownership).
async fn publish(
    State(state): State<Arc<RegistryState>>,
    body: Result<Bytes, BytesRejection>,
) -> Response {
    if state.readonly {
        return json_err(StatusCode::FORBIDDEN, "registry is read-only");
    }
    let body = match body {
        Ok(body) => body,
        Err(rejection) => {
            let status = rejection.into_response().status();
            return if status == StatusCode::PAYLOAD_TOO_LARGE {
                json_err(StatusCode::PAYLOAD_TOO_LARGE, "payload_too_large")
            } else {
                json_err(StatusCode::BAD_REQUEST, "unreadable_body")
            };
        }
    };
    if body.is_empty() {
        return json_err(StatusCode::BAD_REQUEST, "empty body");
    }

    // Validate the manifest and bound every entry's decompression (zip-bomb guard).
    let bytes = body.clone();
    let manifest = tokio::task::spawn_blocking(move || index::validate_package(&bytes)).await;
    let manifest = match manifest {
        Ok(Ok(m)) => m,
        Ok(Err(e)) => return json_err(StatusCode::BAD_REQUEST, format!("invalid .hexon: {e:#}")),
        Err(e) => return internal("validate task failed", &e),
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
    if !contract_package_id(&id)
        || !safe_component(&version, &[])
        || !index::valid_release_version(&version)
    {
        return json_err(
            StatusCode::BAD_REQUEST,
            "invalid hexon_id, or version is not SemVer without build metadata",
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
        Err(e) => return internal("verify task failed", &e),
    };

    // Held until the reindex below: two concurrent publishes of one id can
    // never both pass the duplicate/continuity check (see AGENTS.md §Publish validation).
    let _publishing = state.publish_lock.lock().await;
    let conflict = {
        let index = state.index.read().await;
        if index
            .iter()
            .any(|e| e.hexon_id == id && e.version == version)
        {
            Some(format!("already published: {id}@{version}"))
        } else if let Some(other) = index
            .iter()
            .find(|e| e.hexon_id != id && e.hexon_id.eq_ignore_ascii_case(&id))
        {
            // Ids are one case-insensitive namespace: no squatting a look-alike of another publisher's id.
            Some(format!("{id} differs only in case from {}", other.hexon_id))
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
    match state.store.put(&id, &version, body.clone()).await {
        Ok(()) => {}
        Err(StoreError::AlreadyExists) => {
            return json_err(
                StatusCode::CONFLICT,
                format!("already published: {id}@{version}"),
            );
        }
        Err(e) => return internal("publish write failed", &e),
    }

    // Full relist keeps the index consistent with the store contents.
    match state.store.list().await {
        Ok(entries) => {
            tracing::info!(count = entries.len(), published = %format!("{id}@{version}"), "reindexed");
            *state.index.write().await = entries;
        }
        Err(e) => return internal("reindex after publish failed", &e),
    }
    Json(ApiResponse::success(manifest)).into_response()
}
