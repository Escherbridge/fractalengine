//! Sync thread — owns the iroh endpoint and processes [`SyncCommand`]s
//! (P2P Mycelium Phase D).
//!
//! Modelled after `fe-database::spawn_db_thread`: a dedicated OS thread with
//! its own single-threaded Tokio runtime.  If the iroh endpoint fails to
//! bind the thread enters **offline mode** — it stays alive, responds to
//! commands, but all network-dependent operations are no-ops.

use std::collections::HashMap;
use std::sync::Arc;

use fe_runtime::blob_store::{hash_to_hex, BlobStoreHandle};

use iroh_gossip::net::{Gossip, GossipTopic};
use iroh_gossip::proto::TopicId;

use crate::endpoint::SyncEndpoint;
use crate::messages::{SyncCommand, SyncCommandReceiver, SyncEvent, SyncEventSender, TransformUpdate};
use crate::replicator::{
    IrohDocsEngineHolder, IrohDocsReplicator, IrohPetalReplicator, PetalReplicator, VerseReplicator,
};
use crate::verse_peers;

/// Derive an iroh-gossip 0.35 `TopicId` (32 bytes) from a topic key string.
fn gossip_topic_id(topic_key: &str) -> TopicId {
    let bytes: [u8; 32] = *blake3::hash(topic_key.as_bytes()).as_bytes();
    TopicId::from(bytes)
}

/// Spawn the sync thread.
///
/// Returns a join handle so the caller can optionally wait for a clean
/// shutdown.
///
/// # Arguments
/// * `secret_key` — deterministic ed25519 seed for the iroh endpoint.
/// * `blob_store` — shared content-addressed blob store.
/// * `cmd_rx` — receives [`SyncCommand`]s from the main / Bevy thread.
/// * `evt_tx` — sends [`SyncEvent`]s back to the main / Bevy thread.
/// * `local_did` — the node's DID (`did:key:z6Mk…`) used as author ID in replicas.
pub fn spawn_sync_thread(
    secret_key: iroh::SecretKey,
    blob_store: BlobStoreHandle,
    cmd_rx: SyncCommandReceiver,
    evt_tx: SyncEventSender,
    local_did: String,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("Failed to build sync Tokio runtime");

        rt.block_on(async move {
            // Phase F.1: Create the iroh endpoint first
            let endpoint = match SyncEndpoint::new(secret_key).await {
                Ok(ep) => {
                    tracing::info!(
                        node_id = %ep.node_id(),
                        "Sync thread started (online)"
                    );
                    evt_tx.send(SyncEvent::Started { online: true }).ok();
                    Some(ep)
                }
                Err(e) => {
                    tracing::warn!("Sync thread could not bind iroh endpoint: {e}");
                    tracing::warn!("Running in offline mode — network fetch disabled");
                    evt_tx.send(SyncEvent::Started { online: false }).ok();
                    None
                }
            };

            // Phase F.1: iroh-docs Engine holder.
            // TODO(iroh-0.35): real Engine<D> wiring is deferred (needs the full
            // P2P stack — gossip, blob store, downloader, local pool). Replicators
            // use the in-memory fallback until then. See AGENTS.md §iroh-0.35.
            let docs_engine_holder = Arc::new(IrohDocsEngineHolder::new());

            // Phase F.3: Create iroh-gossip 0.35 Gossip instance from the endpoint.
            let gossip_host: Option<Gossip> = match endpoint.as_ref() {
                Some(ep) => match Gossip::builder().spawn(ep.inner().clone()).await {
                    Ok(gossip) => {
                        tracing::info!("iroh-gossip Gossip initialized");
                        Some(gossip)
                    }
                    Err(e) => {
                        tracing::warn!("Failed to spawn gossip: {e}");
                        None
                    }
                },
                None => {
                    tracing::debug!("No endpoint, skipping gossip creation");
                    None
                }
            };

            // Track active gossip subscriptions (topic key -> live handle) for verse/petal.
            let mut gossip_topics: HashMap<String, GossipTopic> = HashMap::new();

            // Phase E: per-verse replica map.
            let mut replicas: HashMap<String, Box<dyn VerseReplicator>> = HashMap::new();

            // Phase F.2: per-petal replica map.
            let mut petal_replicas: HashMap<String, Box<dyn PetalReplicator>> = HashMap::new();

            // Phase F.5: tileset download tracker
            let mut download_tracker = TilesetDownloadTracker::new();

            // §D1: policy gate for replica row writes. Defaults to migration
            // mode (warn-logs would-be denies) until peer roles are plumbed —
            // see AGENTS.md §write-policy.
            let write_policy = crate::write_policy::PolicyHandle::default();

            // Command loop
            loop {
                match cmd_rx.recv() {
                    Ok(SyncCommand::FetchBlob { hash, verse_id }) => {
                        handle_fetch_blob(
                            &blob_store,
                            endpoint.as_ref(),
                            &hash,
                            &verse_id,
                            &evt_tx,
                        );
                    }
                    Ok(SyncCommand::OpenVerseReplica {
                        verse_id,
                        namespace_id,
                        namespace_secret,
                    }) => {
                        handle_open_verse_replica(
                            &mut replicas,
                            docs_engine_holder.clone(),
                            &verse_id,
                            &namespace_id,
                            namespace_secret,
                            &local_did,
                        );
                        // Phase F.4: Subscribe to verse gossip topic
                        subscribe_to_verse_gossip_topic(
                            &gossip_host,
                            &mut gossip_topics,
                            &verse_id,
                        );
                    }
                    Ok(SyncCommand::CloseVerseReplica { verse_id }) => {
                        handle_close_verse_replica(&mut replicas, &verse_id);
                        // Phase F.4: Unsubscribe from verse gossip topic
                        unsubscribe_from_verse_gossip_topic(
                            &gossip_host,
                            &mut gossip_topics,
                            &verse_id,
                        );
                    }
                    Ok(SyncCommand::WriteRowEntry {
                        verse_id,
                        table,
                        record_id,
                        content_hash,
                    }) => {
                        handle_write_row_entry(
                            &replicas,
                            &blob_store,
                            &write_policy,
                            &local_did,
                            &verse_id,
                            &table,
                            &record_id,
                            &content_hash,
                        )
                        .await;
                    }
                    Ok(SyncCommand::UpdateNodeTransform {
                        verse_id,
                        node_id,
                        position,
                        rotation,
                        scale,
                    }) => {
                        handle_update_node_transform(
                            &gossip_host,
                            &mut gossip_topics,
                            &verse_id,
                            &node_id,
                            position,
                            rotation,
                            scale,
                            &local_did,
                        )
                        .await;
                    }
                    Ok(SyncCommand::SubscribePetal { petal_id }) => {
                        handle_subscribe_petal(
                            &mut petal_replicas,
                            docs_engine_holder.clone(),
                            &petal_id,
                            &local_did,
                        );
                    }
                    Ok(SyncCommand::UnsubscribePetal { petal_id }) => {
                        handle_unsubscribe_petal(&mut petal_replicas, &petal_id);
                    }
                    Ok(SyncCommand::SubmitComputeTask {
                        task_id,
                        query,
                        petal_scope,
                        requester_did,
                    }) => {
                        // TODO(Phase 6.2): execute compute task locally, emit ComputeResultReady
                        tracing::debug!(%task_id, %query, ?petal_scope, %requester_did, "SubmitComputeTask (stub)");
                    }
                    Ok(SyncCommand::AdvertiseTilesets { verse_id, advertisements_json }) => {
                        handle_advertise_tilesets(
                            &gossip_host,
                            &gossip_topics,
                            &advertisements_json,
                            &verse_id,
                        )
                        .await;
                    }
                    Ok(SyncCommand::RequestTilesetMeta { peer_id, tileset_id }) => {
                        handle_request_tileset_meta(&peer_id, &tileset_id, &evt_tx);
                    }
                    Ok(SyncCommand::RequestChunk { peer_id, tileset_id, chunk_seq }) => {
                        handle_request_chunk(&peer_id, &tileset_id, chunk_seq, &evt_tx);
                    }
                    Ok(SyncCommand::CancelTilesetDownload { tileset_id }) => {
                        handle_cancel_tileset_download(&mut download_tracker, &tileset_id);
                    }
                    Ok(SyncCommand::Shutdown) => {
                        tracing::info!("Sync thread shutting down");
                        break;
                    }
                    Err(_) => {
                        // Channel closed — main thread dropped the sender.
                        tracing::info!("Sync command channel closed, shutting down");
                        break;
                    }
                }
            }

            // Close all open replicas before shutting down.
            for (vid, repl) in replicas.drain() {
                if let Err(e) = repl.close() {
                    tracing::warn!("Error closing replica for verse {vid}: {e}");
                }
            }

            // Graceful endpoint shutdown
            if let Some(ep) = endpoint {
                ep.shutdown().await;
            }
            evt_tx.send(SyncEvent::Stopped).ok();
        });
    })
}

/// Handle a [`SyncCommand::FetchBlob`].
///
/// Phase D stub: checks the local blob store first and emits
/// [`SyncEvent::BlobReady`] if found.  Otherwise logs a placeholder message
/// — actual peer discovery and transfer will be added in Phase F.
fn handle_fetch_blob(
    blob_store: &BlobStoreHandle,
    _endpoint: Option<&SyncEndpoint>,
    hash: &fe_runtime::blob_store::BlobHash,
    verse_id: &str,
    evt_tx: &SyncEventSender,
) {
    let hex = hash_to_hex(hash);

    // Fast path: already have it locally.
    if blob_store.has_blob(hash) {
        tracing::debug!(hash = %hex, "Blob already present locally");
        evt_tx.send(SyncEvent::BlobReady { hash: *hash }).ok();
        return;
    }

    // Phase F: VersePeers lookup would happen here. The VersePeers resource
    // lives in the Bevy world; a future phase will bridge peer sets into the
    // sync thread so we can attempt fetches from known peers.
    // For now, compute the gossip topic to log context.
    let topic = verse_peers::verse_gossip_topic(verse_id);
    tracing::info!(
        hash = %hex,
        verse_id = %verse_id,
        gossip_topic = %hex::encode(topic),
        "Would fetch blob from peers (stub — peer discovery not yet implemented)"
    );
}

/// Handle [`SyncCommand::OpenVerseReplica`].
///
/// Creates an `IrohDocsReplicator` (now with real iroh-docs when available) and
/// inserts it into the replica map. If a replica is already open for this verse,
/// it is closed first.
fn handle_open_verse_replica(
    replicas: &mut HashMap<String, Box<dyn VerseReplicator>>,
    engine_holder: Arc<IrohDocsEngineHolder>,
    verse_id: &str,
    namespace_id: &str,
    namespace_secret: Option<String>,
    local_did: &str,
) {
    // Close existing replica if any.
    if let Some(old) = replicas.remove(verse_id) {
        tracing::debug!(verse_id, "Closing existing replica before re-open");
        old.close().ok();
    }

    let secret = namespace_secret.unwrap_or_default();
    let replicator = IrohDocsReplicator::new(
        namespace_id.to_string(),
        secret,
        local_did.to_string(),
        engine_holder.clone(),
    );

    // Open the document for this namespace
    replicator.open_document().ok();

    replicas.insert(verse_id.to_string(), Box::new(replicator));

    // Phase F: compute gossip topic for this verse
    let topic_hash = verse_peers::verse_gossip_topic(verse_id);
    tracing::info!(
        verse_id,
        namespace_id,
        gossip_topic = %hex::encode(topic_hash),
        "Opened verse replica — iroh-docs {}",
        if engine_holder.is_available() { "online" } else { "mock fallback" }
    );
}

/// Handle [`SyncCommand::CloseVerseReplica`].
fn handle_close_verse_replica(
    replicas: &mut HashMap<String, Box<dyn VerseReplicator>>,
    verse_id: &str,
) {
    if let Some(repl) = replicas.remove(verse_id) {
        if let Err(e) = repl.close() {
            tracing::warn!(verse_id, "Error closing replica: {e}");
        }
        tracing::info!(verse_id, "Closed verse replica");
    } else {
        tracing::debug!(verse_id, "CloseVerseReplica: no open replica");
    }
}

/// Handle [`SyncCommand::WriteRowEntry`] (blob read off-loaded via spawn_blocking).
#[allow(clippy::too_many_arguments)]
async fn handle_write_row_entry(
    replicas: &HashMap<String, Box<dyn VerseReplicator>>,
    blob_store: &BlobStoreHandle,
    write_policy: &crate::write_policy::PolicyHandle,
    author_did: &str,
    verse_id: &str,
    table: &str,
    record_id: &str,
    content_hash: &fe_runtime::blob_store::BlobHash,
) {
    // §D1 gate: no row applies to a replica without a Policy::evaluate decision.
    // Role is not plumbed to this thread yet (None) — enforcement flips with
    // the strict PolicyHandle once it is. See AGENTS.md §write-policy.
    if !write_policy.allow_write(author_did, None, verse_id) {
        return;
    }

    let Some(repl) = replicas.get(verse_id) else {
        tracing::warn!(
            verse_id,
            table,
            record_id,
            "WriteRowEntry: no open replica for verse"
        );
        return;
    };

    // Read the blob data to pass to the replicator.
    let hex = hash_to_hex(content_hash);
    let blob_path = match blob_store.get_blob_path(content_hash) {
        Some(p) => p,
        None => {
            tracing::warn!(
                verse_id,
                hash = %hex,
                "WriteRowEntry: blob not found in store"
            );
            return;
        }
    };

    // Keep the current-thread runtime responsive during slow disk reads
    // (see AGENTS.md §sync-thread-blocking-io).
    let data = match tokio::task::spawn_blocking(move || std::fs::read(&blob_path)).await {
        Ok(Ok(d)) => d,
        Ok(Err(e)) => {
            tracing::error!(
                verse_id,
                hash = %hex,
                "WriteRowEntry: could not read blob: {e}"
            );
            return;
        }
        Err(e) => {
            tracing::error!(
                verse_id,
                hash = %hex,
                "WriteRowEntry: blob read task failed: {e}"
            );
            return;
        }
    };

    if let Err(e) = repl.write_row(table, record_id, &data) {
        tracing::error!(
            verse_id,
            table,
            record_id,
            "WriteRowEntry: replicator write failed: {e}"
        );
    }
}

/// Derive a namespace ID from a petal ID.
///
/// Uses a deterministic hash to derive the namespace ID so that
/// the same petal on different nodes maps to the same iroh-docs namespace.
fn derive_namespace_id(petal_id: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    petal_id.hash(&mut hasher);
    format!("petal-ns-{:x}", hasher.finish())
}

/// Handle [`SyncCommand::SubscribePetal`].
///
/// Creates a new `IrohPetalReplicator` for the petal and inserts it into the
/// petal replica map. Each petal gets its own iroh-docs namespace derived
/// from the petal_id.
fn handle_subscribe_petal(
    petal_replicas: &mut HashMap<String, Box<dyn PetalReplicator>>,
    engine_holder: Arc<IrohDocsEngineHolder>,
    petal_id: &str,
    local_did: &str,
) {
    // Remove existing replicator if any (cleanup old subscription)
    if let Some(old) = petal_replicas.remove(petal_id) {
        tracing::debug!(
            petal_id,
            "Closing existing petal replicator before re-subscribe"
        );
        old.close().ok();
    }

    // Derive namespace ID from petal_id
    let namespace_id = derive_namespace_id(petal_id);

    // Create the petal replicator
    let replicator =
        IrohPetalReplicator::new(petal_id.to_string(), namespace_id, local_did.to_string());

    petal_replicas.insert(petal_id.to_string(), Box::new(replicator));

    tracing::info!(
        petal_id,
        "Subscribed to petal — iroh-docs {}",
        if engine_holder.is_available() {
            "online"
        } else {
            "mock fallback"
        }
    );
}

/// Handle [`SyncCommand::UnsubscribePetal`].
///
/// Closes and removes the petal replicator from the map.
fn handle_unsubscribe_petal(
    petal_replicas: &mut HashMap<String, Box<dyn PetalReplicator>>,
    petal_id: &str,
) {
    if let Some(repl) = petal_replicas.remove(petal_id) {
        if let Err(e) = repl.close() {
            tracing::warn!(petal_id, "Error closing petal replicator: {e}");
        }
        tracing::info!(petal_id, "Unsubscribed from petal");
    } else {
        tracing::debug!(petal_id, "UnsubscribePetal: no open replicator");
    }
}

/// Derive a gossip topic from a verse ID.
///
/// The topic is used for real-time transform sync among peers
/// viewing the same verse.
fn derive_gossip_topic(verse_id: &str) -> String {
    format!("verse:{}", verse_id)
}

/// Handle [`SyncCommand::UpdateNodeTransform`].
///
/// Broadcasts the transform update to peers via iroh-gossip.
/// If gossip is not available, logs a warning and skips broadcast.
async fn handle_update_node_transform(
    gossip_host: &Option<Gossip>,
    gossip_topics: &mut HashMap<String, GossipTopic>,
    verse_id: &str,
    node_id: &str,
    position: [f32; 3],
    rotation: [f32; 3],
    scale: [f32; 3],
    local_did: &str,
) {
    let Some(ref gossip) = gossip_host else {
        tracing::debug!(verse_id, node_id, "No gossip, skipping transform broadcast");
        return;
    };

    // Get or create the live subscription handle for this verse.
    let topic_key = derive_gossip_topic(verse_id);
    if !gossip_topics.contains_key(&topic_key) {
        match gossip.subscribe(gossip_topic_id(&topic_key), Vec::new()) {
            Ok(handle) => {
                tracing::debug!(verse_id, "Subscribed to gossip topic");
                gossip_topics.insert(topic_key.clone(), handle);
            }
            Err(e) => {
                tracing::warn!(verse_id, "Failed to subscribe to gossip topic: {e}");
                return;
            }
        }
    }
    let topic = gossip_topics.get(&topic_key).expect("just inserted");

    // Create the transform update message
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);

    let update = TransformUpdate {
        verse_id: verse_id.to_string(),
        node_id: node_id.to_string(),
        position,
        rotation,
        scale,
        author_id: local_did.to_string(),
        timestamp,
    };

    // Serialize and broadcast
    let payload = match serde_json::to_vec(&update) {
        Ok(p) => p,
        Err(e) => {
            tracing::error!(node_id, "Failed to serialize TransformUpdate: {e}");
            return;
        }
    };

    // Broadcast to topic (best-effort; queued until a neighbor is available).
    if let Err(e) = topic.broadcast(payload.into()).await {
        tracing::warn!(node_id, "Failed to broadcast transform: {e}");
    } else {
        tracing::debug!(node_id, verse_id, "Broadcasted transform update");
    }
}

/// Subscribe to a verse gossip topic.
///
/// This is called when opening a verse replica to ensure we receive
/// transform updates from other peers viewing the same verse.
fn subscribe_to_verse_gossip_topic(
    gossip_host: &Option<Gossip>,
    gossip_topics: &mut HashMap<String, GossipTopic>,
    verse_id: &str,
) {
    let Some(ref gossip) = gossip_host else {
        tracing::debug!(verse_id, "No gossip, skipping topic subscription");
        return;
    };

    let topic_key = derive_gossip_topic(verse_id);

    // Already subscribed?
    if gossip_topics.contains_key(&topic_key) {
        tracing::debug!(verse_id, "Already subscribed to gossip topic");
        return;
    }

    match gossip.subscribe(gossip_topic_id(&topic_key), Vec::new()) {
        Ok(handle) => {
            tracing::debug!(verse_id, "Subscribed to verse gossip topic");
            gossip_topics.insert(topic_key, handle);
        }
        Err(e) => {
            tracing::warn!(verse_id, "Failed to subscribe to gossip topic: {e}");
        }
    }
}

/// Unsubscribe from a verse gossip topic.
///
/// This is called when closing a verse replica. Dropping the `GossipTopic`
/// handle leaves the topic in iroh-gossip 0.35.
fn unsubscribe_from_verse_gossip_topic(
    _gossip_host: &Option<Gossip>,
    gossip_topics: &mut HashMap<String, GossipTopic>,
    verse_id: &str,
) {
    let topic_key = derive_gossip_topic(verse_id);

    if gossip_topics.remove(&topic_key).is_some() {
        tracing::debug!(verse_id, "Unsubscribed from verse gossip topic");
    }
}

// ============================================================================
// Phase 5: Tileset P2P
// ============================================================================

/// Tileset advertisement message for P2P discovery.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TilesetAdvertisement {
    pub tileset_id: String,
    pub chunk_count: u32,
    pub size_bytes: u64,
}

/// Track in-flight tileset downloads.
struct TilesetDownloadTracker {
    /// Active downloads: tileset_id -> (peer_id, chunk_seq)
    active: HashMap<String, (String, u32)>,
}

impl TilesetDownloadTracker {
    fn new() -> Self {
        Self {
            active: HashMap::new(),
        }
    }

    fn start(&mut self, tileset_id: String, peer_id: String, chunk_seq: u32) {
        self.active.insert(tileset_id, (peer_id, chunk_seq));
    }

    fn cancel(&mut self, tileset_id: &str) -> Option<(String, u32)> {
        self.active.remove(tileset_id).map(|(peer_id, chunk_seq)| (peer_id, chunk_seq))
    }

    fn get(&self, tileset_id: &str) -> Option<&(String, u32)> {
        self.active.get(tileset_id)
    }
}

/// Handle [`SyncCommand::AdvertiseTilesets`].
///
/// Broadcasts tileset advertisements to connected peers via gossip.
async fn handle_advertise_tilesets(
    gossip_host: &Option<Gossip>,
    gossip_topics: &HashMap<String, GossipTopic>,
    advertisements_json: &str,
    verse_id: &str,
) {
    if gossip_host.is_none() {
        tracing::debug!(len = advertisements_json.len(), "No gossip, skipping tileset advertise");
        return;
    };

    // Empty payload = nothing to advertise (fe-ui sends "" as a refresh nudge).
    if advertisements_json.trim().is_empty() {
        tracing::debug!("AdvertiseTilesets with empty payload — skipping");
        return;
    }

    // Parse advertisements
    let ads: Vec<TilesetAdvertisement> = match serde_json::from_str(advertisements_json) {
        Ok(a) => a,
        Err(e) => {
            tracing::error!("Failed to parse tileset advertisements: {e}");
            return;
        }
    };

    // Get the verse topic handle
    let topic_key = derive_gossip_topic(verse_id);
    let Some(topic) = gossip_topics.get(&topic_key) else {
        tracing::warn!(verse_id, "No gossip topic for verse, cannot advertise tilesets");
        return;
    };

    // Broadcast each advertisement
    for ad in ads {
        let payload = match serde_json::to_vec(&ad) {
            Ok(p) => p,
            Err(e) => {
                tracing::error!(tileset_id = %ad.tileset_id, "Failed to serialize advertisement: {e}");
                continue;
            }
        };

        if let Err(e) = topic.broadcast(payload.into()).await {
            tracing::warn!(tileset_id = %ad.tileset_id, "Failed to broadcast tileset advertisement: {e}");
        } else {
            tracing::debug!(tileset_id = %ad.tileset_id, "Broadcasted tileset advertisement");
        }
    }
}

/// Handle [`SyncCommand::RequestTilesetMeta`].
///
/// Looks up tileset metadata locally and sends to requesting peer.
/// This is a stub - full implementation would track local tilesets.
fn handle_request_tileset_meta(
    peer_id: &str,
    tileset_id: &str,
    evt_tx: &SyncEventSender,
) {
    tracing::debug!(peer_id = %peer_id, tileset_id = %tileset_id, "RequestTilesetMeta (stub)");

    // Stub: emit a placeholder response
    // In full implementation, we'd look up actual metadata
    let meta_json = r#"{"error": "tileset not found locally"}"#;
    evt_tx.send(SyncEvent::TilesetMetaReceived {
        peer_id: peer_id.to_string(),
        tileset_id: tileset_id.to_string(),
        meta_json: meta_json.to_string(),
        total_chunks: 0,
        approx_size_bytes: 0,
    }).ok();
}

/// Handle [`SyncCommand::RequestChunk`].
///
/// Requests a chunk from a peer via iroh-blobs.
/// This is a stub - full implementation would use actual blob transfer.
fn handle_request_chunk(
    peer_id: &str,
    tileset_id: &str,
    chunk_seq: u32,
    evt_tx: &SyncEventSender,
) {
    tracing::debug!(peer_id = %peer_id, tileset_id = %tileset_id, chunk_seq, "RequestChunk (stub)");

    // Stub: emit failure since we can't actually transfer
    evt_tx.send(SyncEvent::ChunkFailed {
        tileset_id: tileset_id.to_string(),
        chunk_seq,
        reason: "chunk transfer not implemented in stub".to_string(),
    }).ok();
}

/// Handle [`SyncCommand::CancelTilesetDownload`].
///
/// Cancels an in-progress tileset download.
fn handle_cancel_tileset_download(
    download_tracker: &mut TilesetDownloadTracker,
    tileset_id: &str,
) {
    if let Some((peer_id, chunk_seq)) = download_tracker.cancel(tileset_id) {
        tracing::info!(tileset_id, peer_id = %peer_id, chunk_seq, "Cancelled tileset download");
    } else {
        tracing::debug!(tileset_id, "CancelTilesetDownload: no active download");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fe_runtime::blob_store::mock::MockBlobStore;
    use std::sync::Arc;

    #[test]
    fn fetch_blob_local_hit_emits_ready() {
        let store: BlobStoreHandle = Arc::new(MockBlobStore::new());
        let hash = store.add_blob(b"test data").unwrap();
        let (evt_tx, evt_rx) = crossbeam::channel::bounded(8);

        handle_fetch_blob(&store, None, &hash, "verse-1", &evt_tx);

        let evt = evt_rx.try_recv().expect("should have BlobReady");
        assert!(matches!(evt, SyncEvent::BlobReady { hash: h } if h == hash));
    }

    #[test]
    fn fetch_blob_miss_does_not_emit_ready() {
        let store: BlobStoreHandle = Arc::new(MockBlobStore::new());
        let missing = [0xABu8; 32];
        let (evt_tx, evt_rx) = crossbeam::channel::bounded(8);

        handle_fetch_blob(&store, None, &missing, "verse-2", &evt_tx);

        assert!(
            evt_rx.try_recv().is_err(),
            "no event for a miss in Phase D stub"
        );
    }

    #[test]
    fn shutdown_command_terminates_thread() {
        let store: BlobStoreHandle = Arc::new(MockBlobStore::new());
        let secret = iroh::SecretKey::from_bytes(&[99u8; 32]);
        let (cmd_tx, cmd_rx) = crossbeam::channel::bounded(8);
        let (evt_tx, evt_rx) = crossbeam::channel::bounded(8);

        let handle = spawn_sync_thread(secret, store, cmd_rx, evt_tx, "test-node-did".to_string());
        // Wait for Started event (online or offline)
        let started = evt_rx.recv_timeout(std::time::Duration::from_secs(15));
        assert!(
            matches!(started, Ok(SyncEvent::Started { .. })),
            "expected Started event, got {started:?}"
        );

        cmd_tx.send(SyncCommand::Shutdown).unwrap();
        handle.join().expect("sync thread panicked");

        let stopped = evt_rx.recv_timeout(std::time::Duration::from_secs(2));
        assert!(matches!(stopped, Ok(SyncEvent::Stopped)));
    }

    // -------------------------------------------------------------------------
    // Phase 2: Petal-Level Replication Tests (TDD)
    // -------------------------------------------------------------------------

    #[test]
    fn derive_namespace_id_is_deterministic() {
        let id1 = derive_namespace_id("petal-123");
        let id2 = derive_namespace_id("petal-123");
        let id3 = derive_namespace_id("petal-456");

        assert_eq!(id1, id2, "same petal_id should produce same namespace");
        assert_ne!(
            id1, id3,
            "different petal_ids should produce different namespaces"
        );
    }

    #[test]
    fn derive_namespace_id_format() {
        let namespace = derive_namespace_id("my-petal");

        assert!(
            namespace.starts_with("petal-ns-"),
            "namespace should start with 'petal-ns-', got: {}",
            namespace
        );
    }

    #[test]
    fn handle_subscribe_petal_creates_replicator() {
        let mut petal_replicas: HashMap<String, Box<dyn PetalReplicator>> = HashMap::new();
        let engine_holder = Arc::new(IrohDocsEngineHolder::new());
        let local_did = "did:test:local-author";

        handle_subscribe_petal(&mut petal_replicas, engine_holder, "petal-alpha", local_did);

        assert!(
            petal_replicas.contains_key("petal-alpha"),
            "petal-alpha should be in replica map"
        );
    }

    #[test]
    fn handle_subscribe_petal_idempotent_resubscribe() {
        let mut petal_replicas: HashMap<String, Box<dyn PetalReplicator>> = HashMap::new();
        let engine_holder = Arc::new(IrohDocsEngineHolder::new());
        let local_did = "did:test:local-author";

        // First subscription
        handle_subscribe_petal(
            &mut petal_replicas,
            engine_holder.clone(),
            "petal-beta",
            local_did,
        );
        let first_count = petal_replicas.len();

        // Re-subscription should replace old replicator (idempotent)
        handle_subscribe_petal(
            &mut petal_replicas,
            engine_holder.clone(),
            "petal-beta",
            local_did,
        );
        let second_count = petal_replicas.len();

        assert_eq!(
            first_count, second_count,
            "re-subscribe should not add duplicate"
        );
        assert!(petal_replicas.contains_key("petal-beta"));
    }

    #[test]
    fn handle_unsubscribe_petal_removes_replicator() {
        let mut petal_replicas: HashMap<String, Box<dyn PetalReplicator>> = HashMap::new();
        let engine_holder = Arc::new(IrohDocsEngineHolder::new());
        let local_did = "did:test:local-author";

        // Subscribe first
        handle_subscribe_petal(&mut petal_replicas, engine_holder, "petal-gamma", local_did);
        assert!(petal_replicas.contains_key("petal-gamma"));

        // Unsubscribe
        handle_unsubscribe_petal(&mut petal_replicas, "petal-gamma");

        assert!(
            !petal_replicas.contains_key("petal-gamma"),
            "petal-gamma should be removed after unsubscribe"
        );
    }

    #[test]
    fn handle_unsubscribe_petal_nonexistent_is_noop() {
        let mut petal_replicas: HashMap<String, Box<dyn PetalReplicator>> = HashMap::new();

        // Unsubscribe non-existent should not panic
        handle_unsubscribe_petal(&mut petal_replicas, "petal-missing");

        assert!(petal_replicas.is_empty(), "map should remain empty");
    }

    #[test]
    fn petal_replicator_write_and_subscribe() {
        use crate::replicator::IrohPetalReplicator;

        // Create a petal replicator
        let repl = IrohPetalReplicator::new(
            "test-petal".to_string(),
            "petal-ns-test".to_string(),
            "did:test:author".to_string(),
        );

        // Write a row
        repl.write_row("nodes", "node-001", b"{\"name\": \"test\"}")
            .expect("write_row should succeed");

        // Subscribe to changes
        let mut rx = repl.subscribe().expect("subscribe should succeed");

        // Write another row - should trigger notification
        repl.write_row("nodes", "node-002", b"{\"name\": \"test2\"}")
            .expect("write_row should succeed");

        // Check we received a change notification (non-blocking)
        let change = rx.try_recv();
        assert!(
            change.is_ok(),
            "should receive change notification after write"
        );
    }

    // -------------------------------------------------------------------------
    // Phase 3: Real-Time Transform Sync Tests (TDD)
    // -------------------------------------------------------------------------

    #[test]
    fn derive_gossip_topic_is_deterministic() {
        let topic1 = derive_gossip_topic("verse-abc");
        let topic2 = derive_gossip_topic("verse-abc");
        let topic3 = derive_gossip_topic("verse-xyz");

        assert_eq!(topic1, topic2, "same verse_id should produce same topic");
        assert_ne!(topic1, topic3, "different verse_ids should produce different topics");
    }

    #[test]
    fn derive_gossip_topic_format() {
        let topic = derive_gossip_topic("my-verse");

        assert!(
            topic.starts_with("verse:"),
            "topic should start with 'verse:', got: {}",
            topic
        );
    }

    #[test]
    fn handle_update_node_transform_no_host_is_noop() {
        let gossip_host: Option<Gossip> = None;
        let mut gossip_topics: HashMap<String, GossipTopic> = HashMap::new();
        let local_did = "did:test:author";

        // Should not panic, just skip
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(handle_update_node_transform(
            &gossip_host,
            &mut gossip_topics,
            "verse-1",
            "node-123",
            [1.0, 2.0, 3.0],
            [0.0, 0.0, 0.0],
            [1.0, 1.0, 1.0],
            local_did,
        ));

        // No topics should be created without a host
        assert!(gossip_topics.is_empty(), "no topics without gossip host");
    }

    // -------------------------------------------------------------------------
    // Phase 4: Gossip Topic Subscription Tests (TDD)
    // -------------------------------------------------------------------------

    #[test]
    fn subscribe_to_verse_gossip_topic_no_host_is_noop() {
        let gossip_host: Option<Gossip> = None;
        let mut gossip_topics: HashMap<String, GossipTopic> = HashMap::new();

        // Should not panic, just skip
        subscribe_to_verse_gossip_topic(&gossip_host, &mut gossip_topics, "verse-test");

        // No topics should be created without a host
        assert!(gossip_topics.is_empty(), "no topics without gossip host");
    }

    #[test]
    fn unsubscribe_from_verse_gossip_topic_no_host_is_noop() {
        let gossip_host: Option<Gossip> = None;
        let mut gossip_topics: HashMap<String, GossipTopic> = HashMap::new();

        // Pre-populate (simulating prior subscription)
        // Note: Can't actually insert valid TopicId without real host,
        // but we test the cleanup path

        // Should not panic
        unsubscribe_from_verse_gossip_topic(&gossip_host, &mut gossip_topics, "verse-test");

        assert!(gossip_topics.is_empty(), "map should be empty after unsubscribe");
    }

    // -------------------------------------------------------------------------
    // Phase 5: Tileset P2P Tests (TDD)
    // -------------------------------------------------------------------------

    #[test]
    fn tileset_download_tracker_new() {
        let tracker = TilesetDownloadTracker::new();
        assert!(tracker.active.is_empty(), "new tracker should be empty");
    }

    #[test]
    fn tileset_download_tracker_start_and_cancel() {
        let mut tracker = TilesetDownloadTracker::new();

        // Start a download
        tracker.start("ts-001".to_string(), "peer-a".to_string(), 0);
        assert!(tracker.get("ts-001").is_some(), "download should be tracked");

        // Cancel it
        let result = tracker.cancel("ts-001");
        assert!(result.is_some(), "cancel should return the download info");
        assert!(tracker.get("ts-001").is_none(), "download should be removed after cancel");
    }

    #[test]
    fn tileset_download_tracker_cancel_nonexistent() {
        let mut tracker = TilesetDownloadTracker::new();

        // Cancel something that doesn't exist
        let result = tracker.cancel("ts-nonexistent");
        assert!(result.is_none(), "canceling non-existent should return None");
    }

    #[test]
    fn handle_advertise_tilesets_no_host_is_noop() {
        let gossip_host: Option<Gossip> = None;
        let gossip_topics: HashMap<String, GossipTopic> = HashMap::new();

        let ads_json = r#"[{"tileset_id": "ts-001", "chunk_count": 10, "size_bytes": 1000}]"#;

        // Should not panic, just skip
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(handle_advertise_tilesets(&gossip_host, &gossip_topics, ads_json, "verse-1"));
    }
}
