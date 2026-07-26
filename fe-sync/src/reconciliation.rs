//! Inbound P2P reconciliation — applies a peer's delta to the durable store
//! through the tombstone-honoring merge so a stale replica can never resurrect a
//! deleted node (FR-1 / N-4). See fe-sync/src/AGENTS.md §lifecycle-forwarding.

use crate::replication::ReplicationStore;
use fe_database::merge::{apply_replicated_node, MergeApplied};
use fe_database::{DbHandle, PetalId};

/// Reconcile `petal_id` against `peer_id`: pull the delta and apply each row to
/// the durable node table via [`apply_replicated_node`]. That merge refuses to
/// resurrect a locally-tombstoned node (N-4) and converges an incoming tombstone
/// — previously this counted bytes and ignored the durable store entirely,
/// letting a stale replica silently re-create a deleted node. Returns the number
/// of rows actually applied (refused resurrections are not counted).
pub async fn reconcile_petal(
    petal_id: &PetalId,
    peer_id: &str,
    store: &dyn ReplicationStore,
    db_handle: &DbHandle,
) -> anyhow::Result<u64> {
    let delta_bytes = store.sync(peer_id, petal_id)?;
    let entries: Vec<serde_json::Value> = serde_json::from_slice(&delta_bytes).unwrap_or_default();

    let db = db_handle.0.as_ref();
    let mut applied = 0u64;
    for entry in &entries {
        let bytes = match serde_json::to_vec(entry) {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!("reconcile: skipping unserializable entry: {e}");
                continue;
            }
        };
        match apply_replicated_node(db, &bytes).await {
            Ok(MergeApplied::SkippedTombstoned) => {
                tracing::debug!("reconcile: refused to resurrect a tombstoned node (N-4)");
            }
            Ok(MergeApplied::NotANode) => {}
            Ok(MergeApplied::Applied) | Ok(MergeApplied::AppliedTombstone) => applied += 1,
            Err(e) => tracing::warn!("reconcile: merge apply failed: {e}"),
        }
    }
    Ok(applied)
}
