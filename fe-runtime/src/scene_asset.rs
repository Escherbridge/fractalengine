//! Private scene-asset transfer admission and native demand; see AGENTS.md.

use crate::{blob_store::BlobHash, scene_replication::ReplicaNodeProvenance};
use std::time::Instant;

pub const MAX_SCENE_ASSET_BYTES: u64 = 256 * 1024 * 1024;
pub const SCENE_ASSET_ROW_MARKER: &str = "_fe_scene_asset";
/// The MIME types a scene asset row may declare (transfer, publication and upload share it).
pub const SCENE_ASSET_MIMES: [&str; 2] = ["model/gltf-binary", "model/gltf+json"];

/// The DB resolves all identities and authority; a request is never an authority token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SceneAssetTransferPurpose {
    Publish { asset_id: String, petal_id: String },
    Fetch,
    Serve { requester_did: String },
}

#[derive(Debug, Clone)]
pub struct SceneAssetTransferRequest {
    pub verse_id: String,
    pub hash: BlobHash,
    pub node_id: Option<String>,
    pub purpose: SceneAssetTransferPurpose,
    pub deadline: Instant,
}

/// Exact current admission witness; callers must re-read before persisting or reporting readiness.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SceneAssetTransferPermit {
    pub verse_id: String,
    pub petal_id: String,
    /// The live reference the request named; `None` for an anchored asset admitted on its
    /// own metadata (QA-0158: an upload is Verse content before its first placement).
    pub node_id: Option<String>,
    pub asset_id: String,
    pub content_hash: BlobHash,
    pub content_type: String,
    pub size_bytes: u64,
    pub asset_fingerprint: String,
}

/// Missing bytes under an actual current read; this hint grants no transfer authority.
#[derive(Debug, Clone)]
pub struct SceneAssetNeeded {
    pub verse_id: String,
    pub scope: String,
    pub node_id: String,
    pub asset_id: String,
    pub content_hash: BlobHash,
    pub size_bytes: u64,
    pub provenance: ReplicaNodeProvenance,
}

/// Owner-reviewed exact local-copy adoption; not a claim about the original uploader.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SceneAssetAdoptionCandidate {
    pub node_id: String,
    pub asset_id: String,
    pub scope: String,
    pub content_hash: BlobHash,
    pub content_type: String,
    pub size_bytes: u64,
    pub asset_fingerprint: String,
    pub reference_fingerprints: Vec<SceneAssetReferenceWitness>,
    pub model_fingerprint: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SceneAssetAdoptionReceipt {
    pub candidate: SceneAssetAdoptionCandidate,
    pub operation_id: String,
    pub adopter_did: String,
    pub adopted_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SceneAssetReferenceWitness {
    pub node_id: String,
    pub fingerprint: String,
}

/// Verify bounded canonical bytes independently of any transport or metadata authority.
pub fn verify_scene_asset_bytes(
    bytes: &[u8],
    hash: &BlobHash,
    content_type: &str,
    size_bytes: u64,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        size_bytes > 0 && size_bytes <= MAX_SCENE_ASSET_BYTES && bytes.len() as u64 == size_bytes,
        "scene asset size differs from bounded metadata"
    );
    anyhow::ensure!(
        *blake3::hash(bytes).as_bytes() == *hash,
        "scene asset digest mismatch"
    );
    let extension = match content_type {
        "model/gltf-binary" => {
            anyhow::ensure!(
                bytes.starts_with(b"glTF"),
                "GLB MIME requires a GLB container"
            );
            "glb"
        }
        "model/gltf+json" => {
            let document: serde_json::Value = serde_json::from_slice(bytes)?;
            anyhow::ensure!(
                document["asset"]["version"]
                    .as_str()
                    .is_some_and(|version| version.starts_with("2.")),
                "glTF MIME requires a glTF 2 document"
            );
            "gltf"
        }
        _ => anyhow::bail!("unsupported scene asset MIME"),
    };
    crate::ktx2_guard::check_asset_bytes(bytes, Some(extension))
        .map_err(|error| anyhow::anyhow!("scene asset refused: {error}"))?;
    Ok(())
}

/// Private metadata-only audit protocol; see AGENTS.md scene-asset-publication-audit.
pub const SCENE_ASSET_AUDIT_LIMIT: std::time::Duration = std::time::Duration::from_secs(8);
pub const SCENE_ASSET_AUDIT_RESPONSE_MAX: usize = 256 * 1024;
pub const SCENE_ASSET_AUDIT_INTERNAL_MAX: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct SceneAssetAuditCaller {
    pub did: String,
    pub scope: String,
    pub role_ceiling: String,
}

#[derive(Debug, Clone)]
pub struct SceneAssetAuditRequest {
    pub node_id: String,
    pub operation_id: Option<String>,
    pub caller: SceneAssetAuditCaller,
    pub deadline: Instant,
}
impl SceneAssetAuditRequest {
    pub fn is_current(&self) -> bool {
        let now = Instant::now();
        self.deadline > now && self.deadline <= now + SCENE_ASSET_AUDIT_LIMIT
    }
    pub fn validate_ids(&self) -> bool {
        crate::safe_id::is_safe_path_id(&self.node_id)
            && self
                .operation_id
                .as_ref()
                .is_none_or(|id| is_canonical_audit_ulid(id))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "state", content = "value", rename_all = "snake_case")]
pub enum SceneAssetField<T> {
    Absent,
    Null,
    Value(T),
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SceneAssetAuditProvenance {
    pub kind: String,
    pub operation_id: String,
    pub adopter_did: String,
    pub adopted_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SceneAssetAuditReference {
    pub node_id: String,
    pub petal_id: String,
    pub state_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SceneAssetAuditOperation {
    pub operation_id: String,
    pub op_type: String,
    pub node_id: String,
    pub scope: String,
    pub asset_id: String,
    pub content_hash: String,
    pub content_type: String,
    pub size_bytes: u64,
    pub provenance: SceneAssetAuditProvenance,
    pub state_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SceneAssetPublicationAudit {
    pub protocol_version: u8,
    pub request_nonce: String,
    pub digest_algorithm: String,
    pub node_id: String,
    pub asset_id: String,
    pub petal_id: String,
    pub scope: String,
    pub content_hash: String,
    pub content_type: String,
    pub size_bytes: u64,
    pub owner_did: SceneAssetField<String>,
    pub asset_petal_id: SceneAssetField<String>,
    pub publication_provenance: SceneAssetField<SceneAssetAuditProvenance>,
    pub asset_state_digest: String,
    pub references: Vec<SceneAssetAuditReference>,
    pub model_state_digest: Option<String>,
    pub payload_reference_count: u8,
    pub adoption_operation: Option<SceneAssetAuditOperation>,
    pub bytes_verified_by_this_read: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SceneAssetAuditFailure {
    Unavailable,
    Expired,
    Busy,
    InvalidRequest,
}

/// Canonical uppercase Crockford ULID, including its 128-bit leading-digit bound.
pub fn is_canonical_audit_ulid(id: &str) -> bool {
    id.len() == 26
        && id.as_bytes()[0] <= b'7'
        && id
            .bytes()
            .all(|b| b"0123456789ABCDEFGHJKMNPQRSTVWXYZ".contains(&b))
}
