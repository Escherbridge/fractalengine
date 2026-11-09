//! Portal bundle ingest, file index, summary read and the share-route CORS rule (README §8, admission.md §8; see `fe-api/AGENTS.md` §portal-bundles).

use std::path::{Path as FsPath, PathBuf};
use std::sync::{Arc, OnceLock};

use axum::extract::{Path, State};
use axum::http::request::Parts;
use axum::http::{HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use axum_extra::extract::Multipart;
use fe_format::integrity::{self, IntegrityStatus};
use fe_format::limits::MAX_ARCHIVE_ENTRIES;
use fe_format::portal_bundle::{
    mime_for_path, validate_portal_bundle, BundleFacts, BundleMember, Refusal, RefusalClass,
    ValidatedBundle, DEFAULT_MAX_BUNDLE_BYTES, PORTAL_BUNDLE_MIME,
};
use fe_format::HexonArchiveIndex;
use fe_identity::api_token::ApiClaims;
use fe_runtime::blob_store::{hash_from_hex, hash_to_hex, BlobStoreHandle};
use fe_runtime::messages::{DbCommand, DbResult};
use serde::Serialize;
use serde_json::{json, Value};

use crate::auth::require_role;
use crate::server::ApiState;
use crate::types::{is_valid_ulid, ApiResponse};

/// Env var overriding the per-node bundle size cap in bytes (README §8.4, OD-4).
pub const BUNDLE_MAX_BYTES_ENV: &str = "FE_PORTAL_BUNDLE_MAX_BYTES";
/// Multipart framing overhead allowed above the bundle cap.
const ROUTE_OVERHEAD_BYTES: usize = 1024 * 1024;
/// Longest asset display name derived from a bundle title.
const MAX_ASSET_NAME_CHARS: usize = 255;
/// Cap used when only listing an already-stored bundle (no ingest decision rides on it).
const LISTING_CAP_BYTES: u64 = u64::MAX / 16;

/// Per-node bundle size cap: [`BUNDLE_MAX_BYTES_ENV`] or the 32 MiB default.
pub fn bundle_max_bytes() -> u64 {
    std::env::var(BUNDLE_MAX_BYTES_ENV)
        .ok()
        .and_then(|raw| raw.trim().parse::<u64>().ok())
        .filter(|cap| *cap > 0)
        .unwrap_or(DEFAULT_MAX_BUNDLE_BYTES)
}

/// Body limit for the upload route: the cap plus multipart framing (SI wires it in `server.rs`).
pub fn route_body_limit() -> usize {
    usize::try_from(bundle_max_bytes())
        .unwrap_or(usize::MAX - ROUTE_OVERHEAD_BYTES)
        .saturating_add(ROUTE_OVERHEAD_BYTES)
}

// ---------------------------------------------------------------------------
// Local admission seam (governance owns the record; README §8.5, admission.md §2)
// ---------------------------------------------------------------------------

/// admission.md §2 state for this peer: `state` ∈ admitted | static_sandbox | revoked | blocked; `reason` a snake_case
/// code (`cannot_admit` on `static_sandbox` when the local identity may not admit it, README §8.5).
#[derive(Debug, Clone, Serialize)]
pub struct AdmissionView {
    pub state: String,
    pub reason: Option<String>,
}

/// The build facts governance judges; both digests are kill-list keys (an entry matches by hash under any id).
#[derive(Debug, Clone, Copy)]
pub struct AdmissionQuery<'a> {
    pub package_id: &'a str,
    pub content_hash: &'a str,
    /// BLAKE3 of the stored archive: the blob hash.
    pub archive_digest: &'a str,
    /// Committed member digests (sealed builds; empty when unsealed or not yet read): kill-list keys too (admission.md §4.2).
    pub member_digests: &'a [String],
    pub sealed_by: Option<&'a str>,
    pub capability_digest: &'a str,
}

/// Governance's read side, injected by the binary (which maps it through the portal's scope-free record verdict).
pub trait LocalAdmissionSource: Send + Sync {
    /// The host's own API client DID: a token is local only when its `client` claim names it.
    fn local_did(&self) -> String;
    /// The §2 state of this build on this peer, scope-free (no verse ceiling); a static build the local identity may not
    /// admit (unsealed, or governance's may-admit verdict is no) carries `reason: "cannot_admit"`. Empty `package_id`
    /// and `content_hash` mean "not yet read": judge the archive digest alone.
    fn admission(&self, bundle: &AdmissionQuery<'_>) -> Option<AdmissionView>;
}

static LOCAL_ADMISSION: OnceLock<Arc<dyn LocalAdmissionSource>> = OnceLock::new();

/// Register governance's admission reader once at startup (later calls are ignored).
pub fn set_local_admission_source(source: Arc<dyn LocalAdmissionSource>) {
    let _ = LOCAL_ADMISSION.set(source);
}

/// Admission block for the summary: present only for the host's own client identity.
fn admission_for(claims: &ApiClaims, bundle: &AdmissionQuery<'_>) -> Option<AdmissionView> {
    let source = LOCAL_ADMISSION.get()?;
    // Every accepted token's `sub` is this node, so only the `client` claim can single out the local identity.
    (claims.client.as_deref() == Some(source.local_did().as_str()))
        .then(|| source.admission(bundle))
        .flatten()
}

// ---------------------------------------------------------------------------
// Share-route CORS for static portal origins (admission.md §8.2)
// ---------------------------------------------------------------------------

/// The share-redemption route prefix (`GET /api/v1/shared/{token}`).
const SHARE_ROUTE_PREFIX: &str = "/api/v1/shared/";
const STATIC_PORTAL_ORIGIN_PREFIX: &[u8] = b"https://fe-portal.s-";
const PORTAL_ORIGIN_SUFFIX: &[u8] = b".localhost";
/// Base32 characters of a portal origin label after its `s-` prefix.
const PORTAL_LABEL_CHARS: usize = 26;

/// Exactly `https://fe-portal.s-[a-z2-7]{26}.localhost`: a static-label portal origin, never an admitted `a-` one.
pub fn is_static_portal_origin(origin: &[u8]) -> bool {
    origin
        .strip_prefix(STATIC_PORTAL_ORIGIN_PREFIX)
        .and_then(|rest| rest.strip_suffix(PORTAL_ORIGIN_SUFFIX))
        .is_some_and(|label| {
            label.len() == PORTAL_LABEL_CHARS
                && label
                    .iter()
                    .all(|c| c.is_ascii_lowercase() || (b'2'..=b'7').contains(c))
        })
}

/// The CORS rule SI combines with `cors_origins` in `server.rs`: a static portal origin, on `GET /api/v1/shared/{token}`
/// or its preflight only, never with credentials (the token is the credential).
pub fn share_route_allows(origin: &HeaderValue, parts: &Parts) -> bool {
    let route = parts
        .uri
        .path()
        .strip_prefix(SHARE_ROUTE_PREFIX)
        .is_some_and(|token| !token.is_empty() && !token.contains('/'));
    let method = parts.method == Method::GET
        || (parts.method == Method::OPTIONS
            && parts
                .headers
                .get("access-control-request-method")
                .is_some_and(|requested| requested == "GET"));
    route && method && is_static_portal_origin(origin.as_bytes())
}

// ---------------------------------------------------------------------------
// Inspection (pure of HTTP; synchronous, run under spawn_blocking)
// ---------------------------------------------------------------------------

/// One archive member as the file index lists it.
#[derive(Debug, Clone, Serialize)]
pub struct MemberInfo {
    pub path: String,
    pub size: u64,
    /// Committed digest when sealed, `null` when unsealed.
    pub blake3: Option<String>,
    pub mime: &'static str,
}

/// A bundle that passed README §6, with everything the routes report.
#[derive(Debug, Clone)]
pub struct InspectedBundle {
    pub validated: ValidatedBundle,
    pub content_hash: String,
    pub sealed_by: Option<String>,
    pub members: Vec<MemberInfo>,
}

fn refusal(class: RefusalClass, detail: impl Into<String>) -> Refusal {
    Refusal {
        class,
        detail: detail.into(),
    }
}

/// Map a lazy member-read failure: a limit breach is `archive_limit`, anything else a seal/CRC failure.
fn member_read_refusal(path: &str, err: &anyhow::Error) -> Refusal {
    let text = format!("{err:#}");
    if text.to_ascii_lowercase().contains("limit") {
        refusal(RefusalClass::ArchiveLimit, format!("member {path}: {text}"))
    } else {
        refusal(RefusalClass::Integrity, format!("member {path}: {text}"))
    }
}

/// Run README §6 over the archive at `path` (`archive_bytes` long) under `max_bytes`.
pub fn inspect_archive(
    path: &FsPath,
    archive_bytes: u64,
    max_bytes: u64,
) -> Result<InspectedBundle, Refusal> {
    // Check the cap before opening so an over-cap body costs no inflate work.
    if archive_bytes > max_bytes {
        return Err(refusal(
            RefusalClass::TooLarge,
            format!("{archive_bytes} bytes over cap {max_bytes}"),
        ));
    }
    let index = HexonArchiveIndex::open(path).map_err(|e| {
        refusal(
            RefusalClass::Integrity,
            format!("not a readable hexon archive: {}", e.root_cause()),
        )
    })?;
    let status = index.integrity_status();
    let sealed_by = match &status {
        IntegrityStatus::Sealed { publisher_did } => Some(publisher_did.clone()),
        IntegrityStatus::Unsealed => None,
    };

    let names: Vec<String> = index.member_names().map(str::to_string).collect();
    if names.len() > MAX_ARCHIVE_ENTRIES {
        return Err(refusal(
            RefusalClass::ArchiveLimit,
            format!("{} entries", names.len()),
        ));
    }
    let inflate_budget = max_bytes.saturating_mul(8);
    let mut inflated: u64 = 0;
    let mut members: Vec<BundleMember> = Vec::with_capacity(names.len());
    let mut infos: Vec<MemberInfo> = Vec::with_capacity(names.len());
    for name in &names {
        if name.ends_with('/') {
            members.push(BundleMember {
                path: name.clone(),
                uncompressed_size: 0,
            });
            continue;
        }
        let bytes = match index.read_member(name) {
            Ok(Some(bytes)) => bytes,
            Ok(None) => continue,
            Err(e) => return Err(member_read_refusal(name, &e)),
        };
        let size = bytes.len() as u64;
        inflated = inflated.saturating_add(size);
        if inflated > inflate_budget {
            return Err(refusal(
                RefusalClass::TooLarge,
                "declared bytes over 8x cap",
            ));
        }
        members.push(BundleMember {
            path: name.clone(),
            uncompressed_size: size,
        });
        infos.push(MemberInfo {
            path: name.clone(),
            size,
            // A sealed read already matched the committed digest, so this is that digest.
            blake3: sealed_by.is_some().then(|| integrity::digest_hex(&bytes)),
            mime: mime_for_path(name).unwrap_or("application/octet-stream"),
        });
    }

    let header = index.header_doc();
    let manifest = index.manifest();
    let extension_json = index.read_member("plugin/extension.json").ok().flatten();
    let facts = BundleFacts {
        archive_bytes,
        max_bytes,
        members: &members,
        integrity_failure: None,
        extension_json: extension_json.as_deref(),
        hexon_id: &manifest.hexon_id,
        hexon_version: &manifest.version,
    };
    let validated = validate_portal_bundle(&facts, &origin_allowed)?;

    let content_hash = if sealed_by.is_some() {
        integrity::content_hash(header.manifest.as_bytes()).map_err(|e| {
            refusal(RefusalClass::Integrity, format!("content hash: {e:#}"))
        })?
    } else {
        file_blake3_hex(path).map_err(|e| refusal(RefusalClass::Integrity, format!("hash: {e}")))?
    };
    Ok(InspectedBundle {
        validated,
        content_hash,
        sealed_by,
        members: infos,
    })
}

/// The URL policy for a declared origin or feed origin (spec §6.1): `is_url_allowed` on the parsed URL, no fixture bypass.
fn origin_allowed(origin: &str) -> bool {
    origin
        .parse::<url::Url>()
        .is_ok_and(|url| fe_runtime::portal_url::is_url_allowed(&url))
}

/// The committed member digests of an inspected build (kill-list keys beside `content_hash` and the archive digest).
fn committed_member_digests(inspected: &InspectedBundle) -> Vec<String> {
    inspected.members.iter().filter_map(|member| member.blake3.clone()).collect()
}

/// BLAKE3 hex of a file, streamed.
fn file_blake3_hex(path: &FsPath) -> std::io::Result<String> {
    let mut hasher = blake3::Hasher::new();
    let mut file = std::fs::File::open(path)?;
    std::io::copy(&mut file, &mut hasher)?;
    Ok(hasher.finalize().to_hex().to_string())
}

/// A scratch file removed on drop; the archive reader needs a path, not bytes.
struct ScratchFile(PathBuf);

impl ScratchFile {
    fn write(bytes: &[u8]) -> std::io::Result<Self> {
        let path = std::env::temp_dir().join(format!("fe-portal-{}.hexon", ulid::Ulid::new()));
        std::fs::write(&path, bytes)?;
        Ok(Self(path))
    }
}

impl Drop for ScratchFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Why a stored bundle could not be inspected.
#[derive(Debug)]
pub enum StoredBundleError {
    /// The asset's `content_hash` is not a valid blob hash.
    CorruptRecord,
    /// The blob is not in the local store.
    BlobMissing,
    /// The blob failed README §6.
    Refused(Refusal),
    /// Reading the blob failed.
    Io,
}

/// Inspect a bundle already in the blob store (file index and summary reads).
pub fn inspect_stored(
    blob_store: &BlobStoreHandle,
    blob_hash_hex: &str,
) -> Result<InspectedBundle, StoredBundleError> {
    let hash = hash_from_hex(blob_hash_hex).map_err(|_| StoredBundleError::CorruptRecord)?;
    let path = blob_store
        .get_blob_path(&hash)
        .ok_or(StoredBundleError::BlobMissing)?;
    let len = std::fs::metadata(&path)
        .map_err(|_| StoredBundleError::Io)?
        .len();
    inspect_archive(&path, len, LISTING_CAP_BYTES).map_err(StoredBundleError::Refused)
}

// ---------------------------------------------------------------------------
// Response shapes
// ---------------------------------------------------------------------------

fn error_json(status: StatusCode, message: &str) -> Response {
    (
        status,
        Json(ApiResponse::<Value>::error(message.to_string())),
    )
        .into_response()
}

fn refusal_response(refused: &Refusal) -> Response {
    let status =
        StatusCode::from_u16(refused.class.http_status()).unwrap_or(StatusCode::UNPROCESSABLE_ENTITY);
    (
        status,
        Json(json!({
            "ok": false,
            "error": refused.class.as_str(),
            "detail": refused.detail,
        })),
    )
        .into_response()
}

/// The shared summary fields (README §8.3, without `members`).
fn summary_fields(
    inspected: &InspectedBundle,
    asset_id: &str,
    blob_hash: &str,
) -> serde_json::Map<String, Value> {
    let portal = &inspected.validated.portal;
    let mut map = serde_json::Map::new();
    map.insert("asset_id".into(), json!(asset_id));
    map.insert("content_type".into(), json!(PORTAL_BUNDLE_MIME));
    map.insert("blob_hash".into(), json!(blob_hash));
    map.insert("content_hash".into(), json!(inspected.content_hash));
    map.insert("package_id".into(), json!(inspected.validated.package_id));
    map.insert("version".into(), json!(inspected.validated.version));
    map.insert("sealed_by".into(), json!(inspected.sealed_by));
    map.insert("title".into(), json!(portal.title));
    map.insert("entry".into(), json!(portal.entry));
    map.insert(
        "capability_digest".into(),
        json!(inspected.validated.capability_digest),
    );
    map
}

fn stored_error_response(asset_id: &str, err: &StoredBundleError) -> Response {
    match err {
        StoredBundleError::BlobMissing => {
            tracing::warn!(asset_id, "bundle blob missing from the local store");
            error_json(StatusCode::NOT_FOUND, "asset blob not found in store")
        }
        StoredBundleError::CorruptRecord => {
            tracing::error!(asset_id, "bundle asset has a malformed content_hash");
            error_json(StatusCode::INTERNAL_SERVER_ERROR, "corrupt asset record")
        }
        StoredBundleError::Io => {
            tracing::error!(asset_id, "failed reading bundle blob");
            error_json(StatusCode::INTERNAL_SERVER_ERROR, "failed to read asset blob")
        }
        StoredBundleError::Refused(refused) => refusal_response(refused),
    }
}

/// File index for a bundle asset (README §8.3): members are listed, never served.
pub(crate) async fn file_index_response(
    blob_store: &BlobStoreHandle,
    asset_id: &str,
    blob_hash_hex: &str,
) -> Response {
    let store = blob_store.clone();
    let hash_hex = blob_hash_hex.to_string();
    let inspected = tokio::task::spawn_blocking(move || inspect_stored(&store, &hash_hex)).await;
    match inspected {
        Ok(Ok(bundle)) => {
            let mut fields = summary_fields(&bundle, asset_id, blob_hash_hex);
            fields.insert("members".into(), json!(bundle.members));
            (
                [("x-content-type-options", "nosniff")],
                Json(ApiResponse::success(Value::Object(fields))),
            )
                .into_response()
        }
        Ok(Err(err)) => stored_error_response(asset_id, &err),
        Err(e) => {
            tracing::error!("bundle index task failed: {e}");
            error_json(StatusCode::INTERNAL_SERVER_ERROR, "bundle index failed")
        }
    }
}

// ---------------------------------------------------------------------------
// POST /api/v1/petals/{petal_id}/portal-bundles
// ---------------------------------------------------------------------------

/// Printable asset display name derived from the bundle title.
fn asset_name(title: &str, package_id: &str) -> String {
    let cleaned: String = title
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let trimmed = cleaned.trim();
    let name = if trimmed.is_empty() { package_id } else { trimmed };
    name.chars().take(MAX_ASSET_NAME_CHARS).collect()
}

/// POST /api/v1/petals/{petal_id}/portal-bundles — multipart `file` part; Editor+ at the petal.
/// Validates README §6 in full, stores one blob, registers one metadata-only `asset` row.
pub async fn upload_portal_bundle(
    State(state): State<Arc<ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    Path(petal_id): Path<String>,
    mut multipart: Multipart,
) -> Response {
    if require_role(&claims, "editor").is_err() {
        return error_json(StatusCode::FORBIDDEN, "insufficient permissions");
    }
    if !is_valid_ulid(&petal_id) {
        return error_json(StatusCode::BAD_REQUEST, "invalid petal_id");
    }
    if crate::rest::authorized_petal_scope(&state, &claims, &petal_id)
        .await
        .is_none()
    {
        return error_json(StatusCode::NOT_FOUND, crate::rest::NOT_FOUND_OR_DENIED);
    }
    let Some(store) = state.blob_store.clone() else {
        return error_json(
            StatusCode::SERVICE_UNAVAILABLE,
            "asset upload unavailable: no blob store configured",
        );
    };

    let max_bytes = bundle_max_bytes();
    let mut file: Option<Vec<u8>> = None;
    loop {
        match multipart.next_field().await {
            Ok(Some(field)) if field.name() == Some("file") => match field.bytes().await {
                Ok(bytes) => file = Some(bytes.to_vec()),
                Err(e) => {
                    let status = e.status();
                    let message = if status == StatusCode::PAYLOAD_TOO_LARGE {
                        format!("too_large: bundle exceeds the {max_bytes}-byte limit")
                    } else {
                        format!("failed to read upload: {e}")
                    };
                    return error_json(status, &message);
                }
            },
            Ok(Some(_)) => {}
            Ok(None) => break,
            Err(e) => {
                return error_json(StatusCode::BAD_REQUEST, &format!("multipart error: {e}"));
            }
        }
    }
    let Some(bytes) = file else {
        return error_json(
            StatusCode::BAD_REQUEST,
            "missing 'file' field in multipart upload",
        );
    };
    if bytes.len() as u64 > max_bytes {
        return refusal_response(&refusal(
            RefusalClass::TooLarge,
            format!("{} bytes over cap {max_bytes}", bytes.len()),
        ));
    }

    let owner_did = crate::upload::owner_key(&claims).to_string();
    let quota_owner = crate::upload::UploadOwner::new(&claims, &petal_id);
    if let Err(denied) =
        crate::upload::check_upload_quota(&state, &quota_owner, bytes.len() as u64).await
    {
        return error_json(denied.status(), &denied.message());
    }

    // Validate, then store: bytes are consumed here and never reach the DB channel.
    let size_bytes = bytes.len() as u64;
    let stored = tokio::task::spawn_blocking(move || -> Result<(InspectedBundle, String), Response> {
        let scratch = ScratchFile::write(&bytes).map_err(|e| {
            tracing::error!("bundle scratch write failed: {e}");
            error_json(StatusCode::BAD_GATEWAY, "asset storage failed")
        })?;
        let inspected = inspect_archive(&scratch.0, size_bytes, max_bytes)
            .map_err(|refused| refusal_response(&refused))?;
        let hash = store.add_blob(&bytes).map_err(|e| {
            tracing::error!("bundle blob write failed: {e}");
            error_json(StatusCode::BAD_GATEWAY, "asset storage failed")
        })?;
        Ok((inspected, hash_to_hex(&hash)))
    })
    .await;
    let (inspected, blob_hash) = match stored {
        Ok(Ok(done)) => done,
        Ok(Err(response)) => return response,
        Err(e) => {
            tracing::error!("bundle store task failed: {e}");
            return error_json(StatusCode::BAD_GATEWAY, "asset storage failed");
        }
    };

    let correlation_id = ulid::Ulid::new().to_string();
    let cmd = DbCommand::CreateAsset {
        name: asset_name(&inspected.validated.portal.title, &inspected.validated.package_id),
        content_type: PORTAL_BUNDLE_MIME.to_string(),
        size_bytes,
        content_hash: blob_hash.clone(),
        owner_did: Some(owner_did),
        petal_id: Some(petal_id),
        correlation_id: Some(correlation_id.clone()),
    };
    let asset_id = match crate::rest::db_round_trip(&state, cmd).await {
        Ok(DbResult::AssetCreated {
            asset_id,
            content_hash,
            correlation_id: Some(echoed),
            ..
        }) if echoed == correlation_id && content_hash == blob_hash => asset_id,
        Ok(other) => {
            tracing::warn!("CreateAsset for bundle was refused or mismatched: {other:?}");
            return error_json(StatusCode::BAD_GATEWAY, "asset registration failed");
        }
        Err(e) => {
            tracing::warn!("CreateAsset round trip failed for bundle: {e}");
            return error_json(StatusCode::BAD_GATEWAY, "asset registration failed");
        }
    };

    let portal = &inspected.validated.portal;
    Json(ApiResponse::success(json!({
        "asset_id": asset_id,
        "blob_hash": blob_hash,
        "content_hash": inspected.content_hash,
        "package_id": inspected.validated.package_id,
        "version": inspected.validated.version,
        "sealed_by": inspected.sealed_by,
        "title": portal.title,
        "entry": portal.entry,
    })))
    .into_response()
}

// ---------------------------------------------------------------------------
// GET /api/v1/portal-bundles/{asset_id}
// ---------------------------------------------------------------------------

/// GET /api/v1/portal-bundles/{asset_id} — the file-index summary without `members`, plus the
/// local `admission` block for the host's own identity only (README §8.5).
pub async fn get_portal_bundle(
    State(state): State<Arc<ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    Path(asset_id): Path<String>,
) -> Response {
    if require_role(&claims, "viewer").is_err() {
        return error_json(StatusCode::FORBIDDEN, "insufficient permissions");
    }
    if !is_valid_ulid(&asset_id) {
        return error_json(StatusCode::BAD_REQUEST, "invalid asset_id");
    }
    let Some(store) = state.blob_store.clone() else {
        return error_json(StatusCode::SERVICE_UNAVAILABLE, "blob store not configured");
    };
    // One uniform 404 for unknown, unreachable and not-a-bundle ids (no existence oracle).
    let denied = || error_json(StatusCode::NOT_FOUND, crate::rest::NOT_FOUND_OR_DENIED);
    if crate::assets::asset_reach(&state, &claims, std::slice::from_ref(&asset_id)).await
        != crate::assets::AssetReach::Covered
    {
        return denied();
    }
    let Some(rows) = crate::gis::run_select(
        &state,
        "SELECT content_type, content_hash FROM asset WHERE asset_id = $aid LIMIT 1",
        vec![("aid".to_string(), json!(asset_id))],
    )
    .await
    else {
        return error_json(StatusCode::BAD_GATEWAY, "asset metadata query failed");
    };
    let text = |row: &Value, key: &str| row.get(key).and_then(Value::as_str).map(str::to_string);
    let Some(row) = rows.first() else {
        return denied();
    };
    if text(row, "content_type").as_deref() != Some(PORTAL_BUNDLE_MIME) {
        return denied();
    }
    let Some(blob_hash) = text(row, "content_hash") else {
        return error_json(StatusCode::INTERNAL_SERVER_ERROR, "corrupt asset record");
    };

    let hash_for_task = blob_hash.clone();
    let inspected =
        match tokio::task::spawn_blocking(move || inspect_stored(&store, &hash_for_task)).await {
            Ok(Ok(bundle)) => bundle,
            Ok(Err(err)) => return stored_error_response(&asset_id, &err),
            Err(e) => {
                tracing::error!("bundle summary task failed: {e}");
                return error_json(StatusCode::INTERNAL_SERVER_ERROR, "bundle summary failed");
            }
        };
    let mut fields = summary_fields(&inspected, &asset_id, &blob_hash);
    let member_digests = committed_member_digests(&inspected);
    let query = AdmissionQuery {
        package_id: &inspected.validated.package_id,
        content_hash: &inspected.content_hash,
        archive_digest: &blob_hash,
        member_digests: &member_digests,
        sealed_by: inspected.sealed_by.as_deref(),
        capability_digest: &inspected.validated.capability_digest,
    };
    if let Some(view) = admission_for(&claims, &query) {
        fields.insert("admission".into(), json!(view));
    }
    Json(ApiResponse::success(Value::Object(fields))).into_response()
}

// ---------------------------------------------------------------------------
// GET /api/v1/nodes/{node_id}/portal/export.zip (static-export.md, R17)
// ---------------------------------------------------------------------------

/// `fe-portal.json` schema tag (static-export.md §3).
const EXPORT_SCHEMA: &str = "fe-portal-export/1";
/// Prefix every bundle member carries inside the archive; stripped in the export.
const WEB_PREFIX: &str = "web/";
/// Feeds kept per export, in name order (matches the in-app static mount).
const MAX_EXPORT_FEEDS: usize = 16;
/// Longest feed name (README §9.3: `[a-z][a-z0-9_]{0,47}`).
const MAX_FEED_NAME_BYTES: usize = 48;
/// Longest share token kept.
const MAX_FEED_TOKEN_BYTES: usize = 4096;
/// Longest CSP source expression accepted.
const MAX_CSP_SOURCE_BYTES: usize = 255;

/// The feeds an export carries, after static-export.md §4.2.
#[derive(Debug, Default, PartialEq, Eq)]
struct ExportFeeds {
    /// name -> `{url, expires_at}`, the `__fe/feeds.json` `feeds` object.
    kept: serde_json::Map<String, Value>,
    /// `<origin>/api/v1/shared/` of each kept feed (the CSP `connect-src` additions).
    sources: Vec<String>,
    /// Names left out (loopback, non-https, malformed or over the cap).
    dropped: Vec<String>,
}

fn is_feed_name(name: &str) -> bool {
    let mut chars = name.chars();
    name.len() <= MAX_FEED_NAME_BYTES
        && chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// One CSP host-source: `http(s)://` + lowercase host chars + an optional `[a-z0-9/_-]` path.
fn is_csp_source(source: &str) -> bool {
    let Some(rest) = source
        .strip_prefix("https://")
        .or_else(|| source.strip_prefix("http://"))
    else {
        return false;
    };
    let (host, path) = rest.split_at(rest.find('/').unwrap_or(rest.len()));
    !host.is_empty()
        && source.len() <= MAX_CSP_SOURCE_BYTES
        && host.bytes().all(|c| {
            c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, b'.' | b'-' | b':' | b'[' | b']')
        })
        && path
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, b'/' | b'_' | b'-'))
}

/// `(origin, token)` of exactly `https://<origin>/api/v1/shared/<token>` on a reachable origin, else `None`.
fn reachable_share_url(raw: &str) -> Option<(&str, &str)> {
    let (origin, token) = raw.split_once(SHARE_ROUTE_PREFIX)?;
    let token_ok = !token.is_empty()
        && token.len() <= MAX_FEED_TOKEN_BYTES
        && token
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.'));
    (origin.starts_with("https://") && token_ok && is_csp_source(origin) && origin_allowed(origin))
        .then_some((origin, token))
}

/// RFC 3339 `exp` of a share token's payload segment, or `null` when unreadable.
fn share_token_expiry(token: &str) -> Value {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine as _;
    token
        .split_once('.')
        .and_then(|(payload, _)| URL_SAFE_NO_PAD.decode(payload).ok())
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|claims| claims.get("exp").and_then(Value::as_i64))
        .and_then(|exp| chrono::DateTime::from_timestamp(exp, 0))
        .map_or(Value::Null, |at| {
            json!(at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
        })
}

/// Apply static-export.md §4.2 to the node's `portal.feeds` value (name -> share URL).
fn admit_export_feeds(declared: Option<&Value>) -> ExportFeeds {
    let mut out = ExportFeeds::default();
    let Some(map) = declared.and_then(Value::as_object) else {
        return out;
    };
    let mut names: Vec<&String> = map.keys().collect();
    names.sort();
    let mut sources = std::collections::BTreeSet::new();
    for name in names {
        let kept = map[name]
            .as_str()
            .and_then(reachable_share_url)
            .filter(|_| is_feed_name(name) && out.kept.len() < MAX_EXPORT_FEEDS);
        let Some((origin, token)) = kept else {
            out.dropped.push(name.clone());
            continue;
        };
        out.kept.insert(
            name.clone(),
            json!({
                "url": format!("{origin}{SHARE_ROUTE_PREFIX}{token}"),
                "expires_at": share_token_expiry(token),
            }),
        );
        sources.insert(format!("{origin}{SHARE_ROUTE_PREFIX}"));
    }
    out.sources = sources.into_iter().collect();
    out
}

/// The policy a static mount of this bundle gets (protocol.md §5): `connect-src` is `'self'` plus the manifest's origins and the kept feed origins.
fn static_csp(
    csp: &fe_format::portal_bundle::PortalCsp,
    feed_sources: &[String],
) -> Result<String, String> {
    fn with(base: &str, lists: &[&[String]]) -> Result<String, String> {
        let mut out = base.to_owned();
        for source in lists.iter().flat_map(|list| list.iter()) {
            if !is_csp_source(source) {
                return Err(source.clone());
            }
            out.push(' ');
            out.push_str(source);
        }
        Ok(out)
    }
    let flag = |base: &str, on: bool, keyword: &str| {
        if on {
            format!("{base} {keyword}")
        } else {
            base.to_owned()
        }
    };
    let frame = if csp.frame.is_empty() {
        "frame-src 'none'".to_owned()
    } else {
        with("frame-src", &[csp.frame.as_slice()])?
    };
    Ok([
        "default-src 'none'".to_owned(),
        flag("script-src 'self'", csp.wasm, "'wasm-unsafe-eval'"),
        flag("style-src 'self'", csp.inline_styles, "'unsafe-inline'"),
        with("img-src 'self' data: blob:", &[csp.img.as_slice()])?,
        with("font-src 'self'", &[csp.font.as_slice()])?,
        with("media-src 'self'", &[csp.media.as_slice()])?,
        with("connect-src 'self'", &[csp.connect.as_slice(), feed_sources])?,
        frame,
        "worker-src 'self'".to_owned(),
        "form-action 'none'".to_owned(),
        "base-uri 'none'".to_owned(),
        "object-src 'none'".to_owned(),
        "frame-ancestors 'none'".to_owned(),
    ]
    .join("; "))
}

/// A revoked or kill-listed build is not exported to render elsewhere (admission.md §4); governance matches by hash under any id.
fn revoked_on_this_peer(query: &AdmissionQuery<'_>) -> bool {
    LOCAL_ADMISSION
        .get()
        .and_then(|source| source.admission(query))
        .is_some_and(|view| view.state == "revoked")
}

/// Why an export could not be built.
enum ExportFailure {
    Stored(StoredBundleError),
    Revoked,
    UnsafeCsp(String),
    Zip,
}

/// Download name `<package_id>-<version>.zip`, limited to header-safe characters.
fn export_file_name(package_id: &str, version: &str) -> String {
    let safe: String = format!("{package_id}-{version}")
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!("{safe}.zip")
}

/// Inspect, re-check revocation with the full build facts, and write the export archive (static-export.md §3).
fn build_export(
    store: &BlobStoreHandle,
    blob_hash_hex: &str,
    feeds: &ExportFeeds,
) -> Result<(Vec<u8>, String), ExportFailure> {
    let inspected = inspect_stored(store, blob_hash_hex).map_err(ExportFailure::Stored)?;
    let member_digests = committed_member_digests(&inspected);
    let query = AdmissionQuery {
        package_id: &inspected.validated.package_id,
        content_hash: &inspected.content_hash,
        archive_digest: blob_hash_hex,
        member_digests: &member_digests,
        sealed_by: inspected.sealed_by.as_deref(),
        capability_digest: &inspected.validated.capability_digest,
    };
    if revoked_on_this_peer(&query) {
        return Err(ExportFailure::Revoked);
    }
    let portal = &inspected.validated.portal;
    let csp = static_csp(&portal.csp, &feeds.sources).map_err(ExportFailure::UnsafeCsp)?;
    let hash = hash_from_hex(blob_hash_hex)
        .map_err(|_| ExportFailure::Stored(StoredBundleError::CorruptRecord))?;
    let path = store
        .get_blob_path(&hash)
        .ok_or(ExportFailure::Stored(StoredBundleError::BlobMissing))?;
    // Reads verify the seal member by member (VerifyIfSealed); a tampered member is a refusal, never exported.
    let index =
        HexonArchiveIndex::open(&path).map_err(|_| ExportFailure::Stored(StoredBundleError::Io))?;

    let entry = portal.entry.strip_prefix(WEB_PREFIX).unwrap_or(&portal.entry);
    let descriptor = json!({
        "schema": EXPORT_SCHEMA,
        "package_id": inspected.validated.package_id,
        "version": inspected.validated.version,
        "content_hash": inspected.content_hash,
        "publisher_did": inspected.sealed_by,
        "title": portal.title,
        "entry": entry,
        "csp": csp,
        "feeds_dropped": feeds.dropped,
        "exported_at": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
    });
    let feeds_doc = json!({ "v": 1, "feeds": feeds.kept });

    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let mut put = |name: &str, bytes: &[u8]| -> Result<(), ExportFailure> {
        use std::io::Write as _;
        writer
            .start_file(name, options)
            .map_err(|_| ExportFailure::Zip)?;
        writer.write_all(bytes).map_err(|_| ExportFailure::Zip)
    };
    put("fe-portal.json", descriptor.to_string().as_bytes())?;
    put("__fe/feeds.json", feeds_doc.to_string().as_bytes())?;
    let mut members: Vec<&String> = inspected
        .validated
        .web_members
        .iter()
        .map(|(member, _mime)| member)
        .filter(|member| !member.ends_with('/'))
        .collect();
    members.sort();
    for member in members {
        let Some(relative) = member.strip_prefix(WEB_PREFIX) else {
            continue;
        };
        match index.read_member(member) {
            Ok(Some(bytes)) => put(relative, &bytes)?,
            Ok(None) => {}
            Err(e) => {
                return Err(ExportFailure::Stored(StoredBundleError::Refused(
                    member_read_refusal(member, &e),
                )))
            }
        }
    }
    let bytes = writer.finish().map_err(|_| ExportFailure::Zip)?.into_inner();
    Ok((
        bytes,
        export_file_name(&inspected.validated.package_id, &inspected.validated.version),
    ))
}

/// GET /api/v1/nodes/{node_id}/portal/export.zip — Viewer+ with reach; a repository node's bundle as a static directory.
pub async fn export_portal_bundle(
    State(state): State<Arc<ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    Path(node_id): Path<String>,
) -> Response {
    if require_role(&claims, "viewer").is_err() {
        return error_json(StatusCode::FORBIDDEN, "insufficient permissions");
    }
    if !is_valid_ulid(&node_id) {
        return error_json(StatusCode::BAD_REQUEST, "invalid node_id");
    }
    // One uniform 404 for unknown, unreachable and not-a-repository nodes (no existence oracle).
    let denied = || error_json(StatusCode::NOT_FOUND, crate::rest::NOT_FOUND_OR_DENIED);
    let Some(scope) = crate::rest::authorized_node_scope(&state, &claims, &node_id).await else {
        return denied();
    };
    let Ok(read) = crate::rest::get_properties_core(&state, &node_id, &scope).await else {
        return denied();
    };
    let properties = &read.properties;
    if properties.get("node_kind").and_then(Value::as_str) != Some("repository") {
        return denied();
    }
    let Some(asset_id) = properties
        .get("portal.bundle_asset")
        .and_then(Value::as_str)
        .filter(|id| is_valid_ulid(id))
        .map(str::to_string)
    else {
        return denied();
    };
    let Some(store) = state.blob_store.clone() else {
        return error_json(StatusCode::SERVICE_UNAVAILABLE, "blob store not configured");
    };
    if crate::assets::asset_reach(&state, &claims, std::slice::from_ref(&asset_id)).await
        != crate::assets::AssetReach::Covered
    {
        return denied();
    }
    let Some(rows) = crate::gis::run_select(
        &state,
        "SELECT content_type, content_hash FROM asset WHERE asset_id = $aid LIMIT 1",
        vec![("aid".to_string(), json!(asset_id))],
    )
    .await
    else {
        return error_json(StatusCode::BAD_GATEWAY, "asset metadata query failed");
    };
    let text = |row: &Value, key: &str| row.get(key).and_then(Value::as_str).map(str::to_string);
    let Some(row) = rows.first() else {
        return denied();
    };
    if text(row, "content_type").as_deref() != Some(PORTAL_BUNDLE_MIME) {
        return denied();
    }
    let Some(blob_hash) = text(row, "content_hash") else {
        return error_json(StatusCode::INTERNAL_SERVER_ERROR, "corrupt asset record");
    };

    // Revocation precedes bytes: the stored archive digest alone is a kill-list key (admission.md §2).
    let archive_only = AdmissionQuery {
        package_id: "",
        content_hash: "",
        archive_digest: &blob_hash,
        member_digests: &[],
        sealed_by: None,
        capability_digest: "",
    };
    let revoked = || {
        (
            StatusCode::CONFLICT,
            Json(json!({ "ok": false, "error": "revoked" })),
        )
            .into_response()
    };
    if revoked_on_this_peer(&archive_only) {
        return revoked();
    }

    let feeds = admit_export_feeds(properties.get("portal.feeds"));
    let hash_for_task = blob_hash.clone();
    let built =
        tokio::task::spawn_blocking(move || build_export(&store, &hash_for_task, &feeds)).await;
    match built {
        Ok(Ok((bytes, file_name))) => (
            [
                ("content-type", "application/zip".to_string()),
                (
                    "content-disposition",
                    format!("attachment; filename=\"{file_name}\""),
                ),
                ("x-content-type-options", "nosniff".to_string()),
            ],
            bytes,
        )
            .into_response(),
        Ok(Err(ExportFailure::Revoked)) => revoked(),
        Ok(Err(ExportFailure::Stored(StoredBundleError::BlobMissing))) => (
            StatusCode::CONFLICT,
            Json(json!({ "ok": false, "error": "bundle_bytes_absent" })),
        )
            .into_response(),
        Ok(Err(ExportFailure::Stored(err))) => stored_error_response(&asset_id, &err),
        Ok(Err(ExportFailure::UnsafeCsp(source))) => refusal_response(&refusal(
            RefusalClass::ManifestMalformed,
            format!("csp source {source} cannot appear in a header"),
        )),
        Ok(Err(ExportFailure::Zip)) => {
            tracing::error!(asset_id, "export archive write failed");
            error_json(StatusCode::INTERNAL_SERVER_ERROR, "export failed")
        }
        Err(e) => {
            tracing::error!("bundle export task failed: {e}");
            error_json(StatusCode::INTERNAL_SERVER_ERROR, "export failed")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// admission.md §8.2: static labels only, the share route only, GET and its preflight only.
    #[test]
    fn share_route_cors_admits_only_static_portal_origins_on_the_share_route() {
        let label = "abcdefghijklmnopqrstuvwxyz";
        let static_origin = format!("https://fe-portal.s-{label}.localhost");
        let admitted_origin = format!("https://fe-portal.a-{label}.localhost");
        let share = "/api/v1/shared/eyJ2IjoxfQ.c2ln";
        let rows = [
            ("static origin, GET", static_origin.clone(), "GET", share, None, true),
            ("static origin, preflight for GET", static_origin.clone(), "OPTIONS", share, Some("GET"), true),
            ("preflight for POST", static_origin.clone(), "OPTIONS", share, Some("POST"), false),
            ("POST", static_origin.clone(), "POST", share, None, false),
            ("admitted origin", admitted_origin, "GET", share, None, false),
            ("http spelling", format!("http://fe-portal.s-{label}.localhost"), "GET", share, None, false),
            ("explicit port", format!("{static_origin}:443"), "GET", share, None, false),
            ("uppercase", static_origin.to_uppercase(), "GET", share, None, false),
            ("short label", format!("https://fe-portal.s-{}.localhost", &label[1..]), "GET", share, None, false),
            ("extra host label", format!("https://x.fe-portal.s-{label}.localhost"), "GET", share, None, false),
            ("another route", static_origin.clone(), "GET", "/api/v1/query", None, false),
            ("empty token", static_origin.clone(), "GET", "/api/v1/shared/", None, false),
            ("nested path", static_origin, "GET", "/api/v1/shared/a/b", None, false),
        ];
        for (row, origin, method, path, requested, expected) in rows {
            let mut request = axum::http::Request::builder().method(method).uri(path);
            if let Some(requested) = requested {
                request = request.header("access-control-request-method", requested);
            }
            let (parts, ()) = request.body(()).expect("request").into_parts();
            let origin = HeaderValue::from_str(&origin).expect("origin header");
            assert_eq!(share_route_allows(&origin, &parts), expected, "{row}");
        }
    }
}
