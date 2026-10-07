//! Sync thread — owns the iroh endpoint and processes [`SyncCommand`]s
//! (P2P Mycelium Phase D).
//!
//! Modelled after `fe-database::spawn_db_thread`: a dedicated OS thread with
//! its own single-threaded Tokio runtime.  If the iroh endpoint fails to
//! bind the thread enters **offline mode** — it stays alive, responds to
//! commands, but all network-dependent operations are no-ops.
//!
//! The command loop is a `tokio::select!` over the command channel and the
//! per-replica inbound event pumps (A2/A4): crossbeam's receiver has no
//! async API, so a bridge thread forwards commands into a tokio channel
//! before the select.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use fe_runtime::blob_store::{hash_to_hex, BlobStoreHandle};
use fe_runtime::messages::DbCommand;

use iroh_gossip::net::{Gossip, GossipTopic};
use iroh_gossip::proto::TopicId;

use crate::docs_engine::{p2p_data_dir, DocsStack};
use crate::endpoint::SyncEndpoint;
use crate::messages::{SyncCommand, SyncCommandReceiver, SyncEvent, SyncEventSender};
use crate::relay_config::{RelayConfig, RelayHealth};
use crate::replicator::{
    IrohDocsEngineHolder, IrohDocsReplicator, IrohPetalReplicator, PetalReplicator, RowChange,
    VerseReplicator,
};
use crate::verse_peers;

/// Env var carrying bootstrap peers for every opened replica: **semicolon**
/// `-separated` iroh `NodeAddr` JSON entries (or bare `NodeId` hex), parsed
/// with iroh's own serde/FromStr types. Semicolons because `NodeAddr` JSON
/// itself contains commas — a comma-separated list would be ambiguous.
/// Invalid entries are skipped loudly, never fatal (see AGENTS.md
/// §relay-health for the relay config this rides alongside).
pub const BOOTSTRAP_ENV_VAR: &str = "FE_SYNC_BOOTSTRAP";

/// Inbound rows dropped because the DB-thread command channel was full
/// (§replication-backpressure — drop-and-count, never block the sync thread).
static INBOUND_APPLY_DROPS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Total inbound rows dropped due to a full DB command channel.
pub fn inbound_apply_drop_count() -> u64 {
    INBOUND_APPLY_DROPS.load(std::sync::atomic::Ordering::Relaxed)
}

/// Send a [`SyncEvent`] back to the main thread, warning on failure —
/// never a bare `.ok()` (§warn-on-send-failure).
fn send_sync_event(evt_tx: &SyncEventSender, event: SyncEvent) {
    if let Err(e) = evt_tx.send(event) {
        tracing::warn!("sync event send failed: {e}");
    }
}

/// Parse one bootstrap peer entry with iroh's own types: a JSON-serialized
/// `iroh::NodeAddr` (what `SyncEvent::Started.node_addr` emits), or a bare
/// hex `NodeId` (which carries no address and therefore cannot be dialed —
/// that is reported loudly and skipped).
fn parse_bootstrap_peer(entry: &str) -> Option<iroh::NodeAddr> {
    let entry = entry.trim();
    if entry.is_empty() {
        return None;
    }
    match serde_json::from_str::<iroh::NodeAddr>(entry) {
        Ok(addr) => Some(addr),
        Err(json_err) => {
            use std::str::FromStr;
            match iroh::NodeId::from_str(entry) {
                Ok(node_id) => {
                    tracing::warn!(
                        node_id = %node_id.fmt_short(),
                        "bootstrap entry carries no address (expected NodeAddr JSON) — skipped"
                    );
                    None
                }
                Err(_) => {
                    tracing::warn!(
                        entry = %entry.truncate_str(80),
                        "invalid FE_SYNC_BOOTSTRAP entry (NodeAddr JSON or NodeId hex expected) — skipped: {json_err}"
                    );
                    None
                }
            }
        }
    }
}

/// Minimal string truncation helper for loud logs (no new deps).
trait TruncateStr {
    fn truncate_str(&self, max: usize) -> String;
}

impl TruncateStr for str {
    fn truncate_str(&self, max: usize) -> String {
        if self.len() <= max {
            self.to_string()
        } else {
            let mut cut = max;
            while !self.is_char_boundary(cut) {
                cut -= 1;
            }
            format!("{}…", &self[..cut])
        }
    }
}

/// Resolve the configured bootstrap peers from [`BOOTSTRAP_ENV_VAR`].
///
/// Entries are **semicolon-separated** (a `NodeAddr` JSON contains commas,
/// so the list separator must not). Invalid entries fail loudly at startup
/// (one warn each) and are skipped — a bad config must never take the sync
/// thread down.
fn bootstrap_peers_from_env() -> Vec<iroh::NodeAddr> {
    std::env::var(BOOTSTRAP_ENV_VAR)
        .ok()
        .map(|raw| {
            raw.split(';')
                .filter_map(parse_bootstrap_peer)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

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
/// * `db_cmd_tx` — the DB thread's command sender for the inbound apply path
///   (`DbCommand::ApplyReplicatedRow`, A4). `None` disables inbound applies
///   (rows are logged and dropped) — for tests without a DB thread.
/// * `p2p_dir` — explicit per-thread P2P data dir (one `DocsStack` per data
///   dir per process — the redb store takes an exclusive file lock, so two
///   sync threads in one process need two dirs; this parameter is how
///   multi-peer tests give each peer its own). `None` resolves
///   [`crate::docs_engine::P2P_DIR_ENV_VAR`] (env, default `data/p2p`).
pub fn spawn_sync_thread(
    secret_key: iroh::SecretKey,
    blob_store: BlobStoreHandle,
    cmd_rx: SyncCommandReceiver,
    evt_tx: SyncEventSender,
    local_did: String,
    db_cmd_tx: Option<crossbeam::channel::Sender<DbCommand>>,
    p2p_dir: Option<PathBuf>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("Failed to build sync Tokio runtime");

        rt.block_on(async move {
            // Relay-health hardening (track p2p_asset_streaming_20260718 FR-1 /
            // decision D-77): resolve the relay config up front so both the
            // bind attempt and the health signal below use the same value.
            let relay_config = RelayConfig::from_env();
            tracing::info!(relay_config = ?relay_config, "Resolved relay config for sync thread");

            // Bootstrap peers (A9 seam): parsed once with iroh's own parsers,
            // invalid entries warned loudly and skipped.
            let bootstrap_peers = bootstrap_peers_from_env();
            if !bootstrap_peers.is_empty() {
                tracing::info!(
                    count = bootstrap_peers.len(),
                    "Resolved bootstrap peers from {BOOTSTRAP_ENV_VAR}"
                );
            }

            // Phase F.1: Create the iroh endpoint first
            let mut relay_health: RelayHealth;
            let endpoint = match SyncEndpoint::new(secret_key, &relay_config).await {
                Ok(ep) => {
                    tracing::info!(
                        node_id = %ep.node_id(),
                        "Sync thread started (online)"
                    );
                    // Our own dialable address, for peers/relays joining us
                    // (serialized NodeAddr JSON — `parse_bootstrap_peer`
                    // reads this exact form).
                    let node_addr = match ep.inner().node_addr().await {
                        Ok(addr) => Some(addr),
                        Err(e) => {
                            tracing::warn!("Could not resolve our node addr: {e}");
                            None
                        }
                    };
                    let addr_json = node_addr
                        .as_ref()
                        .and_then(|a| serde_json::to_string(a).ok());
                    send_sync_event(
                        &evt_tx,
                        SyncEvent::Started {
                            online: true,
                            node_addr: addr_json,
                        },
                    );
                    relay_health = RelayHealth::on_bind_success(&relay_config);
                    send_sync_event(
                        &evt_tx,
                        SyncEvent::RelayHealthChanged {
                            health: relay_health,
                        },
                    );
                    Some(ep)
                }
                Err(e) => {
                    // LOUD-FAIL: this is a hard connectivity failure, not a
                    // debug-level detail — see AGENTS.md §relay-health.
                    tracing::error!("Sync thread could not bind iroh endpoint — relay unreachable: {e}");
                    tracing::warn!("Running in offline mode — network fetch disabled");
                    send_sync_event(
                        &evt_tx,
                        SyncEvent::Started {
                            online: false,
                            node_addr: None,
                        },
                    );
                    relay_health = RelayHealth::on_bind_failure();
                    send_sync_event(
                        &evt_tx,
                        SyncEvent::RelayHealthChanged {
                            health: relay_health,
                        },
                    );
                    None
                }
            };

            // Real iroh-docs 0.35 stack (A1): Blobs (fs) + Gossip + Docs
            // (persistent redb) + Router accepting all three ALPNs, with
            // persistent stores under the data dir. Absent in offline mode
            // (endpoint bind failure or stack spawn failure) — the holder
            // then reports unavailable and replicators degrade to the
            // in-memory mock. See AGENTS.md §iroh-0.35 and docs_engine.rs.
            let p2p_dir = p2p_dir.unwrap_or_else(p2p_data_dir);
            let docs_stack: Option<Arc<DocsStack>> = match endpoint.as_ref() {
                Some(ep) => match DocsStack::spawn(ep.inner().clone(), &p2p_dir).await {
                    Ok(stack) => Some(Arc::new(stack)),
                    Err(e) => {
                        // LOUD-FAIL: the endpoint is up but replication is
                        // not. Degrade to offline/mock and record the
                        // degradation in relay health (the stack includes
                        // gossip, which rides the same endpoint/relay
                        // connection — see AGENTS.md §relay-health).
                        tracing::error!(
                            p2p_dir = %p2p_dir.display(),
                            "Failed to spawn P2P docs stack — degrading to offline/mock replication: {e}"
                        );
                        relay_health = relay_health.on_error();
                        send_sync_event(
                            &evt_tx,
                            SyncEvent::RelayHealthChanged {
                                health: relay_health,
                            },
                        );
                        None
                    }
                },
                None => {
                    tracing::debug!("No endpoint, skipping P2P stack spawn");
                    None
                }
            };

            // Gossip lives inside the stack (one instance, routed by the
            // Router) — a second instance here would never see inbound
            // connections. None in offline mode, like before.
            let gossip_host: Option<Gossip> = docs_stack
                .as_ref()
                .map(|stack| stack.gossip().clone());

            // Phase F.1: iroh-docs Engine holder — real when the stack
            // spawned, empty (mock fallback) in offline mode.
            let docs_engine_holder = Arc::new(match docs_stack.as_ref() {
                Some(stack) => IrohDocsEngineHolder::online(stack.clone()),
                None => IrohDocsEngineHolder::new(),
            });

            // TODO(ultrapilot): continuous relay-health monitoring.
            // `endpoint.inner().home_relay()` returns a `Watcher<Option<RelayUrl>>`
            // that would let us detect relay loss mid-session (Healthy ->
            // Degraded/Unreachable via `RelayHealth::on_error`, and recovery via
            // `on_success`), but wiring it requires watching the stream alongside
            // the select! loop below. Only the startup bind result and the
            // P2P-stack spawn outcome are tracked today — see AGENTS.md §relay-health.

            // Track active gossip subscriptions (topic key -> live handle) for verse/petal.
            let mut gossip_topics: HashMap<String, GossipTopic> = HashMap::new();

            // Phase E: per-verse replica map.
            let mut replicas: HashMap<String, Box<dyn VerseReplicator>> = HashMap::new();

            // A2: per-replica inbound pump handles (verse_id -> forwarder).
            let mut inbound_pumps: HashMap<String, tokio::task::AbortHandle> = HashMap::new();

            // A2/A4: aggregated inbound row stream. Every open replica's event
            // pump forwards (verse_id, RowChange) here; the select! loop drains
            // it alongside commands.
            let (inbound_tx, mut inbound_rx) =
                tokio::sync::mpsc::channel::<(String, RowChange)>(256);

            // Phase F.2: per-petal replica map.
            let mut petal_replicas: HashMap<String, Box<dyn PetalReplicator>> = HashMap::new();

            // Phase F.5: tileset download tracker
            let mut download_tracker = TilesetDownloadTracker::new();

            // crossbeam's receiver has no async readiness API — bridge the
            // command channel into a tokio channel so the select! below can
            // poll it next to the inbound pumps. Ordering is preserved (one
            // forwarder); backpressure mirrors the old blocking recv (the
            // bridge blocks while the sync thread is busy).
            let (async_cmd_tx, mut async_cmd_rx) =
                tokio::sync::mpsc::channel::<SyncCommand>(256);
            {
                let cmd_rx = cmd_rx;
                std::thread::spawn(move || {
                    while let Ok(cmd) = cmd_rx.recv() {
                        // blocking_send parks this bridge thread until the
                        // sync thread has capacity — never called from async
                        // context, which is exactly what blocking_send wants.
                        if cmd_rx_bridge_send(&async_cmd_tx, cmd).is_err() {
                            break;
                        }
                    }
                });
            }

            // Command loop (select! over commands + inbound replica rows).
            loop {
                tokio::select! {
                    maybe_row = inbound_rx.recv() => {
                        // `None` (all pumps closed) needs no handling —
                        // commands still flow.
                        if let Some((verse_id, change)) = maybe_row {
                            handle_inbound_row_change(
                                &verse_id,
                                &change,
                                &local_did,
                                &evt_tx,
                                &db_cmd_tx,
                            );
                        }
                    }
                    maybe_cmd = async_cmd_rx.recv() => {
                        match maybe_cmd {
                            Some(SyncCommand::FetchBlob { hash, verse_id }) => {
                                handle_fetch_blob(
                                    &blob_store,
                                    endpoint.as_ref(),
                                    &hash,
                                    &verse_id,
                                    &evt_tx,
                                );
                            }
                            Some(SyncCommand::OpenVerseReplica {
                                verse_id,
                                namespace_id,
                                namespace_secret,
                                bootstrap_peers: cmd_peers,
                            }) => {
                                // Merge the command's explicit peers with the
                                // env-configured bootstrap set (deduped).
                                let mut peers = bootstrap_peers.clone();
                                for entry in cmd_peers {
                                    if let Some(addr) = parse_bootstrap_peer(&entry) {
                                        if !peers.contains(&addr) {
                                            peers.push(addr);
                                        }
                                    }
                                }
                                handle_open_verse_replica(
                                    &mut replicas,
                                    &mut inbound_pumps,
                                    inbound_tx.clone(),
                                    docs_engine_holder.clone(),
                                    &verse_id,
                                    &namespace_id,
                                    namespace_secret,
                                    &peers,
                                    &local_did,
                                )
                                .await;
                                // Phase F.4: Subscribe to verse gossip topic
                                subscribe_to_verse_gossip_topic(
                                    &gossip_host,
                                    &mut gossip_topics,
                                    &verse_id,
                                );
                            }
                            Some(SyncCommand::CloseVerseReplica { verse_id }) => {
                                handle_close_verse_replica(
                                    &mut replicas,
                                    &mut inbound_pumps,
                                    &verse_id,
                                )
                                .await;
                                // Phase F.4: Unsubscribe from verse gossip topic
                                unsubscribe_from_verse_gossip_topic(
                                    &gossip_host,
                                    &mut gossip_topics,
                                    &verse_id,
                                );
                            }
                            Some(SyncCommand::WriteRowEntry {
                                verse_id,
                                table,
                                record_id,
                                content_hash,
                            }) => {
                                handle_write_row_entry(
                                    &replicas,
                                    &blob_store,
                                    &local_did,
                                    &verse_id,
                                    &table,
                                    &record_id,
                                    &content_hash,
                                )
                                .await;
                            }
                            Some(SyncCommand::UpdateNodeTransform {
                                verse_id,
                                node_id,
                                ..
                            }) => {
                                tracing::warn!(
                                    verse_id,
                                    node_id,
                                    "dropped unsigned transform sync command; a signed canonical operation is required before network forwarding"
                                );
                            }
                            Some(SyncCommand::SubscribePetal { petal_id }) => {
                                handle_subscribe_petal(
                                    &mut petal_replicas,
                                    docs_engine_holder.clone(),
                                    &petal_id,
                                    &local_did,
                                )
                                .await;
                            }
                            Some(SyncCommand::UnsubscribePetal { petal_id }) => {
                                handle_unsubscribe_petal(&mut petal_replicas, &petal_id).await;
                            }
                            Some(SyncCommand::SubmitComputeTask {
                                task_id,
                                query,
                                petal_scope,
                                requester_did,
                            }) => {
                                // TODO(Phase 6.2): execute compute task locally, emit ComputeResultReady
                                tracing::debug!(%task_id, %query, ?petal_scope, %requester_did, "SubmitComputeTask (stub)");
                            }
                            Some(SyncCommand::AdvertiseTilesets { verse_id, advertisements_json }) => {
                                handle_advertise_tilesets(
                                    &gossip_host,
                                    &gossip_topics,
                                    &advertisements_json,
                                    &verse_id,
                                )
                                .await;
                            }
                            Some(SyncCommand::RequestTilesetMeta { peer_id, tileset_id }) => {
                                handle_request_tileset_meta(&peer_id, &tileset_id, &evt_tx);
                            }
                            Some(SyncCommand::RequestChunk { peer_id, tileset_id, chunk_seq }) => {
                                handle_request_chunk(&peer_id, &tileset_id, chunk_seq, &evt_tx);
                            }
                            Some(SyncCommand::CancelTilesetDownload { tileset_id }) => {
                                handle_cancel_tileset_download(&mut download_tracker, &tileset_id);
                            }
                            Some(SyncCommand::Shutdown) => {
                                tracing::info!("Sync thread shutting down");
                                break;
                            }
                            None => {
                                // Channel closed — main thread dropped the sender.
                                tracing::info!("Sync command channel closed, shutting down");
                                break;
                            }
                        }
                    }
                }
            }

            // Close all open replicas before shutting down.
            for (vid, repl) in replicas.drain() {
                if let Err(e) = repl.close().await {
                    tracing::warn!("Error closing replica for verse {vid}: {e}");
                }
            }
            for (_, pump) in inbound_pumps.drain() {
                pump.abort();
            }

            // Shut the P2P stack's protocol handlers down (docs engine
            // flush) before the endpoint close.
            if let Some(stack) = &docs_stack {
                stack.shutdown().await;
            }

            // Graceful endpoint shutdown
            if let Some(ep) = endpoint {
                ep.shutdown().await;
            }
            send_sync_event(&evt_tx, SyncEvent::Stopped);
        });
    })
}

/// Bridge-thread helper: forward one command into the tokio channel.
///
/// Kept as a named fn so the bridge thread's blocking shape is greppable.
/// The `SendError` carries the unsent command back (`.0`) so the bridge can
/// stop cleanly on shutdown.
fn cmd_rx_bridge_send(
    tx: &tokio::sync::mpsc::Sender<SyncCommand>,
    cmd: SyncCommand,
) -> Result<(), SyncCommand> {
    tx.blocking_send(cmd).map_err(|e| e.0)
}

/// Forward one inbound replicated row to the DB thread (A4 seam).
///
/// Own-author rows are skipped first (E.8 loop prevention): the real Doc
/// path never echoes local writes (the pump skips `InsertLocal`), but the
/// offline mock fallback broadcasts every write to its subscribers —
/// re-applying our own row would fire spurious `RowApplied` events and a
/// redundant DB write.
///
/// Surviving rows emit [`SyncEvent::RowApplied`] and are sent as
/// [`DbCommand::ApplyReplicatedRow`] **on the DB thread's command channel** —
/// the DB thread is the single SurrealDB writer and the enforcement point.
/// The send is fire-and-forget with the drop-and-count backpressure contract
/// (§replication-backpressure): a full channel drops the row with a warn
/// (replication lag, not a stalled sync thread); a disconnected channel is
/// shutdown and silent.
fn handle_inbound_row_change(
    verse_id: &str,
    change: &RowChange,
    local_did: &str,
    evt_tx: &SyncEventSender,
    db_cmd_tx: &Option<crossbeam::channel::Sender<DbCommand>>,
) {
    if change.author_id == local_did {
        tracing::debug!(
            verse_id,
            record_id = %change.record_id,
            "Inbound row echoes our own write — skipped (loop prevention)"
        );
        return;
    }
    tracing::debug!(
        verse_id,
        table = %change.table,
        record_id = %change.record_id,
        author = %change.author_id,
        tombstone = change.is_tombstone,
        "Inbound replicated row from peer"
    );
    send_sync_event(
        evt_tx,
        SyncEvent::RowApplied {
            verse_id: verse_id.to_string(),
            table: change.table.clone(),
            record_id: change.record_id.clone(),
        },
    );

    let Some(tx) = db_cmd_tx else {
        tracing::debug!(
            verse_id,
            "No DB command channel — inbound row not applied (test mode)"
        );
        return;
    };
    match tx.try_send(DbCommand::ApplyReplicatedRow {
        verse_id: verse_id.to_string(),
        table: change.table.clone(),
        record_id: change.record_id.clone(),
        row_bytes: change.data.clone(),
        author_did: change.author_id.clone(),
    }) {
        Ok(()) => {}
        Err(crossbeam::channel::TrySendError::Full(_)) => {
            let dropped =
                INBOUND_APPLY_DROPS.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
            tracing::warn!(
                verse_id,
                table = %change.table,
                record_id = %change.record_id,
                dropped_total = dropped,
                "DB command channel full — dropping inbound replicated row"
            );
        }
        Err(crossbeam::channel::TrySendError::Disconnected(_)) => {}
    }
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
/// Creates an [`IrohDocsReplicator`] and opens its document: with the
/// namespace secret the capability is imported on the real stack, without it
/// the namespace opens read-only. A doc-backed replica then joins the live
/// sync swarm (dialing `peers`) and spawns its per-replica inbound event
/// pump, whose `RowChange`s are forwarded into `inbound_tx` — the aggregated
/// stream the command loop selects on (A2/A4). If a replica is already open
/// for this verse, it (and its pump) is closed first.
///
/// Every degradation on this path is loud but non-fatal: an offline stack or
/// unusable capability leaves the replicator mock-backed, never crashed.
#[allow(clippy::too_many_arguments)]
async fn handle_open_verse_replica(
    replicas: &mut HashMap<String, Box<dyn VerseReplicator>>,
    inbound_pumps: &mut HashMap<String, tokio::task::AbortHandle>,
    inbound_tx: tokio::sync::mpsc::Sender<(String, RowChange)>,
    engine_holder: Arc<IrohDocsEngineHolder>,
    verse_id: &str,
    namespace_id: &str,
    namespace_secret: Option<String>,
    peers: &[iroh::NodeAddr],
    local_did: &str,
) {
    // Close existing replica (and its inbound pump) if any.
    if let Some(old) = replicas.remove(verse_id) {
        tracing::debug!(verse_id, "Closing existing replica before re-open");
        if let Err(e) = old.close().await {
            tracing::warn!(
                verse_id,
                "Error closing existing replica before re-open: {e}"
            );
        }
        if let Some(pump) = inbound_pumps.remove(verse_id) {
            pump.abort();
        }
    }

    let secret = namespace_secret.unwrap_or_default();
    let replicator = IrohDocsReplicator::new(
        namespace_id.to_string(),
        secret,
        local_did.to_string(),
        engine_holder.clone(),
    );

    // Open the document for this namespace. Offline stacks return `Ok(())`
    // with no doc (mock stays); a genuine failure (unparseable capability)
    // warns loudly and also keeps the mock backing.
    if let Err(e) = replicator.open_document().await {
        tracing::warn!(
            verse_id,
            "Verse replica document open failed — mock fallback: {e}"
        );
    } else if replicator.is_doc_backed() {
        // Join the live sync swarm. `start_sync` runs even with no peers:
        // the dialed side must have its sync task running to serve entries
        // to peers that dial *us* (A2), and the bootstrap set is exactly the
        // peers we dial out to (A9).
        if let Err(e) = replicator.start_sync(peers.to_vec()).await {
            tracing::warn!(verse_id, "Replica start_sync failed: {e}");
        }
    }

    // Per-replica inbound event pump (A2/A4): every inbound RowChange is
    // forwarded into the aggregated stream the command loop selects on.
    match replicator.subscribe().await {
        Ok(rx) => {
            let pump_verse_id = verse_id.to_string();
            let pump_tx = inbound_tx.clone();
            let pump = tokio::spawn(async move {
                let mut rx = rx;
                while let Some(change) = rx.recv().await {
                    if pump_tx.send((pump_verse_id.clone(), change)).await.is_err() {
                        // Command loop dropped the aggregate stream — shutdown.
                        break;
                    }
                }
            });
            inbound_pumps.insert(verse_id.to_string(), pump.abort_handle());
        }
        Err(e) => {
            tracing::warn!(verse_id, "Replica subscribe failed — no inbound pump: {e}");
        }
    }

    replicas.insert(verse_id.to_string(), Box::new(replicator));

    // Phase F: compute gossip topic for this verse
    let topic_hash = verse_peers::verse_gossip_topic(verse_id);
    tracing::info!(
        verse_id,
        namespace_id,
        peers = peers.len(),
        gossip_topic = %hex::encode(topic_hash),
        "Opened verse replica — P2P stack {}",
        if engine_holder.is_available() { "online" } else { "offline (mock fallback)" }
    );
}

/// Handle [`SyncCommand::CloseVerseReplica`].
async fn handle_close_verse_replica(
    replicas: &mut HashMap<String, Box<dyn VerseReplicator>>,
    inbound_pumps: &mut HashMap<String, tokio::task::AbortHandle>,
    verse_id: &str,
) {
    // Stop the inbound pump first so no further rows are emitted mid-close.
    if let Some(pump) = inbound_pumps.remove(verse_id) {
        pump.abort();
    }
    if let Some(repl) = replicas.remove(verse_id) {
        if let Err(e) = repl.close().await {
            tracing::warn!(verse_id, "Error closing replica: {e}");
        }
        tracing::info!(verse_id, "Closed verse replica");
    } else {
        tracing::debug!(verse_id, "CloseVerseReplica: no open replica");
    }
}

/// Handle [`SyncCommand::WriteRowEntry`] (blob read off-loaded via spawn_blocking).
///
/// The §D1 write-policy gate was deliberately removed from this outbound
/// path: these commands originate from the local DB thread's replication
/// bridge (a local, already-admitted write being published to peers), so
/// gating here would only block our own publishes. Peer admission happens on
/// the inbound path (`DbCommand::ApplyReplicatedRow` on the DB thread —
/// single writer + fe-policy deny-by-default), not here.
#[allow(clippy::too_many_arguments)]
async fn handle_write_row_entry(
    replicas: &HashMap<String, Box<dyn VerseReplicator>>,
    blob_store: &BlobStoreHandle,
    author_did: &str,
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

    if let Err(e) = repl.write_row(table, record_id, &data).await {
        tracing::error!(
            verse_id,
            table,
            record_id,
            author = author_did,
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
async fn handle_subscribe_petal(
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
        if let Err(e) = old.close().await {
            tracing::warn!(petal_id, "Error closing existing petal replicator: {e}");
        }
    }

    // Derive namespace ID from petal_id
    let namespace_id = derive_namespace_id(petal_id);

    // Create the petal replicator
    let replicator =
        IrohPetalReplicator::new(petal_id.to_string(), namespace_id, local_did.to_string());

    petal_replicas.insert(petal_id.to_string(), Box::new(replicator));

    tracing::info!(
        petal_id,
        "Subscribed to petal — P2P stack {} (replicator mock-backed until the async rewrite)",
        if engine_holder.is_available() {
            "online"
        } else {
            "offline (mock fallback)"
        }
    );
}

/// Handle [`SyncCommand::UnsubscribePetal`].
///
/// Closes and removes the petal replicator from the map.
async fn handle_unsubscribe_petal(
    petal_replicas: &mut HashMap<String, Box<dyn PetalReplicator>>,
    petal_id: &str,
) {
    if let Some(repl) = petal_replicas.remove(petal_id) {
        if let Err(e) = repl.close().await {
            tracing::warn!(petal_id, "Error closing petal replicator: {e}");
        }
        tracing::info!(petal_id, "Unsubscribed from petal");
    } else {
        tracing::debug!(petal_id, "UnsubscribePetal: no open replicator");
    }
}

/// Derive a gossip topic from a verse ID.
///
/// The topic is used for allowed verse-scoped gossip such as tileset
/// announcements.
fn derive_gossip_topic(verse_id: &str) -> String {
    format!("verse:{}", verse_id)
}

/// Subscribe to a verse gossip topic.
///
/// This is called when opening a verse replica to make its permitted gossip
/// controls available.
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

    #[allow(dead_code)] // download-start bookkeeping — wiring lands with P2P chunk transfer
    fn start(&mut self, tileset_id: String, peer_id: String, chunk_seq: u32) {
        self.active.insert(tileset_id, (peer_id, chunk_seq));
    }

    fn cancel(&mut self, tileset_id: &str) -> Option<(String, u32)> {
        self.active.remove(tileset_id)
    }

    #[allow(dead_code)] // download-start bookkeeping — wiring lands with P2P chunk transfer
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
        tracing::debug!(
            len = advertisements_json.len(),
            "No gossip, skipping tileset advertise"
        );
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
        tracing::warn!(
            verse_id,
            "No gossip topic for verse, cannot advertise tilesets"
        );
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
fn handle_request_tileset_meta(peer_id: &str, tileset_id: &str, evt_tx: &SyncEventSender) {
    tracing::debug!(peer_id = %peer_id, tileset_id = %tileset_id, "RequestTilesetMeta (stub)");

    // Stub: emit a placeholder response
    // In full implementation, we'd look up actual metadata
    let meta_json = r#"{"error": "tileset not found locally"}"#;
    evt_tx
        .send(SyncEvent::TilesetMetaReceived {
            peer_id: peer_id.to_string(),
            tileset_id: tileset_id.to_string(),
            meta_json: meta_json.to_string(),
            total_chunks: 0,
            approx_size_bytes: 0,
        })
        .ok();
}

/// Handle [`SyncCommand::RequestChunk`].
///
/// Requests a chunk from a peer via iroh-blobs.
/// This is a stub - full implementation would use actual blob transfer.
fn handle_request_chunk(peer_id: &str, tileset_id: &str, chunk_seq: u32, evt_tx: &SyncEventSender) {
    tracing::debug!(peer_id = %peer_id, tileset_id = %tileset_id, chunk_seq, "RequestChunk (stub)");

    // Stub: emit failure since we can't actually transfer
    evt_tx
        .send(SyncEvent::ChunkFailed {
            tileset_id: tileset_id.to_string(),
            chunk_seq,
            reason: "chunk transfer not implemented in stub".to_string(),
        })
        .ok();
}

/// Handle [`SyncCommand::CancelTilesetDownload`].
///
/// Cancels an in-progress tileset download.
fn handle_cancel_tileset_download(download_tracker: &mut TilesetDownloadTracker, tileset_id: &str) {
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

    /// Serializes tests that mutate process env and spawn sync threads —
    /// env is process-global and cargo runs tests in parallel.
    static SYNC_THREAD_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Hermetic env for thread-spawning tests: loopback-only relay config
    /// (§loopback-only P2P tests) plus an isolated FE_P2P_DIR tempdir so the
    /// persistent stores never touch the working tree.
    fn hermetic_p2p_env() -> tempfile::TempDir {
        std::env::set_var(crate::relay_config::RELAY_CONFIG_ENV_VAR, "disabled");
        let dir = tempfile::TempDir::new().unwrap();
        std::env::set_var(crate::docs_engine::P2P_DIR_ENV_VAR, dir.path());
        dir
    }

    fn restore_p2p_env() {
        std::env::remove_var(crate::relay_config::RELAY_CONFIG_ENV_VAR);
        std::env::remove_var(crate::docs_engine::P2P_DIR_ENV_VAR);
    }

    #[test]
    fn shutdown_command_terminates_thread() {
        let _env_guard = SYNC_THREAD_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let p2p_dir = hermetic_p2p_env();
        let store: BlobStoreHandle = Arc::new(MockBlobStore::new());
        let secret = iroh::SecretKey::from_bytes(&[99u8; 32]);
        let (cmd_tx, cmd_rx) = crossbeam::channel::bounded(8);
        let (evt_tx, evt_rx) = crossbeam::channel::bounded(8);

        let handle = spawn_sync_thread(
            secret,
            store,
            cmd_rx,
            evt_tx,
            "test-node-did".to_string(),
            None,
            Some(p2p_dir.path().to_path_buf()),
        );
        // Wait for Started event (online or offline)
        let started = evt_rx.recv_timeout(std::time::Duration::from_secs(15));
        assert!(
            matches!(started, Ok(SyncEvent::Started { .. })),
            "expected Started event, got {started:?}"
        );

        // Startup also emits a RelayHealthChanged signal right after Started
        // (relay-health hardening — see AGENTS.md §relay-health).
        let health = evt_rx.recv_timeout(std::time::Duration::from_secs(2));
        assert!(
            matches!(health, Ok(SyncEvent::RelayHealthChanged { .. })),
            "expected RelayHealthChanged event, got {health:?}"
        );

        cmd_tx.send(SyncCommand::Shutdown).unwrap();
        handle.join().expect("sync thread panicked");

        // Drain any further RelayHealthChanged events (e.g. a P2P
        // stack-spawn failure in a sandboxed CI network) until the final
        // Stopped event.
        let stopped = loop {
            match evt_rx.recv_timeout(std::time::Duration::from_secs(2)) {
                Ok(SyncEvent::RelayHealthChanged { .. }) => continue,
                other => break other,
            }
        };
        assert!(
            matches!(stopped, Ok(SyncEvent::Stopped)),
            "expected Stopped event, got {stopped:?}"
        );
        restore_p2p_env();
    }

    /// A1 (test): with an online endpoint the sync thread spawns the real
    /// Blobs + Gossip + Docs + Router stack with persistent stores under
    /// `FE_P2P_DIR`. READ-BACK: the redb store and blobs dir must exist on
    /// disk, not merely the startup log lines.
    #[test]
    fn sync_thread_spawns_persistent_stack_when_online() {
        let _env_guard = SYNC_THREAD_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let p2p_dir = hermetic_p2p_env();
        let store: BlobStoreHandle = Arc::new(MockBlobStore::new());
        let secret = iroh::SecretKey::from_bytes(&[98u8; 32]);
        let (cmd_tx, cmd_rx) = crossbeam::channel::bounded(8);
        let (evt_tx, evt_rx) = crossbeam::channel::bounded(8);

        let handle = spawn_sync_thread(
            secret,
            store,
            cmd_rx,
            evt_tx,
            "test-node-did".to_string(),
            None,
            Some(p2p_dir.path().to_path_buf()),
        );
        let started = evt_rx.recv_timeout(std::time::Duration::from_secs(15));
        assert!(
            matches!(started, Ok(SyncEvent::Started { .. })),
            "expected Started event, got {started:?}"
        );

        if matches!(started, Ok(SyncEvent::Started { online: true, .. })) {
            // Started is emitted between the endpoint bind and the stack
            // spawn, so poll for the docs engine's redb store to appear.
            let docs_redb = p2p_dir.path().join("docs.redb");
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            while !docs_redb.is_file() && std::time::Instant::now() < deadline {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            assert!(
                docs_redb.is_file(),
                "docs.redb must exist under FE_P2P_DIR when online"
            );
            assert!(
                p2p_dir.path().join("blobs").is_dir(),
                "blobs fs store dir must exist under FE_P2P_DIR when online"
            );
        } else {
            // Sandboxed environment: the endpoint could not bind, so the
            // thread degraded to offline/mock (proven tolerantly by
            // shutdown_command_terminates_thread and the docs_engine tests).
            tracing::warn!(
                "endpoint offline in this environment — skipping online stack assertions"
            );
        }

        cmd_tx.send(SyncCommand::Shutdown).unwrap();
        handle.join().expect("sync thread panicked");
        let stopped = loop {
            match evt_rx.recv_timeout(std::time::Duration::from_secs(2)) {
                Ok(SyncEvent::RelayHealthChanged { .. }) => continue,
                other => break other,
            }
        };
        assert!(
            matches!(stopped, Ok(SyncEvent::Stopped)),
            "expected Stopped event, got {stopped:?}"
        );
        restore_p2p_env();
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

    #[tokio::test]
    async fn handle_subscribe_petal_creates_replicator() {
        let mut petal_replicas: HashMap<String, Box<dyn PetalReplicator>> = HashMap::new();
        let engine_holder = Arc::new(IrohDocsEngineHolder::new());
        let local_did = "did:test:local-author";

        handle_subscribe_petal(&mut petal_replicas, engine_holder, "petal-alpha", local_did).await;

        assert!(
            petal_replicas.contains_key("petal-alpha"),
            "petal-alpha should be in replica map"
        );
    }

    #[tokio::test]
    async fn handle_subscribe_petal_idempotent_resubscribe() {
        let mut petal_replicas: HashMap<String, Box<dyn PetalReplicator>> = HashMap::new();
        let engine_holder = Arc::new(IrohDocsEngineHolder::new());
        let local_did = "did:test:local-author";

        // First subscription
        handle_subscribe_petal(
            &mut petal_replicas,
            engine_holder.clone(),
            "petal-beta",
            local_did,
        )
        .await;
        let first_count = petal_replicas.len();

        // Re-subscription should replace old replicator (idempotent)
        handle_subscribe_petal(
            &mut petal_replicas,
            engine_holder.clone(),
            "petal-beta",
            local_did,
        )
        .await;
        let second_count = petal_replicas.len();

        assert_eq!(
            first_count, second_count,
            "re-subscribe should not add duplicate"
        );
        assert!(petal_replicas.contains_key("petal-beta"));
    }

    #[tokio::test]
    async fn handle_unsubscribe_petal_removes_replicator() {
        let mut petal_replicas: HashMap<String, Box<dyn PetalReplicator>> = HashMap::new();
        let engine_holder = Arc::new(IrohDocsEngineHolder::new());
        let local_did = "did:test:local-author";

        // Subscribe first
        handle_subscribe_petal(&mut petal_replicas, engine_holder, "petal-gamma", local_did).await;
        assert!(petal_replicas.contains_key("petal-gamma"));

        // Unsubscribe
        handle_unsubscribe_petal(&mut petal_replicas, "petal-gamma").await;

        assert!(
            !petal_replicas.contains_key("petal-gamma"),
            "petal-gamma should be removed after unsubscribe"
        );
    }

    #[tokio::test]
    async fn handle_unsubscribe_petal_nonexistent_is_noop() {
        let mut petal_replicas: HashMap<String, Box<dyn PetalReplicator>> = HashMap::new();

        // Unsubscribe non-existent should not panic
        handle_unsubscribe_petal(&mut petal_replicas, "petal-missing").await;

        assert!(petal_replicas.is_empty(), "map should remain empty");
    }

    #[tokio::test]
    async fn petal_replicator_write_and_subscribe() {
        use crate::replicator::IrohPetalReplicator;

        // Create a petal replicator
        let repl = IrohPetalReplicator::new(
            "test-petal".to_string(),
            "petal-ns-test".to_string(),
            "did:test:author".to_string(),
        );

        // Write a row
        repl.write_row("nodes", "node-001", b"{\"name\": \"test\"}")
            .await
            .expect("write_row should succeed");

        // Subscribe to changes
        let mut rx = repl.subscribe().await.expect("subscribe should succeed");

        // Write another row - should trigger notification
        repl.write_row("nodes", "node-002", b"{\"name\": \"test2\"}")
            .await
            .expect("write_row should succeed");

        // Check we received a change notification (non-blocking)
        let change = rx.try_recv();
        assert!(
            change.is_ok(),
            "should receive change notification after write"
        );
    }

    #[test]
    fn derive_gossip_topic_is_deterministic() {
        let topic1 = derive_gossip_topic("verse-abc");
        let topic2 = derive_gossip_topic("verse-abc");
        let topic3 = derive_gossip_topic("verse-xyz");

        assert_eq!(topic1, topic2, "same verse_id should produce same topic");
        assert_ne!(
            topic1, topic3,
            "different verse_ids should produce different topics"
        );
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

        assert!(
            gossip_topics.is_empty(),
            "map should be empty after unsubscribe"
        );
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
        assert!(
            tracker.get("ts-001").is_some(),
            "download should be tracked"
        );

        // Cancel it
        let result = tracker.cancel("ts-001");
        assert!(result.is_some(), "cancel should return the download info");
        assert!(
            tracker.get("ts-001").is_none(),
            "download should be removed after cancel"
        );
    }

    #[test]
    fn tileset_download_tracker_cancel_nonexistent() {
        let mut tracker = TilesetDownloadTracker::new();

        // Cancel something that doesn't exist
        let result = tracker.cancel("ts-nonexistent");
        assert!(
            result.is_none(),
            "canceling non-existent should return None"
        );
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
        rt.block_on(handle_advertise_tilesets(
            &gossip_host,
            &gossip_topics,
            ads_json,
            "verse-1",
        ));
    }

    // -------------------------------------------------------------------------
    // A2/A4: bootstrap peer parsing + inbound apply seam
    // -------------------------------------------------------------------------

    #[test]
    fn parse_bootstrap_peer_accepts_node_addr_json() {
        // Round-trip through iroh's own serde types — the exact form
        // `SyncEvent::Started.node_addr` emits.
        let addr = iroh::NodeAddr::new(iroh::SecretKey::from_bytes(&[7u8; 32]).public());
        let json = serde_json::to_string(&addr).unwrap();
        assert_eq!(parse_bootstrap_peer(&json), Some(addr));
    }

    #[test]
    fn parse_bootstrap_peer_rejects_bare_node_id() {
        // A bare NodeId carries no address — undialable, so skipped loudly.
        let node_id = iroh::SecretKey::from_bytes(&[8u8; 32]).public();
        assert_eq!(parse_bootstrap_peer(&node_id.to_string()), None);
    }

    #[test]
    fn parse_bootstrap_peer_rejects_garbage() {
        assert_eq!(parse_bootstrap_peer("not-a-peer"), None);
        assert_eq!(parse_bootstrap_peer("  "), None);
    }

    #[test]
    fn bootstrap_peers_from_env_parses_valid_and_skips_invalid() {
        let _env_guard = SYNC_THREAD_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let addr = iroh::NodeAddr::new(iroh::SecretKey::from_bytes(&[9u8; 32]).public());
        let json = serde_json::to_string(&addr).unwrap();
        // Semicolon-separated: NodeAddr JSON contains commas, so the list
        // separator must not be a comma (this test pins that exact contract).
        std::env::set_var(BOOTSTRAP_ENV_VAR, format!("{json};not-a-peer;;{json}"));

        let peers = bootstrap_peers_from_env();
        std::env::remove_var(BOOTSTRAP_ENV_VAR);
        assert_eq!(peers.len(), 2, "both valid entries parse, garbage skipped");
        assert!(peers.contains(&addr));
    }

    #[test]
    fn inbound_row_change_emits_event_and_applies_via_db_command() {
        let (evt_tx, evt_rx) = crossbeam::channel::bounded(8);
        let (db_tx, db_rx) = crossbeam::channel::bounded(8);
        let payload = br#"{"name":"from peer"}"#.to_vec();
        let change = RowChange {
            table: "node".to_string(),
            record_id: "node-42".to_string(),
            content_hash: [1u8; 32],
            author_id: "did:key:peer-a".to_string(),
            timestamp: 1234,
            is_tombstone: false,
            data: payload.clone(),
        };

        handle_inbound_row_change("verse-1", &change, "did:key:local", &evt_tx, &Some(db_tx));

        // Event seam: RowApplied is emitted for the UI layer.
        let evt = evt_rx.try_recv().expect("RowApplied event expected");
        assert!(matches!(
            evt,
            SyncEvent::RowApplied {
                ref verse_id,
                ref table,
                ref record_id,
            } if verse_id == "verse-1" && table == "node" && record_id == "node-42"
        ));
        // DB seam (A4): the apply rides a DbCommand on the DB thread —
        // the single-writer path — carrying the payload bytes.
        let cmd = db_rx
            .try_recv()
            .expect("ApplyReplicatedRow DbCommand expected");
        assert!(matches!(
            cmd,
            DbCommand::ApplyReplicatedRow {
                ref verse_id,
                ref table,
                ref record_id,
                ref row_bytes,
                ref author_did,
            } if verse_id == "verse-1" && table == "node" && record_id == "node-42"
                && row_bytes == &payload && author_did == "did:key:peer-a"
        ));
    }

    #[test]
    fn inbound_row_change_skips_own_author_loop_prevention() {
        let (evt_tx, evt_rx) = crossbeam::channel::bounded(8);
        let (db_tx, db_rx) = crossbeam::channel::bounded(8);
        let change = RowChange {
            table: "node".to_string(),
            record_id: "node-echo".to_string(),
            content_hash: [3u8; 32],
            author_id: "did:key:local".to_string(),
            timestamp: 55,
            is_tombstone: false,
            data: br#"{"name":"own echo"}"#.to_vec(),
        };

        handle_inbound_row_change("verse-9", &change, "did:key:local", &evt_tx, &Some(db_tx));

        assert!(
            evt_rx.try_recv().is_err(),
            "own-author rows must not emit RowApplied (loop prevention)"
        );
        assert!(
            db_rx.try_recv().is_err(),
            "own-author rows must not re-apply on the DB thread"
        );
    }

    #[test]
    fn inbound_row_change_without_db_channel_emits_event_only() {
        let (evt_tx, evt_rx) = crossbeam::channel::bounded(8);
        let change = RowChange {
            table: "petal".to_string(),
            record_id: "petal-7".to_string(),
            content_hash: [2u8; 32],
            author_id: "did:key:peer-b".to_string(),
            timestamp: 99,
            is_tombstone: true,
            data: Vec::new(),
        };

        handle_inbound_row_change("verse-2", &change, "did:key:local", &evt_tx, &None);

        assert!(matches!(
            evt_rx.try_recv(),
            Ok(SyncEvent::RowApplied { .. })
        ));
    }

    #[test]
    fn inbound_row_change_drops_and_counts_on_full_db_channel() {
        let (evt_tx, _evt_rx) = crossbeam::channel::bounded(8);
        let (db_tx, _db_rx) = crossbeam::channel::bounded(1);
        let change = RowChange {
            table: "node".to_string(),
            record_id: "node-full".to_string(),
            content_hash: [4u8; 32],
            author_id: "did:key:peer-c".to_string(),
            timestamp: 7,
            is_tombstone: false,
            data: br#"{"name":"overflow"}"#.to_vec(),
        };

        // Fill the DB channel to capacity.
        db_tx
            .send(DbCommand::ApplyReplicatedRow {
                verse_id: "verse-3".to_string(),
                table: "node".to_string(),
                record_id: "filler".to_string(),
                row_bytes: Vec::new(),
                author_did: "did:key:filler".to_string(),
            })
            .unwrap();

        let before = inbound_apply_drop_count();
        handle_inbound_row_change("verse-3", &change, "did:key:local", &evt_tx, &Some(db_tx));
        assert!(
            inbound_apply_drop_count() > before,
            "a full DB channel must drop-and-count, never block the sync thread"
        );
    }

    /// A2/A4 (offline holder — mock fallback): opening a verse replica
    /// registers both the replica and its inbound pump; closing removes both.
    #[tokio::test]
    async fn open_and_close_verse_replica_manage_replica_and_pump() {
        let mut replicas: HashMap<String, Box<dyn VerseReplicator>> = HashMap::new();
        let mut inbound_pumps: HashMap<String, tokio::task::AbortHandle> = HashMap::new();
        let (inbound_tx, _inbound_rx) = tokio::sync::mpsc::channel::<(String, RowChange)>(16);
        let engine_holder = Arc::new(IrohDocsEngineHolder::new()); // offline → mock

        handle_open_verse_replica(
            &mut replicas,
            &mut inbound_pumps,
            inbound_tx.clone(),
            engine_holder.clone(),
            "verse-1",
            "0".repeat(64).as_str(),
            None,
            &[],
            "did:key:local",
        )
        .await;

        assert!(replicas.contains_key("verse-1"), "replica registered");
        assert!(
            inbound_pumps.contains_key("verse-1"),
            "inbound pump spawned with the replica"
        );

        // Idempotent re-open replaces both without leaking.
        handle_open_verse_replica(
            &mut replicas,
            &mut inbound_pumps,
            inbound_tx.clone(),
            engine_holder,
            "verse-1",
            "0".repeat(64).as_str(),
            None,
            &[],
            "did:key:local",
        )
        .await;
        assert_eq!(replicas.len(), 1);
        assert_eq!(inbound_pumps.len(), 1);

        handle_close_verse_replica(&mut replicas, &mut inbound_pumps, "verse-1").await;
        assert!(!replicas.contains_key("verse-1"), "replica closed");
        assert!(!inbound_pumps.contains_key("verse-1"), "pump aborted");
    }

    /// A4 end-of-seam (offline holder): a mock-backed replica's own-author
    /// rows flow through the pump into the aggregated inbound stream, where
    /// `handle_inbound_row_change` filters them (loop prevention) — proving
    /// the pump wiring forwards and the seam filters.
    #[tokio::test]
    async fn mock_replica_pump_feeds_inbound_stream_and_own_writes_are_filtered() {
        let mut replicas: HashMap<String, Box<dyn VerseReplicator>> = HashMap::new();
        let mut inbound_pumps: HashMap<String, tokio::task::AbortHandle> = HashMap::new();
        let (inbound_tx, mut inbound_rx) = tokio::sync::mpsc::channel::<(String, RowChange)>(16);
        let engine_holder = Arc::new(IrohDocsEngineHolder::new()); // offline → mock

        handle_open_verse_replica(
            &mut replicas,
            &mut inbound_pumps,
            inbound_tx,
            engine_holder,
            "verse-pump",
            "0".repeat(64).as_str(),
            None,
            &[],
            "did:key:local",
        )
        .await;

        let repl = replicas.get("verse-pump").expect("replica present");
        repl.write_row("node", "node-1", br#"{"name":"written locally"}"#)
            .await
            .expect("mock write succeeds");

        // The pump must forward the write into the aggregated inbound stream.
        let (verse_id, change) = inbound_rx
            .recv()
            .await
            .expect("pump forwarded the row into the inbound stream");
        assert_eq!(verse_id, "verse-pump");
        assert_eq!(change.record_id, "node-1");
        assert_eq!(change.data, br#"{"name":"written locally"}"#.to_vec());

        // The seam filters our own author (mock echo) before any DB apply.
        let (evt_tx, evt_rx) = crossbeam::channel::bounded(8);
        handle_inbound_row_change(&verse_id, &change, "did:key:local", &evt_tx, &None);
        assert!(
            evt_rx.try_recv().is_err(),
            "own-author rows are filtered at the seam"
        );

        handle_close_verse_replica(&mut replicas, &mut inbound_pumps, "verse-pump").await;
    }
}
