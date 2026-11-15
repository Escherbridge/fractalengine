//! Reclaim a node's superseded published-row blob from the canonical store; see AGENTS.md §publish-reclaim.

use crate::{repo::Db, BlobHash, BlobStoreHandle};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

/// Nodes whose last published hash is remembered per process; past it the map restarts (one blob each may leak).
const TRACKED_NODES: usize = 65_536;

/// Last row hash this process enqueued per (blob store, node id).
static LAST_PUBLISHED: LazyLock<Mutex<HashMap<(usize, String), BlobHash>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Identity of one canonical store instance (each DB thread owns one handle).
fn store_key(blobs: &BlobStoreHandle) -> usize {
    std::sync::Arc::as_ptr(blobs).cast::<()>() as usize
}

/// Remember `hash` as the node's current published row and return the hash it replaced.
pub(super) fn record(blobs: &BlobStoreHandle, node_id: &str, hash: BlobHash) -> Option<BlobHash> {
    let mut map = LAST_PUBLISHED.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if map.len() >= TRACKED_NODES && !map.contains_key(&(store_key(blobs), node_id.to_owned())) {
        map.clear();
    }
    map.insert((store_key(blobs), node_id.to_owned()), hash)
}

/// Record the newly enqueued row and remove the previous one when nothing else can name it.
pub(super) async fn superseded(db: &Db, blobs: &BlobStoreHandle, node_id: &str, current: BlobHash) {
    let Some(previous) = record(blobs, node_id, current) else {
        return;
    };
    // Identical consecutive rows share one hash.
    if previous == current {
        return;
    }
    match still_named(db, blobs, &previous).await {
        Ok(false) => {
            if let Err(error) = blobs.remove_blob(&previous) {
                tracing::debug!(node_id, "superseded scene row blob kept: {error:#}");
            }
        }
        Ok(true) => tracing::debug!(node_id, "superseded scene row blob kept: still referenced"),
        Err(error) => tracing::debug!(node_id, "superseded scene row blob kept: reference check failed: {error:#}"),
    }
}

/// Whether an asset row, a hexon crate entry or another node's current published row names `hash`.
async fn still_named(db: &Db, blobs: &BlobStoreHandle, hash: &BlobHash) -> anyhow::Result<bool> {
    {
        let store = store_key(blobs);
        let map = LAST_PUBLISHED.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if map.iter().any(|((key_store, _), named)| *key_store == store && named == hash) {
            return Ok(true);
        }
    }
    let mut response = db
        .query("SELECT VALUE asset_id FROM asset WHERE content_hash=$reclaim_hash LIMIT 1 TIMEOUT 5s; SELECT VALUE entry_id FROM crate_entry WHERE asset_hash=$reclaim_hash LIMIT 1 TIMEOUT 5s")
        .bind(("reclaim_hash", crate::hash_to_hex(hash)))
        .await?
        .check()?;
    let assets: Vec<Value> = response.take(0)?;
    let entries: Vec<Value> = response.take(1)?;
    Ok(!assets.is_empty() || !entries.is_empty())
}
