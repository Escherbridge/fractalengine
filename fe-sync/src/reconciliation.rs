use crate::replication::ReplicationStore;
use fe_database::{DbHandle, PetalId};

pub async fn reconcile_petal(
    petal_id: &PetalId,
    peer_id: &str,
    store: &dyn ReplicationStore,
    _db_handle: &DbHandle,
) -> anyhow::Result<u64> {
    let delta_bytes = store.sync(peer_id, petal_id)?;
    let entries: Vec<serde_json::Value> = serde_json::from_slice(&delta_bytes).unwrap_or_default();
    Ok(entries.len() as u64)
}
