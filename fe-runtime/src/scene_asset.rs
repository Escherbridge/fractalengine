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
