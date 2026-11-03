//! Replica current-state projections without local journal claims; see src/AGENTS.md.

use crate::node_history::NodeRowSnapshot;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ReplicaNodeProvenance {
    OriginalSceneCommit { hlc: u64, publisher: String },
    LegacyReplicaApply,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplicaAssetUnavailable {
    MissingMetadata,
    AmbiguousMetadata,
    ForeignAnchor,
    UnanchoredMetadata,
    InvalidMetadata,
    UnsupportedSceneAsset,
    MissingBlob,
    TransferLimitExceeded,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ReplicaNodeAsset {
    NoAsset,
    Unavailable {
        asset_id: String,
        reason: ReplicaAssetUnavailable,
    },
    LocalBlob {
        asset_id: String,
        content_hash: String,
        content_type: String,
        asset_path: String,
    },
}

impl ReplicaNodeAsset {
    pub fn asset_id(&self) -> Option<&str> {
        match self {
            Self::NoAsset => None,
            Self::Unavailable { asset_id, .. } | Self::LocalBlob { asset_id, .. } => Some(asset_id),
        }
    }
    pub fn asset_path(&self) -> Option<&str> {
        match self {
            Self::LocalBlob { asset_path, .. } => Some(asset_path),
            _ => None,
        }
    }
}

/// Stored current row read after successful replica apply; never a local mutation receipt.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplicatedNodeCurrent {
    pub scope: String,
    pub state: NodeRowSnapshot,
    pub provenance: ReplicaNodeProvenance,
    pub asset: ReplicaNodeAsset,
}
