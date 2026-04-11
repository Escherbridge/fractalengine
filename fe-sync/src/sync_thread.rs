//! Sync thread — owns the iroh endpoint and processes [`SyncCommand`]s
//! (P2P Mycelium Phase D).
//!
//! Modelled after `fe-database::spawn_db_thread`: a dedicated OS thread with
//! its own single-threaded Tokio runtime.  If the iroh endpoint fails to
//! bind the thread enters **offline mode** — it stays alive, responds to
//! commands, but all network-dependent operations are no-ops.

use std::collections::HashMap;

use fe_runtime::blob_store::{hash_to_hex, BlobStoreHandle};

use crate::endpoint::SyncEndpoint;
use crate::messages::{SyncCommand, SyncCommandReceiver, SyncEvent, SyncEventSender};
use crate::replicator::{IrohDocsReplicator, VerseReplicator};
use crate::verse_peers;

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
pub fn spawn_sync_thread(
    secret_key: iroh::SecretKey,
    blob_store: BlobStoreHandle,
    cmd_rx: SyncCommandReceiver,
    evt_tx: SyncEventSender,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("Failed to build sync Tokio runtime");

        rt.block_on(async move {
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

            // Phase E: per-verse replica map.
            let mut replicas: HashMap<String, Box<dyn VerseReplicator>> = HashMap::new();

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
                            &verse_id,
                            &namespace_id,
                            namespace_secret,
                        );
                    }
                    Ok(SyncCommand::CloseVerseReplica { verse_id }) => {
                        handle_close_verse_replica(&mut replicas, &verse_id);
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
                            &verse_id,
                            &table,
                            &record_id,
                            &content_hash,
                        );
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
/// Creates an `IrohDocsReplicator` (stub) and inserts it into the replica map.
/// If a replica is already open for this verse, it is closed first.
fn handle_open_verse_replica(
    replicas: &mut HashMap<String, Box<dyn VerseReplicator>>,
    verse_id: &str,
    namespace_id: &str,
    namespace_secret: Option<String>,
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
        "local-node".to_string(), // TODO(Phase F): use actual node DID
    );
    replicas.insert(verse_id.to_string(), Box::new(replicator));

    // Phase F: compute gossip topic for this verse (stub — actual join deferred).
    let topic_hash = verse_peers::verse_gossip_topic(verse_id);
    tracing::info!(
        verse_id,
        namespace_id,
        gossip_topic = %hex::encode(topic_hash),
        "Opened verse replica (stub) — gossip topic computed"
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

/// Handle [`SyncCommand::WriteRowEntry`].
///
/// Reads the row data from the blob store and writes it to the verse's replica.
fn handle_write_row_entry(
    replicas: &HashMap<String, Box<dyn VerseReplicator>>,
    blob_store: &BlobStoreHandle,
    verse_id: &str,
    table: &str,
    record_id: &str,
    content_hash: &fe_runtime::blob_store::BlobHash,
) {
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

    let data = match std::fs::read(&blob_path) {
        Ok(d) => d,
        Err(e) => {
            tracing::error!(
                verse_id,
                hash = %hex,
                "WriteRowEntry: could not read blob: {e}"
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

        let handle = spawn_sync_thread(secret, store, cmd_rx, evt_tx);
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
}
