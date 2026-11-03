//! Private scene-asset transfer admission and native demand; see AGENTS.md.

use crate::{blob_store::BlobHash, scene_replication::ReplicaNodeProvenance};
use std::time::Instant;

pub const MAX_SCENE_ASSET_BYTES: u64 = 256 * 1024 * 1024;

/// The DB resolves all identities and authority; a request is never an authority token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SceneAssetTransferPurpose {
    Publish,
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
    pub node_id: String,
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
