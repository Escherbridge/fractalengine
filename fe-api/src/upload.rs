//! GLB asset ingest — validate → store API-side → register metadata (F13;
//! mcp_scene_primitives FR-1/FR-2/FR-3/FR-7). The bytes-never-cross-the-channel
//! invariant and limits rationale: see `fe-api/AGENTS.md` §asset-ingest.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use axum_extra::extract::Multipart;
use base64::Engine as _;
use fe_identity::api_token::ApiClaims;
use fe_runtime::blob_store::{hash_to_hex, BlobStoreHandle};
use fe_runtime::messages::{DbCommand, DbResult};
use serde::Serialize;

use crate::auth::{require_role, require_scope};
use crate::server::ApiState;
use crate::types::{is_valid_ulid, ApiResponse};

/// Max decoded GLB size, both transports (tech-stack "configurable per Node" seam).
pub const MAX_ASSET_BYTES: usize = 256 * 1024 * 1024;
/// Per-route body limit for the REST multipart upload (asset + form overhead).
pub const ASSET_ROUTE_BODY_LIMIT: usize = MAX_ASSET_BYTES + 1024 * 1024;
/// `/mcp` body limit: a max-size asset as base64 (4/3) plus JSON-RPC overhead.
pub const MCP_ROUTE_BODY_LIMIT: usize = MAX_ASSET_BYTES.div_ceil(3) * 4 + 8 * 1024 * 1024;
/// Content type every ingested asset row carries (GLB only — FR-7).
pub const GLB_CONTENT_TYPE: &str = "model/gltf-binary";
/// Longest accepted asset display name.
const MAX_ASSET_NAME_LEN: usize = 255;
/// GLB header: magic + version + total length, each u32 LE.
const GLB_HEADER_LEN: usize = 12;

/// Why an upload was refused. Each variant is a distinct, actionable message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UploadError {
    /// `ApiState.blob_store` is `None` — fail closed (503).
    StoreNotConfigured,
    InvalidName,
    InvalidBase64,
    TooLarge {
        max: usize,
    },
    Truncated,
    BadMagic,
    UnsupportedVersion(u32),
    LengthMismatch {
        declared: u64,
        actual: u64,
    },
    /// Blob write / hashing failed (logged; generic to the client).
    StoreFailed,
    /// The DB thread refused or never answered the `CreateAsset` metadata.
    RegisterFailed,
}

impl UploadError {
    /// Client-facing message (internal causes go to `tracing`).
    pub fn message(&self) -> String {
        match self {
            Self::StoreNotConfigured => "asset upload unavailable: no blob store configured".into(),
            Self::InvalidName => {
                format!("name must be 1..={MAX_ASSET_NAME_LEN} printable characters")
            }
            Self::InvalidBase64 => "data_base64 is not valid standard base64".into(),
            Self::TooLarge { max } => format!("asset exceeds the {max}-byte limit"),
            Self::Truncated => "not a GLB: shorter than the 12-byte header".into(),
            Self::BadMagic => {
                "not a GLB: missing the glTF binary magic (JSON .gltf is not accepted)".into()
            }
            Self::UnsupportedVersion(v) => format!("unsupported GLB version {v} (only 2)"),
            Self::LengthMismatch { declared, actual } => {
                format!("GLB header declares {declared} bytes but {actual} were sent")
            }
            Self::StoreFailed => "asset storage failed".into(),
            Self::RegisterFailed => "asset registration failed".into(),
        }
    }

    /// HTTP status for the REST surface.
    pub fn status(&self) -> StatusCode {
        match self {
            Self::StoreNotConfigured => StatusCode::SERVICE_UNAVAILABLE,
            Self::TooLarge { .. } => StatusCode::PAYLOAD_TOO_LARGE,
            Self::StoreFailed | Self::RegisterFailed => StatusCode::BAD_GATEWAY,
            _ => StatusCode::BAD_REQUEST,
        }
    }
}

/// Incoming asset bytes in their transport encoding (decoded off the async workers).
pub enum GlbPayload {
    /// Raw bytes (REST multipart).
    Bytes(Vec<u8>),
    /// Standard base64 text (MCP JSON).
    Base64(String),
}

/// A registered asset — the only thing a caller ever gets back.
#[derive(Debug, Clone, Serialize)]
pub struct IngestedAsset {
    pub asset_id: String,
    pub content_hash: String,
    pub size_bytes: u64,
}

/// Proof that validated bytes now live in the blob store. Holds metadata ONLY
/// and is constructible only by [`store_glb`], which consumes (and drops) the
/// bytes — so the `CreateAsset` sender ([`register_asset`]) structurally
/// cannot put bytes on the crossbeam channel.
pub struct StoredGlb {
    content_hash: String,
    size_bytes: u64,
}

/// FR-7 GLB header check: size cap, 12-byte header, `glTF` magic, version 2,
/// declared total length == actual length.
pub fn validate_glb(bytes: &[u8], max_len: usize) -> Result<(), UploadError> {
    if bytes.len() > max_len {
        return Err(UploadError::TooLarge { max: max_len });
    }
    if bytes.len() < GLB_HEADER_LEN {
        return Err(UploadError::Truncated);
    }
    if !bytes.starts_with(b"glTF") {
        return Err(UploadError::BadMagic);
    }
    let version = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
    if version != 2 {
        return Err(UploadError::UnsupportedVersion(version));
    }
    let declared = u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]) as u64;
    let actual = bytes.len() as u64;
    if declared != actual {
        return Err(UploadError::LengthMismatch { declared, actual });
    }
    Ok(())
}

/// Decode standard base64, refusing (before allocating) any text whose
/// decoded size could exceed `max_len`.
pub fn decode_base64_capped(text: &str, max_len: usize) -> Result<Vec<u8>, UploadError> {
    if text.len() > max_len.div_ceil(3) * 4 {
        return Err(UploadError::TooLarge { max: max_len });
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(text)
        .map_err(|_| UploadError::InvalidBase64)?;
    if bytes.len() > max_len {
        return Err(UploadError::TooLarge { max: max_len });
    }
    Ok(bytes)
}

/// Decode → validate → hash + write. Synchronous (run under `spawn_blocking`).
/// The bytes are consumed here and never leave this function.
fn store_glb(
    store: &BlobStoreHandle,
    payload: GlbPayload,
    max_len: usize,
) -> Result<StoredGlb, UploadError> {
    let bytes = match payload {
        GlbPayload::Bytes(bytes) => bytes,
        GlbPayload::Base64(text) => decode_base64_capped(&text, max_len)?,
    };
    validate_glb(&bytes, max_len)?;
    let hash = store.add_blob(&bytes).map_err(|e| {
        tracing::error!("asset blob write failed: {e}");
        UploadError::StoreFailed
    })?;
    Ok(StoredGlb {
        content_hash: hash_to_hex(&hash),
        size_bytes: bytes.len() as u64,
    })
}

/// Send the metadata-only `CreateAsset` and await its correlated reply.
async fn register_asset(
    state: &ApiState,
    name: String,
    stored: StoredGlb,
) -> Result<IngestedAsset, UploadError> {
    let correlation_id = ulid::Ulid::new().to_string();
    let cmd = DbCommand::CreateAsset {
        name,
        content_type: GLB_CONTENT_TYPE.to_string(),
        size_bytes: stored.size_bytes,
        content_hash: stored.content_hash.clone(),
        correlation_id: Some(correlation_id.clone()),
    };
    match crate::rest::db_round_trip(state, cmd).await {
        Ok(DbResult::AssetCreated {
            asset_id,
            content_hash,
            size_bytes,
            correlation_id: Some(echoed),
        }) if echoed == correlation_id && content_hash == stored.content_hash => {
            Ok(IngestedAsset {
                asset_id,
                content_hash,
                size_bytes,
            })
        }
        Ok(DbResult::Error(e)) => {
            tracing::warn!("CreateAsset refused: {e}");
            Err(UploadError::RegisterFailed)
        }
        Ok(other) => {
            tracing::error!("CreateAsset reply mismatch (not ours): {other:?}");
            Err(UploadError::RegisterFailed)
        }
        Err(e) => {
            tracing::warn!("CreateAsset round trip failed: {e}");
            Err(UploadError::RegisterFailed)
        }
    }
}

/// Shared ingest core for both transports, with an injectable size cap.
/// The caller has authorized the anchor petal.
pub async fn ingest_glb_with_limit(
    state: &ApiState,
    name: &str,
    payload: GlbPayload,
    max_len: usize,
) -> Result<IngestedAsset, UploadError> {
    let name = name.trim();
    if name.is_empty()
        || name.chars().count() > MAX_ASSET_NAME_LEN
        || name.chars().any(char::is_control)
    {
        return Err(UploadError::InvalidName);
    }
    let Some(store) = state.blob_store.clone() else {
        return Err(UploadError::StoreNotConfigured);
    };
    let stored = tokio::task::spawn_blocking(move || store_glb(&store, payload, max_len))
        .await
        .map_err(|e| {
            tracing::error!("asset store task failed: {e}");
            UploadError::StoreFailed
        })??;
    register_asset(state, name.to_string(), stored).await
}

/// [`ingest_glb_with_limit`] at [`MAX_ASSET_BYTES`].
pub async fn ingest_glb(
    state: &ApiState,
    name: &str,
    payload: GlbPayload,
) -> Result<IngestedAsset, UploadError> {
    ingest_glb_with_limit(state, name, payload, MAX_ASSET_BYTES).await
}

/// A node placed from an asset (MCP `place_asset`).
#[derive(Debug, Clone, Serialize)]
pub struct PlacedAsset {
    pub node_id: String,
    pub asset_id: String,
    pub petal_id: String,
    pub name: String,
    pub asset_path: String,
    pub position: [f32; 3],
}

/// Place-asset core (caller has authorized the petal): `CreateNodeWithAsset`
/// with a correlated reply; the DB thread validates the asset row exists.
pub async fn place_asset_core(
    state: &ApiState,
    petal_id: &str,
    name: &str,
    asset_id: &str,
    position: [f32; 3],
    rotation: [f32; 3],
    scale: [f32; 3],
) -> Result<PlacedAsset, String> {
    let correlation_id = ulid::Ulid::new().to_string();
    let cmd = DbCommand::CreateNodeWithAsset {
        petal_id: petal_id.to_string(),
        name: name.to_string(),
        asset_id: asset_id.to_string(),
        position,
        rotation,
        scale,
        correlation_id: Some(correlation_id.clone()),
    };
    match crate::rest::db_round_trip(state, cmd).await? {
        DbResult::GltfImported {
            node_id,
            asset_id,
            petal_id,
            name,
            asset_path,
            position,
            correlation_id: Some(echoed),
        } if echoed == correlation_id => Ok(PlacedAsset {
            node_id,
            asset_id,
            petal_id,
            name,
            asset_path,
            position,
        }),
        DbResult::Error(e) => {
            tracing::warn!("place_asset refused: {e}");
            if e.contains("matched no asset") {
                Err("unknown asset_id".to_string())
            } else {
                Err("operation failed".to_string())
            }
        }
        other => {
            tracing::error!("CreateNodeWithAsset reply mismatch (not ours): {other:?}");
            Err("operation failed".to_string())
        }
    }
}

// ---------------------------------------------------------------------------
// REST: POST /api/v1/petals/{petal_id}/assets (FR-1)
// ---------------------------------------------------------------------------

fn upload_error_response(e: &UploadError) -> Response {
    (
        e.status(),
        Json(ApiResponse::<serde_json::Value>::error(e.message())),
    )
        .into_response()
}

fn deny(status: StatusCode, msg: &str) -> Response {
    (status, Json(ApiResponse::<serde_json::Value>::error(msg))).into_response()
}

/// POST /api/v1/petals/{petal_id}/assets — multipart GLB upload (`file`
/// field; optional `name` field, else the file name). Editor+ at the petal's
/// DB-resolved scope; 201 `{asset_id, content_hash, size_bytes}`.
pub async fn upload_petal_asset(
    State(state): State<Arc<ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    Path(petal_id): Path<String>,
    mut multipart: Multipart,
) -> Response {
    if require_role(&claims, "editor").is_err() {
        return deny(StatusCode::FORBIDDEN, "insufficient permissions");
    }
    if !is_valid_ulid(&petal_id) {
        return deny(StatusCode::BAD_REQUEST, "invalid petal_id");
    }
    let Some(scope) = crate::rest::resolve_petal_scope(&state, &petal_id).await else {
        return deny(StatusCode::NOT_FOUND, "petal not found");
    };
    if require_scope(&claims, &scope).is_err() {
        return deny(StatusCode::FORBIDDEN, "insufficient scope");
    }
    if state.blob_store.is_none() {
        return upload_error_response(&UploadError::StoreNotConfigured);
    }

    let mut file: Option<(Vec<u8>, Option<String>)> = None;
    let mut name_field: Option<String> = None;
    loop {
        match multipart.next_field().await {
            Ok(Some(field)) => match field.name() {
                Some("file") => {
                    let file_name = field.file_name().map(str::to_string);
                    match field.bytes().await {
                        Ok(bytes) => file = Some((bytes.to_vec(), file_name)),
                        Err(e) => {
                            return deny(
                                StatusCode::BAD_REQUEST,
                                &format!("failed to read upload: {e}"),
                            )
                        }
                    }
                }
                Some("name") => name_field = field.text().await.ok(),
                _ => {}
            },
            Ok(None) => break,
            Err(e) => return deny(StatusCode::BAD_REQUEST, &format!("multipart error: {e}")),
        }
    }
    let Some((bytes, file_name)) = file else {
        return deny(
            StatusCode::BAD_REQUEST,
            "missing 'file' field in multipart upload",
        );
    };
    let name = name_field
        .or(file_name)
        .unwrap_or_else(|| "asset.glb".to_string());

    match ingest_glb(&state, &name, GlbPayload::Bytes(bytes)).await {
        Ok(asset) => (
            StatusCode::CREATED,
            Json(ApiResponse::success(
                serde_json::to_value(asset).unwrap_or_default(),
            )),
        )
            .into_response(),
        Err(e) => upload_error_response(&e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn glb(total_len: u32, version: u32, actual_len: usize) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(actual_len.max(GLB_HEADER_LEN));
        bytes.extend_from_slice(b"glTF");
        bytes.extend_from_slice(&version.to_le_bytes());
        bytes.extend_from_slice(&total_len.to_le_bytes());
        bytes.resize(actual_len.max(GLB_HEADER_LEN), 0);
        bytes
    }

    /// (label, bytes, max_len, expected).
    type GlbCase = (&'static str, Vec<u8>, usize, Result<(), UploadError>);

    #[test]
    fn validate_glb_table() {
        let cases: Vec<GlbCase> = vec![
            ("valid 12-byte header", glb(12, 2, 12), 64, Ok(())),
            ("valid with body", glb(40, 2, 40), 64, Ok(())),
            (
                "bad magic",
                b"JSON\x02\0\0\0\x0c\0\0\0".to_vec(),
                64,
                Err(UploadError::BadMagic),
            ),
            (
                "version 1",
                glb(12, 1, 12),
                64,
                Err(UploadError::UnsupportedVersion(1)),
            ),
            (
                "truncated",
                b"glTF\x02\0".to_vec(),
                64,
                Err(UploadError::Truncated),
            ),
            (
                "header length mismatch",
                glb(99, 2, 20),
                64,
                Err(UploadError::LengthMismatch {
                    declared: 99,
                    actual: 20,
                }),
            ),
            (
                "oversize (injected cap)",
                glb(40, 2, 40),
                39,
                Err(UploadError::TooLarge { max: 39 }),
            ),
        ];
        for (label, bytes, max, want) in cases {
            assert_eq!(validate_glb(&bytes, max), want, "{label}");
        }
    }

    #[test]
    fn base64_cap_refuses_before_decoding() {
        let encoded = base64::engine::general_purpose::STANDARD.encode(glb(40, 2, 40));
        assert_eq!(
            decode_base64_capped(&encoded, 12),
            Err(UploadError::TooLarge { max: 12 })
        );
        assert_eq!(
            decode_base64_capped("!!not base64!!", 64),
            Err(UploadError::InvalidBase64)
        );
        assert_eq!(decode_base64_capped(&encoded, 64).map(|b| b.len()), Ok(40));
    }
}
