//! Portal bundle ingest, file index and summary read (README §8; see `fe-api/AGENTS.md` §portal-bundles).

use std::path::{Path as FsPath, PathBuf};
use std::sync::{Arc, OnceLock};

use axum::extract::{Path, State};
use axum::http::StatusCode;
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
// Local admission seam (governance owns the record; README §8.5)
// ---------------------------------------------------------------------------

/// Admission state as governance reports it for this peer.
#[derive(Debug, Clone, Serialize)]
pub struct AdmissionView {
    pub state: String,
    pub reason: Option<String>,
}

/// Governance's read side: the host's own DID and its admission record per build.
pub trait LocalAdmissionSource: Send + Sync {
    /// The DID of this host's local identity.
    fn local_did(&self) -> String;
    /// Admission record for `(package_id, content_hash)` on this peer, if any.
    fn admission(&self, package_id: &str, content_hash: &str) -> Option<AdmissionView>;
}

static LOCAL_ADMISSION: OnceLock<Arc<dyn LocalAdmissionSource>> = OnceLock::new();

/// Register governance's admission reader once at startup (later calls are ignored).
pub fn set_local_admission_source(source: Arc<dyn LocalAdmissionSource>) {
    let _ = LOCAL_ADMISSION.set(source);
}

/// Admission block for the summary: present only for the host's local identity.
fn admission_for(claims: &ApiClaims, package_id: &str, content_hash: &str) -> Option<AdmissionView> {
    let source = LOCAL_ADMISSION.get()?;
    let local = source.local_did();
    (claims.sub == local || claims.client.as_deref() == Some(local.as_str()))
        .then(|| source.admission(package_id, content_hash))
        .flatten()
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

/// The raw URL policy applied to a bundle's declared origin (spec §6.1).
fn origin_allowed(origin: &str) -> bool {
    fe_runtime::portal_url::embedded_allowed(origin)
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

/// Refuse (before any byte is stored) an upload that would pass the owner's quota; fails closed.
async fn check_quota(state: &ApiState, owner_did: &str, incoming: u64) -> Result<(), Response> {
    let quota = state.upload_quota_bytes;
    let rows = crate::gis::run_select(
        state,
        "SELECT math::sum(size_bytes) AS used FROM asset WHERE owner_did = $owner GROUP ALL",
        vec![("owner".to_string(), json!(owner_did))],
    )
    .await
    .ok_or_else(|| {
        tracing::warn!("bundle quota lookup failed; refusing upload");
        error_json(StatusCode::BAD_GATEWAY, "asset registration failed")
    })?;
    let used = rows
        .first()
        .and_then(|row| row.get("used"))
        .and_then(|used| used.as_u64().or_else(|| used.as_f64().map(|f| f as u64)))
        .unwrap_or(0);
    if used.saturating_add(incoming) > quota {
        return Err(error_json(
            StatusCode::PAYLOAD_TOO_LARGE,
            &format!("upload quota exceeded: this owner may register at most {quota} asset bytes"),
        ));
    }
    Ok(())
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
    if let Err(denied) = check_quota(&state, &owner_did, bytes.len() as u64).await {
        return denied;
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
    if let Some(view) = admission_for(
        &claims,
        &inspected.validated.package_id,
        &inspected.content_hash,
    ) {
        fields.insert("admission".into(), json!(view));
    }
    Json(ApiResponse::success(Value::Object(fields))).into_response()
}
