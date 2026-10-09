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

use iroh_gossip::net::{Event as GossipTopicEvent, Gossip, GossipEvent, GossipReceiver};
use iroh_gossip::proto::TopicId;

use crate::distributed_query::{
    handle_gossip_incoming, submit_distributed_query, DistributedTransport, GossipIncoming,
    TopicSender,
};

use crate::docs_engine::{p2p_data_dir, DocsStack};
use crate::endpoint::SyncEndpoint;
use crate::messages::{SyncCommand, SyncCommandReceiver, SyncEvent, SyncEventSender};
use crate::placement::Retention;
use crate::relay_config::{RelayConfig, RelayHealth};
use crate::replicator::{
    IrohDocsEngineHolder, IrohDocsReplicator, IrohPetalReplicator, PetalReplicator,
    ReplicatorFuture, RowChange, VerseReplicator,
};
use crate::sharding::{PeerDeclaration, ShardId, VerseFabric, PEER_DECL_TABLE, SHARD_TABLE};
use crate::verse_peers;
use crate::virtual_transport::{VirtualReplica, VirtualTransportFactory};

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

/// Retention cap for pending writes: far beyond any real emit-before-open
/// race window, while bounding memory on a process that never opens some
/// verse's replica. Past the cap the OLDEST entry is dropped with a warn.
const PENDING_WRITES_CAP: usize = 1024;

/// One row write that arrived while its verse's replica was not open yet.
#[derive(Debug, Clone)]
struct PendingWrite {
    table: String,
    record_id: String,
    content_hash: fe_runtime::blob_store::BlobHash,
}

/// Writes retained for verses whose replica is not open yet — the F21/M2
/// verse-manifest open race (see AGENTS.md §pending-writes).
///
/// `create_verse_handler` emits the manifest `ReplicationEvent` roughly
/// 100ms BEFORE the host's open (`VerseCreated` system on the relay,
/// navigation on the GUI) reaches this thread, so pre-fix the FIRST
/// manifest row hit the no-open-replica warn-and-drop and never entered the
/// doc — `seed_reconciliation` cannot heal a row that never entered, and a
/// fresh peer joining then never received the manifest, defeating the
/// bootstrap-window contract. Retained writes are republished in FIFO order
/// through the same write path a live write takes after the replica opens
/// successfully, so publish/conflict semantics are identical to the live
/// path. Retention is process-lifetime only: sync-thread shutdown drops
/// whatever is still queued with a warn (rows stay durable in the local DB;
/// only their publish is lost this session).
#[derive(Default)]
struct PendingWrites {
    entries: std::collections::VecDeque<(String, PendingWrite)>,
}

impl PendingWrites {
    /// Retain a write for later publication. Bounded: past
    /// [`PENDING_WRITES_CAP`] the oldest entry is dropped so the queue
    /// keeps the newest versions — on flush the doc still converges to the
    /// latest row content per key.
    fn push(&mut self, verse_id: &str, write: PendingWrite) {
        while self.entries.len() >= PENDING_WRITES_CAP {
            if let Some((verse, dropped)) = self.entries.pop_front() {
                tracing::warn!(
                    verse_id = %verse,
                    table = %dropped.table,
                    record_id = %dropped.record_id,
                    cap = PENDING_WRITES_CAP,
                    "Pending-writes cap reached — dropping the oldest retained write (the row stays durable in the local DB)"
                );
            }
        }
        tracing::debug!(
            verse_id,
            table = %write.table,
            record_id = %write.record_id,
            queued = self.entries.len() + 1,
            "WriteRowEntry retained pending — verse replica not open yet (republished after the open)"
        );
        self.entries.push_back((verse_id.to_string(), write));
    }

    /// Take (and clear) the FIFO queue for one verse.
    fn take(&mut self, verse_id: &str) -> Vec<PendingWrite> {
        let mut out = Vec::new();
        let mut i = 0;
        while i < self.entries.len() {
            if self.entries[i].0 == verse_id {
                if let Some((_, write)) = self.entries.remove(i) {
                    out.push(write);
                }
            } else {
                i += 1;
            }
        }
        out
    }

    fn len(&self) -> usize {
        self.entries.len()
    }

    fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
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
/// shutdown. See [`spawn_sync_thread_with_transport`] for the argument
/// reference (this is the production shape — no virtual transport).
pub fn spawn_sync_thread(
    secret_key: iroh::SecretKey,
    blob_store: BlobStoreHandle,
    cmd_rx: SyncCommandReceiver,
    evt_tx: SyncEventSender,
    local_did: String,
    db_cmd_tx: Option<crossbeam::channel::Sender<DbCommand>>,
    p2p_dir: Option<PathBuf>,
) -> std::thread::JoinHandle<()> {
    spawn_sync_thread_with_transport(
        secret_key, blob_store, cmd_rx, evt_tx, local_did, db_cmd_tx, p2p_dir, None,
    )
}

/// The per-verse replica a sync thread holds, over either transport: the
/// real iroh-docs replicator or a sim-lab virtual replica (F8/A19). Both
/// arms satisfy the same [`VerseReplicator`] contract, so the
/// open/inbound/apply path is byte-identical between prod and sim; the enum
/// only adds the open-phase lifecycle passthroughs the open sequence drives.
enum AnyReplicator {
    Iroh(Box<IrohDocsReplicator>),
    Virtual(Box<dyn VirtualReplica>),
}

impl VerseReplicator for AnyReplicator {
    fn write_row(
        &self,
        table: &str,
        record_id: &str,
        data: &[u8],
    ) -> ReplicatorFuture<'_, anyhow::Result<()>> {
        match self {
            Self::Iroh(r) => r.write_row(table, record_id, data),
            Self::Virtual(r) => r.write_row(table, record_id, data),
        }
    }

    fn subscribe(
        &self,
    ) -> ReplicatorFuture<'_, anyhow::Result<tokio::sync::mpsc::Receiver<RowChange>>> {
        match self {
            Self::Iroh(r) => r.subscribe(),
            Self::Virtual(r) => r.subscribe(),
        }
    }

    fn snapshot(&self) -> ReplicatorFuture<'_, anyhow::Result<Vec<RowChange>>> {
        match self {
            Self::Iroh(r) => r.snapshot(),
            Self::Virtual(r) => r.snapshot(),
        }
    }

    fn close(&self) -> ReplicatorFuture<'_, anyhow::Result<()>> {
        match self {
            Self::Iroh(r) => r.close(),
            Self::Virtual(r) => r.close(),
        }
    }
}

impl AnyReplicator {
    /// The open-phase lifecycle. The iroh arm's `open_document`/`start_sync`
    /// are inherent async methods (bare futures), so they are boxed here to
    /// the shared `ReplicatorFuture` shape; the virtual arm already returns
    /// boxed futures through the [`VirtualReplica`] trait.
    fn open_document(&self) -> ReplicatorFuture<'_, anyhow::Result<()>> {
        match self {
            Self::Iroh(r) => Box::pin(r.open_document()),
            Self::Virtual(r) => r.open_document(),
        }
    }

    fn is_doc_backed(&self) -> bool {
        match self {
            Self::Iroh(r) => r.is_doc_backed(),
            Self::Virtual(r) => r.is_doc_backed(),
        }
    }

    fn start_sync(&self, peers: Vec<iroh::NodeAddr>) -> ReplicatorFuture<'_, anyhow::Result<()>> {
        match self {
            Self::Iroh(r) => Box::pin(r.start_sync(peers)),
            Self::Virtual(r) => r.start_sync(peers),
        }
    }

    fn mark_open_failed(&self, reason: String) {
        match self {
            Self::Iroh(r) => r.mark_open_failed(reason),
            Self::Virtual(r) => r.mark_open_failed(reason),
        }
    }

    fn open_error(&self) -> Option<String> {
        match self {
            Self::Iroh(r) => r.open_error(),
            Self::Virtual(r) => r.open_error(),
        }
    }
}

/// Which transport a sync thread builds per-verse replicas on: the real
/// iroh-docs stack, or the sim lab's virtual transport factory (F8/A19 —
/// a sync thread with a factory binds no iroh endpoint at all).
#[derive(Clone)]
enum ReplicaTransport {
    Iroh(Arc<IrohDocsEngineHolder>),
    Virtual(Arc<dyn VirtualTransportFactory>),
}

impl ReplicaTransport {
    fn is_available(&self) -> bool {
        match self {
            Self::Iroh(holder) => holder.is_available(),
            Self::Virtual(factory) => factory.is_available(),
        }
    }

    /// Banner label for the open log line.
    fn describe(&self) -> &'static str {
        match self {
            Self::Iroh(holder) => {
                if holder.is_available() {
                    "online"
                } else {
                    "offline (mock fallback)"
                }
            }
            Self::Virtual(factory) => factory.describe(),
        }
    }
}

/// Spawn a sync thread, optionally on a **virtual transport** (F8/A19, the
/// sim lab): with a [`VirtualTransportFactory`] installed the thread binds
/// **no iroh endpoint** (no real network — everything is in-process) and
/// sources every verse replica from the factory instead of the iroh-docs
/// stack. The command loop, pending-writes retention, inbound pump,
/// startup reconciliation, and fabric bookkeeping all run identically —
/// only the transport under the [`VerseReplicator`] trait differs, so the
/// prod and sim replication paths cannot drift by construction (D3).
///
/// Gossip (the distributed-query compute plane) requires the real stack
/// and is absent in virtual mode — `SubmitComputeTask` answers honestly
/// that the verse has no topic. A21's CI scenarios virtualize that plane
/// on top of this seam (see fe-sync/src/AGENTS.md §virtual-transport).
///
/// # Arguments
/// * `secret_key` — deterministic ed25519 seed for the iroh endpoint (used
///   only on the real path; a virtual transport derives identity from the
///   factory, never the key).
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
///   [`crate::docs_engine::P2P_DIR_ENV_VAR`] (env, default `data/p2p`). No
///   store is touched in virtual mode.
/// * `virtual_transport` — the sim lab's transport factory. `None` (every
///   production host) builds replicas on the real iroh-docs stack.
#[allow(clippy::too_many_arguments)]
pub fn spawn_sync_thread_with_transport(
    secret_key: iroh::SecretKey,
    blob_store: BlobStoreHandle,
    cmd_rx: SyncCommandReceiver,
    evt_tx: SyncEventSender,
    local_did: String,
    db_cmd_tx: Option<crossbeam::channel::Sender<DbCommand>>,
    p2p_dir: Option<PathBuf>,
    virtual_transport: Option<Arc<dyn VirtualTransportFactory>>,
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

            // The node's iroh identity (the same ed25519 key the DID derives
            // from — F20 alignment). On the real path this IS the endpoint's
            // NodeId; in virtual mode it is the identity the sim hub tags
            // this peer's gossip deliveries with (F9/A21), so the F23
            // forged-attribution gate authenticates sim envelopes verbatim.
            let local_node = secret_key.public();

            // Phase F.1: Create the iroh endpoint first. A virtual transport
            // (sim lab, F8/A19) skips the bind entirely — no real network, no
            // relay, no stack; replicas come from the factory instead.
            let mut relay_health: RelayHealth;
            let endpoint = match virtual_transport.as_ref() {
                Some(factory) => {
                    tracing::info!(
                        transport = factory.describe(),
                        "Sync thread started (virtual transport — no iroh endpoint)"
                    );
                    // A virtual transport needs no relay and never dials:
                    // `Disabled` is the honest health state, not a failure.
                    send_sync_event(
                        &evt_tx,
                        SyncEvent::Started {
                            online: true,
                            node_addr: None,
                        },
                    );
                    relay_health = RelayHealth::Disabled;
                    send_sync_event(
                        &evt_tx,
                        SyncEvent::RelayHealthChanged {
                            health: relay_health,
                        },
                    );
                    None
                }
                None => match SyncEndpoint::new(secret_key, &relay_config).await {
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

            // F8/A19: which transport per-verse replicas build on — the
            // factory when a virtual transport was installed, else the real
            // iroh-docs stack (this holder is empty in virtual mode and only
            // the legacy petal-replica path consults it there).
            let replica_transport = match virtual_transport.as_ref() {
                Some(factory) => ReplicaTransport::Virtual(factory.clone()),
                None => ReplicaTransport::Iroh(docs_engine_holder.clone()),
            };

            // TODO(ultrapilot): continuous relay-health monitoring.
            // `endpoint.inner().home_relay()` returns a `Watcher<Option<RelayUrl>>`
            // that would let us detect relay loss mid-session (Healthy ->
            // Degraded/Unreachable via `RelayHealth::on_error`, and recovery via
            // `on_success`), but wiring it requires watching the stream alongside
            // the select! loop below. Only the startup bind result and the
            // P2P-stack spawn outcome are tracked today — see AGENTS.md §relay-health.

            // Track active gossip subscriptions (topic key -> live sender)
            // plus the per-topic inbound pumps. F7 splits each GossipTopic:
            // the sender half broadcasts (tileset ads + distributed queries),
            // the receiver half drains in a pump task feeding the aggregated
            // gossip inbound stream the select! loop below consumes. F9/A21:
            // the sender is a `TopicSender` — the real iroh-gossip half, or
            // the sim hub's virtual topic when a virtual transport is
            // installed (the compute plane then carries the hub's scripted
            // latency/partition/churn; see AGENTS.md §virtual-transport).
            let mut gossip_senders: HashMap<String, TopicSender> = HashMap::new();
            let mut gossip_pumps: HashMap<String, tokio::task::AbortHandle> = HashMap::new();
            let (gossip_inbound_tx, mut gossip_inbound_rx) =
                tokio::sync::mpsc::channel::<GossipIncoming>(64);

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

            // F21/M2: writes retained for verses whose replica is not open
            // yet, republished after the open (see PendingWrites).
            let mut pending_writes = PendingWrites::default();

            // M2/F6: per-verse timeseries fabric (settings from the verse
            // manifest, peer declarations, shard ledger). Sync-thread state,
            // NOT DB state — the DB thread maps `__shards`/`__peers` rows to
            // NotApplicable. See AGENTS.md §sharding.
            let mut fabrics: HashMap<String, VerseFabric> = HashMap::new();

            // M2/F6: the local peer's shard-hosting declaration, applied to
            // every open verse's fabric (published as `__peers/{local_did}`).
            // Defaults to unlimited capacity, non-seeder — a plain node.
            let mut local_declaration = PeerDeclaration::default();

            // M2/F7: the distributed-query transport handle shared by the
            // select loop (submit + inbound routing) and the collector /
            // responder tasks. Bounded concurrency + pending-request caps
            // live inside it.
            let transport = DistributedTransport::new(
                local_did.clone(),
                db_cmd_tx.clone(),
                evt_tx.clone(),
            );

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
                        if !cmd_rx_bridge_send(&async_cmd_tx, cmd) {
                            break;
                        }
                    }
                });
            }

            // Command loop (select! over commands + inbound replica rows +
            // inbound gossip compute traffic).
            loop {
                tokio::select! {
                    maybe_row = inbound_rx.recv() => {
                        // `None` (all pumps closed) needs no handling —
                        // commands still flow.
                        if let Some((verse_id, change)) = maybe_row {
                            let fabric = fabrics.entry(verse_id.clone()).or_default();
                            handle_inbound_row_change(
                                fabric,
                                &verse_id,
                                &change,
                                &local_did,
                                &evt_tx,
                                &db_cmd_tx,
                            );
                        }
                    }
                    maybe_gossip = gossip_inbound_rx.recv() => {
                        if let Some(incoming) = maybe_gossip {
                            handle_gossip_incoming(
                                &transport,
                                &fabrics,
                                &gossip_senders,
                                incoming,
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
                                // Phase F.4 + M2/F7: subscribe the verse's
                                // compute gossip topic BEFORE the open
                                // sequence. The gossip actor is an independent
                                // task and DROPS a Join that arrives for a
                                // topic this node has not subscribed to yet —
                                // and the joining peers dial out as soon as
                                // THEIR opens run, so every step between this
                                // command and the subscribe (doc dial,
                                // start_sync, seed reconciliation) is a race
                                // window in which a peer's one-shot Join is
                                // silently lost, and with it that peer's
                                // compute request/response path for the whole
                                // session. Subscribing first makes the window
                                // microseconds against the joiners'
                                // millisecond dials.
                                subscribe_to_verse_gossip_topic(
                                    endpoint.as_ref(),
                                    &gossip_host,
                                    &virtual_transport,
                                    &local_did,
                                    local_node,
                                    &mut gossip_senders,
                                    &mut gossip_pumps,
                                    gossip_inbound_tx.clone(),
                                    &verse_id,
                                    &peers,
                                );
                                handle_open_verse_replica(
                                    &mut replicas,
                                    &mut inbound_pumps,
                                    inbound_tx.clone(),
                                    &replica_transport,
                                    &verse_id,
                                    &namespace_id,
                                    namespace_secret,
                                    &peers,
                                    &local_did,
                                    &evt_tx,
                                    &db_cmd_tx,
                                    &blob_store,
                                    &mut pending_writes,
                                    fabrics.entry(verse_id.clone()).or_default(),
                                    local_declaration,
                                )
                                .await;
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
                                    &mut gossip_senders,
                                    &mut gossip_pumps,
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
                                    &mut pending_writes,
                                    fabrics.entry(verse_id.clone()).or_default(),
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
                            Some(SyncCommand::SubmitComputeTask { call }) => {
                                // M2/F7: real fan-out transport. Plans against
                                // the verse's fabric, broadcasts over the
                                // verse's gossip topic with bounded
                                // concurrency, gathers per-host partials until
                                // settled or the deadline, merges
                                // commutatively, replies with covered/
                                // missing shard honesty metadata, and emits
                                // `ComputeResultReady` for the diagnostics
                                // surface. Runs synchronously here (no
                                // awaits) — the collector is a spawned task.
                                submit_distributed_query(
                                    &transport,
                                    &fabrics,
                                    &gossip_senders,
                                    call,
                                );
                            }
                            Some(SyncCommand::AdvertiseTilesets { verse_id, advertisements_json }) => {
                                handle_advertise_tilesets(
                                    &gossip_host,
                                    &gossip_senders,
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
                            Some(SyncCommand::SetShardDeclaration {
                                capacity_bytes,
                                seeder,
                            }) => {
                                local_declaration = PeerDeclaration {
                                    capacity_bytes,
                                    seeder,
                                };
                                handle_set_shard_declaration(
                                    &replicas,
                                    &mut fabrics,
                                    &local_did,
                                    local_declaration,
                                )
                                .await;
                            }
                            Some(SyncCommand::GetShardLedger { verse_id }) => {
                                let ledger_json = fabrics
                                    .get(&verse_id)
                                    .map(|f| f.to_dump_json())
                                    .unwrap_or_else(|| serde_json::Value::Null);
                                send_sync_event(
                                    &evt_tx,
                                    SyncEvent::ShardLedger {
                                        verse_id,
                                        ledger_json: ledger_json.to_string(),
                                    },
                                );
                            }
                            Some(SyncCommand::Shutdown) => {
                                if !pending_writes.is_empty() {
                                    tracing::warn!(
                                        count = pending_writes.len(),
                                        "Shutdown: dropping writes still retained for verses whose \
                                         replica never opened this session (rows remain durable in \
                                         the local DB; their publish is lost this session)"
                                    );
                                }
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
            for (_, pump) in gossip_pumps.drain() {
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
/// Returns `false` when the async channel is closed (the sync loop is gone —
/// shutdown): the unsent command is unrecoverable by design (its receiver no
/// longer exists) and is dropped. Deliberately NOT
/// `Result<(), SyncCommand>` — the command payload would make the `Err`
/// variant huge (`SubmitComputeTask` carries a full `DistributedQueryCall`)
/// and no caller ever consumed it anyway.
fn cmd_rx_bridge_send(tx: &tokio::sync::mpsc::Sender<SyncCommand>, cmd: SyncCommand) -> bool {
    tx.blocking_send(cmd).is_ok()
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
    fabric: &mut VerseFabric,
    verse_id: &str,
    change: &RowChange,
    local_did: &str,
    evt_tx: &SyncEventSender,
    db_cmd_tx: &Option<crossbeam::channel::Sender<DbCommand>>,
) {
    // Feed the fabric FIRST — before the own-author filter — because our own
    // snapshot rows (replayed by seed_reconciliation) are exactly how the
    // fabric re-learns settings, peer declarations, and the shard ledger
    // after a restart. Sync-plane rows (`__shards`/`__peers`) are consumed
    // here and never forwarded to the DB thread.
    fabric.note_entry(change);
    if change.table == SHARD_TABLE || change.table == PEER_DECL_TABLE {
        return;
    }

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

    // Transfer routing, receive side (A13): a timeseries row for a shard this
    // peer does not host is not applied locally. `mirror` retains everything
    // (the pre-F6 behavior); an unknown shard (ledger not converged yet) also
    // retains — over-retention is harmless under union semantics, while a
    // wrongly-dropped hosted row would be data loss.
    if change.table == "iot_reading" && !change.is_tombstone && !change.data.is_empty() {
        if let Ok(row) = serde_json::from_slice::<serde_json::Value>(&change.data) {
            if let Some(shard) = ShardId::of_reading_row(&row, fabric.settings.bucket_width_ms) {
                let key = shard.key();
                if fabric.retention_for(&key, local_did) == Retention::Skip {
                    tracing::debug!(
                        verse_id,
                        shard = %key,
                        record_id = %change.record_id,
                        "Timeseries row not retained — another peer hosts this shard (F6 retention)"
                    );
                    return;
                }
            }
        }
    }

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
        send_sync_event(evt_tx, SyncEvent::BlobReady { hash: *hash });
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

/// Replay a freshly-opened replica's current entries through the inbound
/// apply path (F4 startup reconciliation, made deadlock-free by F20/M1
/// finding 1).
///
/// The A3 inbound role gate denies a row whose author does not resolve to
/// Editor+ at the verse scope — including rows that arrive before the verse
/// manifest or an explicit role has converged locally. Such a row stays in
/// the doc store but never re-fires as an `InsertRemote`, so without this pass
/// it could never converge. Replaying the snapshot gives it a second chance
/// whenever the replica is opened — at relay/GUI startup and on every
/// re-open.
///
/// **Self-drain capacity rule (F20 finding 1):** entries are applied
/// DIRECTLY through [`handle_inbound_row_change`] — the same handler the
/// command loop's inbound branch calls — never by awaiting a send into the
/// aggregated inbound stream. That stream (capacity 256) is drained ONLY by
/// the select loop, and this pass runs inline inside the loop's
/// `OpenVerseReplica` arm: awaiting capacity there parked the 257th send
/// forever on any replica whose doc holds more entries than the channel
/// capacity (every real IoT verse — readings are per-row doc keys), wedging
/// the whole sync thread (no further commands, not even `Shutdown`).
/// `handle_inbound_row_change` is fully synchronous and `try_send`s to the
/// DB thread with drop-and-count backpressure, so an unbounded snapshot
/// drains without ever awaiting anything. **A handler awaited inline in the
/// select loop must never await a send into a stream only that select loop
/// drains.**
///
/// Best-effort: a snapshot failure is a loud warn, never fatal.
async fn seed_reconciliation(
    replicator: &dyn VerseReplicator,
    verse_id: &str,
    local_did: &str,
    evt_tx: &SyncEventSender,
    db_cmd_tx: &Option<crossbeam::channel::Sender<DbCommand>>,
    fabric: &mut VerseFabric,
) {
    let entries = match replicator.snapshot().await {
        Ok(entries) => entries,
        Err(e) => {
            tracing::warn!(verse_id, "Reconciliation snapshot failed: {e}");
            return;
        }
    };
    if entries.is_empty() {
        return;
    }
    let total = entries.len();
    let mut forwarded = 0usize;
    for change in entries {
        // The literal same inbound apply path the live pump's rows take:
        // own-author rows filtered here (loop prevention — the docs author
        // is the endpoint identity, so our own snapshot entries carry our
        // DID), survivors emitted + `try_send`'d to the DB thread. The
        // fabric is fed for every row (settings/peers/ledger re-learn) and
        // the retention filter drops unhosted timeseries rows.
        handle_inbound_row_change(fabric, verse_id, &change, local_did, evt_tx, db_cmd_tx);
        forwarded += 1;
    }
    tracing::info!(
        verse_id,
        entries = total,
        forwarded,
        "Reconciliation: replayed replica entries through the inbound apply path"
    );
}

/// Handle [`SyncCommand::OpenVerseReplica`].
///
/// Builds the per-verse replica on the sync thread's transport: the real
/// [`IrohDocsReplicator`] (with the namespace secret the capability is
/// imported on the real stack, without it the namespace opens read-only: a
/// previously imported doc, by the stored id when it is the aligned
/// Ed25519 form, else by the manifest-scan fallback for legacy BLAKE3
/// ids; F20 finding 3) or, when a virtual transport is installed, the
/// factory's sim replica (F8/A19) over the same trait contract. A
/// doc-backed replica then
/// joins the live sync swarm (dialing `peers`) and spawns its per-replica
/// inbound event pump, whose `RowChange`s are forwarded into `inbound_tx` —
/// the aggregated stream the command loop selects on (A2/A4). If a replica
/// is already open for this verse, it (and its pump) is closed first.
///
/// **Degradation contract (F20 finding 4):** the mock fallback is
/// **offline-only** (`is_available() == false`). On an ONLINE stack a
/// document-open failure leaves a loud, observable NON-replicating replica:
/// the error is recorded on the replicator (writes warn and fail,
/// `subscribe`/`snapshot` error), an `error!` log + `SyncEvent::
/// ReplicaOpenFailed` fires, and the open banner reports the failure —
/// never a usable in-memory success path that would publish nothing while
/// looking healthy.
#[allow(clippy::too_many_arguments)]
async fn handle_open_verse_replica(
    replicas: &mut HashMap<String, Box<dyn VerseReplicator>>,
    inbound_pumps: &mut HashMap<String, tokio::task::AbortHandle>,
    inbound_tx: tokio::sync::mpsc::Sender<(String, RowChange)>,
    transport: &ReplicaTransport,
    verse_id: &str,
    namespace_id: &str,
    namespace_secret: Option<String>,
    peers: &[iroh::NodeAddr],
    local_did: &str,
    evt_tx: &SyncEventSender,
    db_cmd_tx: &Option<crossbeam::channel::Sender<DbCommand>>,
    blob_store: &BlobStoreHandle,
    pending: &mut PendingWrites,
    fabric: &mut VerseFabric,
    local_declaration: PeerDeclaration,
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

    let secret = namespace_secret.clone().unwrap_or_default();
    let replicator = match transport {
        ReplicaTransport::Iroh(holder) => AnyReplicator::Iroh(Box::new(IrohDocsReplicator::new(
            verse_id.to_string(),
            namespace_id.to_string(),
            secret,
            local_did.to_string(),
            holder.clone(),
        ))),
        ReplicaTransport::Virtual(factory) => AnyReplicator::Virtual(factory.open_replica(
            verse_id,
            namespace_id,
            namespace_secret.clone(),
            local_did,
        )),
    };

    // Open the document for this namespace. Offline stacks return `Ok(())`
    // with no doc (the sanctioned mock stays). An error while ONLINE is
    // recorded as a loud non-replicating replica state — never a mock
    // install (F20 finding 4).
    match replicator.open_document().await {
        Ok(()) => {
            if replicator.is_doc_backed() {
                // Join the live sync swarm. `start_sync` runs even with no
                // peers: the dialed side must have its sync task running to
                // serve entries to peers that dial *us* (A2), and the
                // bootstrap set is exactly the peers we dial out to (A9).
                if let Err(e) = replicator.start_sync(peers.to_vec()).await {
                    tracing::warn!(verse_id, "Replica start_sync failed: {e}");
                }
            }
        }
        Err(e) => {
            if transport.is_available() {
                // ONLINE + failed open: loud, observable, non-replicating.
                replicator.mark_open_failed(format!("{e}"));
                tracing::error!(
                    verse_id,
                    "Verse replica open FAILED on the online P2P stack — \
                     replica is NOT replicating (writes will fail loudly): {e}"
                );
                send_sync_event(
                    evt_tx,
                    SyncEvent::ReplicaOpenFailed {
                        verse_id: verse_id.to_string(),
                        reason: format!("{e}"),
                    },
                );
            } else {
                // Offline: the sanctioned in-memory mock fallback.
                tracing::warn!(
                    verse_id,
                    "Verse replica document open failed while the stack is offline — mock fallback: {e}"
                );
            }
        }
    }

    // Per-replica inbound event pump (A2/A4): every inbound RowChange is
    // forwarded into the aggregated stream the command loop selects on.
    // Skipped entirely for a failed online open — there is no document to
    // receive events from.
    if replicator.open_error().is_none() {
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
    }

    // F4 startup reconciliation (deadlock-free shape — see
    // seed_reconciliation's doc comment): replay the replica's current
    // entries through the SAME inbound apply path, applied directly. A row
    // the A3 role gate denied while a role or the verse manifest had not
    // yet converged never re-fires as an `InsertRemote`, so it would be
    // stranded in the doc store forever; this pass gives it a second
    // chance on every replica open. Own-author rows are filtered
    // downstream by the inbound handler like any other row. Skipped for a
    // failed online open (no document to snapshot).
    if replicator.open_error().is_none() {
        seed_reconciliation(&replicator, verse_id, local_did, evt_tx, db_cmd_tx, fabric).await;
    }

    let open_failed_reason = replicator.open_error();
    let open_failed = open_failed_reason.is_some();
    replicas.insert(verse_id.to_string(), Box::new(replicator));

    // M2/F6: declare the local peer's shard-hosting capacity/seeder role and
    // publish it as `__peers/{local_did}` so peers' placement plans see it.
    // Skipped on a failed open (nothing publishes from a non-replicating
    // replica — F20 finding 4).
    if !open_failed {
        fabric.note_local_declaration(local_did, local_declaration);
        handle_publish_peer_declaration(replicas, fabric, local_did, verse_id).await;
    }

    // Phase F: compute gossip topic for this verse. The banner reports the
    // replica's REAL state — never "online" for a failed open (F20 finding 4).
    let topic_hash = verse_peers::verse_gossip_topic(verse_id);
    if let Some(reason) = open_failed_reason {
        tracing::error!(
            verse_id,
            namespace_id,
            reason = %reason,
            peers = peers.len(),
            gossip_topic = %hex::encode(topic_hash),
            "Opened verse replica — open FAILED, NOT replicating"
        );
    } else {
        tracing::info!(
            verse_id,
            namespace_id,
            peers = peers.len(),
            gossip_topic = %hex::encode(topic_hash),
            "Opened verse replica — P2P stack {}",
            transport.describe()
        );
    }

    // F21/M2 (the verse-manifest open race): republish writes that arrived
    // while this verse's replica was not open. The canonical case is the
    // verse manifest itself — the DB handler emits it before the host's
    // `VerseCreated`/navigation open reaches this thread, so pre-fix the
    // FIRST manifest row warn-and-dropped and never entered the doc (a row
    // `seed_reconciliation` cannot heal), permanently starving fresh peers
    // of the manifest. Flushed in FIFO order through the SAME write path a
    // live write takes, so publish/conflict semantics are identical. Never
    // flushed on a failed open: a loudly non-replicating replica publishes
    // nothing (F20 finding 4) — the writes stay retained for a later
    // successful (re-)open.
    if !open_failed {
        let queued = pending.take(verse_id);
        if !queued.is_empty() {
            tracing::info!(
                verse_id,
                count = queued.len(),
                "Replica open completed — republishing writes retained from before the open"
            );
            for write in queued {
                handle_write_row_entry(
                    replicas,
                    pending,
                    fabric,
                    blob_store,
                    local_did,
                    verse_id,
                    &write.table,
                    &write.record_id,
                    &write.content_hash,
                )
                .await;
            }
        }
    }
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
///
/// A write for a verse whose replica is not open yet is RETAINED in
/// `pending` (F21/M2 — the verse-manifest open race; see AGENTS.md
/// §pending-writes) and republished after the open, never warn-and-dropped:
/// the verse manifest row is emitted exactly once at creation and can never
/// be re-emitted, so dropping it would permanently starve fresh peers of the
/// manifest.
#[allow(clippy::too_many_arguments)]
async fn handle_write_row_entry(
    replicas: &HashMap<String, Box<dyn VerseReplicator>>,
    pending: &mut PendingWrites,
    fabric: &mut VerseFabric,
    blob_store: &BlobStoreHandle,
    author_did: &str,
    verse_id: &str,
    table: &str,
    record_id: &str,
    content_hash: &fe_runtime::blob_store::BlobHash,
) {
    let Some(repl) = replicas.get(verse_id) else {
        pending.push(
            verse_id,
            PendingWrite {
                table: table.to_string(),
                record_id: record_id.to_string(),
                content_hash: *content_hash,
            },
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

    // M2/F6 fabric bookkeeping on the OUTBOUND path (a local, already-admitted
    // write being published): learn the verse's ts_* settings from its own
    // manifest, and on the first sight of a timeseries row plan the shard's
    // host set and publish its ledger row.
    let mut ledger_publish: Option<(String, Vec<u8>)> = None;
    if let Ok(row) = serde_json::from_slice::<serde_json::Value>(&data) {
        match table {
            "verse" => {
                fabric.note_verse_row(&row);
            }
            "iot_reading" => {
                if let Some(shard) = ShardId::of_reading_row(&row, fabric.settings.bucket_width_ms)
                {
                    if let Some(ledger_bytes) = fabric.ensure_shard(&shard, data.len(), author_did)
                    {
                        ledger_publish = Some((shard.key(), ledger_bytes));
                    }
                }
            }
            _ => {}
        }
    }

    // Publish the shard ledger row BEFORE the reading so a receiving peer
    // learns the host set (its retention decision) before the row lands. The
    // route itself is enforced on the receive side: the doc transport
    // physically reaches every subscriber, and each peer's retention
    // decision applies only the shards it hosts.
    if let Some((shard_key, ledger_bytes)) = ledger_publish {
        if let Err(e) = repl.write_row(SHARD_TABLE, &shard_key, &ledger_bytes).await {
            tracing::warn!(
                verse_id,
                shard = %shard_key,
                "Shard ledger publish failed: {e}"
            );
        }
    }

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

/// Publish the local peer's `__peers/{local_did}` declaration row (M2/F6).
///
/// Called on replica open and on every [`SyncCommand::SetShardDeclaration`]
/// for each open verse, so peers' placement plans converge on our capacity
/// and seeder role.
async fn handle_publish_peer_declaration(
    replicas: &HashMap<String, Box<dyn VerseReplicator>>,
    fabric: &VerseFabric,
    local_did: &str,
    verse_id: &str,
) {
    let Some(bytes) = fabric.local_peer_row(local_did) else {
        return;
    };
    let Some(repl) = replicas.get(verse_id) else {
        return;
    };
    if let Err(e) = repl.write_row(PEER_DECL_TABLE, local_did, &bytes).await {
        tracing::warn!(verse_id, "Peer declaration publish failed: {e}");
    }
}

/// Handle [`SyncCommand::SetShardDeclaration`] (M2/F6): update the local
/// declaration in every open verse's fabric and republish it.
async fn handle_set_shard_declaration(
    replicas: &HashMap<String, Box<dyn VerseReplicator>>,
    fabrics: &mut HashMap<String, VerseFabric>,
    local_did: &str,
    declaration: PeerDeclaration,
) {
    for (verse_id, fabric) in fabrics.iter_mut() {
        fabric.note_local_declaration(local_did, declaration);
        if replicas.contains_key(verse_id) {
            handle_publish_peer_declaration(replicas, fabric, local_did, verse_id).await;
        }
    }
    tracing::info!(
        capacity_bytes = ?declaration.capacity_bytes,
        seeder = declaration.seeder,
        "Shard declaration updated"
    );
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
/// The topic is used for allowed verse-scoped gossip: tileset announcements
/// and (F7) the distributed-query request/response traffic.
pub(crate) fn derive_gossip_topic(verse_id: &str) -> String {
    format!("verse:{}", verse_id)
}

/// Subscribe to a verse gossip topic (M2/F7 rework: split the handle).
///
/// The `GossipTopic` handle is split on arrival: the sender half is kept in
/// `gossip_senders` for broadcasts (tileset ads, distributed-query
/// envelopes), the receiver half drains in a spawned pump task that
/// forwards every gossip message into the aggregated inbound stream the
/// command loop's select consumes. This is the inbound route F7 rides —
/// before it, nothing ever polled a topic's event stream.
///
/// F9/A21 virtual branch: with a `VirtualTransportFactory` installed there
/// is no iroh gossip stack — the topic comes from the factory's virtual
/// gossip plane (the sim hub), which carries the SAME scripted
/// latency/partition/churn as the doc plane. The sender half is a
/// `TopicSender::Virtual`; the inbound half is a channel receiver the same
/// pump shape forwards. A factory without a gossip plane (`join_gossip_topic`
/// default) leaves the verse honestly topic-less, as before F9.
#[allow(clippy::too_many_arguments)]
fn subscribe_to_verse_gossip_topic(
    endpoint: Option<&SyncEndpoint>,
    gossip_host: &Option<Gossip>,
    virtual_transport: &Option<Arc<dyn VirtualTransportFactory>>,
    local_did: &str,
    local_node: iroh::NodeId,
    gossip_senders: &mut HashMap<String, TopicSender>,
    gossip_pumps: &mut HashMap<String, tokio::task::AbortHandle>,
    gossip_inbound_tx: tokio::sync::mpsc::Sender<GossipIncoming>,
    verse_id: &str,
    peers: &[iroh::NodeAddr],
) {
    let topic_key = derive_gossip_topic(verse_id);

    // Already subscribed? (checked before either branch — one topic per verse)
    if gossip_senders.contains_key(&topic_key) {
        tracing::debug!(verse_id, "Already subscribed to gossip topic");
        return;
    }

    // F9/A21: the sim lab's virtual gossip plane. Membership is scripted in
    // the hub, so there is nothing to dial and no bootstrap race.
    if let Some(factory) = virtual_transport {
        match factory.join_gossip_topic(&topic_key, local_did, local_node) {
            Some(topic) => match topic.take_inbound() {
                Some(rx) => {
                    let pump = tokio::spawn(pump_virtual_gossip_topic(
                        rx,
                        verse_id.to_string(),
                        gossip_inbound_tx,
                    ));
                    gossip_pumps.insert(topic_key.clone(), pump.abort_handle());
                    gossip_senders.insert(topic_key, TopicSender::Virtual(topic));
                    tracing::debug!(verse_id, "Subscribed to virtual gossip topic");
                }
                None => {
                    tracing::warn!(
                        verse_id,
                        "Virtual gossip topic handed out no inbound stream — verse has \
                             no compute plane this session"
                    );
                }
            },
            None => {
                tracing::debug!(
                    verse_id,
                    "Virtual transport has no gossip plane — verse topic absent (honest)"
                );
            }
        }
        return;
    }

    let Some(ref gossip) = gossip_host else {
        tracing::debug!(verse_id, "No gossip, skipping topic subscription");
        return;
    };

    // The gossip join below dials by bare NodeId and is ONE-SHOT: it can
    // only resolve an address the endpoint's address book already knows.
    // The docs sync (`start_sync`) dials these same peers only in the
    // background, so without this the join races the docs dial that would
    // first teach the endpoint the peer's address — a lost race leaves the
    // topic with no neighbor for the whole session (every compute broadcast
    // reaches nobody). Register every bootstrap peer's address up front so
    // the join always resolves.
    if let Some(ep) = endpoint {
        for peer in peers {
            if let Err(e) = ep.inner().add_node_addr(peer.clone()) {
                tracing::warn!(
                    verse_id,
                    "add_node_addr for a gossip bootstrap peer failed: {e}"
                );
            }
        }
    }

    // Bootstrap the topic with the verse's known peers (M2/F7). Without this
    // the topic has zero neighbors and a compute request broadcast can only
    // ever reach the local host — the fan-out transport needs real
    // neighbors, and the same peer set the replica dials for doc sync is the
    // one to gossip with. Empty on a fresh host (the dialed side joins via
    // its own bootstrap set).
    let bootstrap: Vec<iroh::NodeId> = peers.iter().map(|p| p.node_id).collect();

    match gossip.subscribe(gossip_topic_id(&topic_key), bootstrap) {
        Ok(handle) => {
            tracing::debug!(verse_id, "Subscribed to verse gossip topic");
            let (sender, receiver) = handle.split();
            let pump = tokio::spawn(pump_gossip_topic(
                receiver,
                verse_id.to_string(),
                gossip_inbound_tx,
            ));
            gossip_pumps.insert(topic_key.clone(), pump.abort_handle());
            gossip_senders.insert(topic_key, TopicSender::Real(sender));
        }
        Err(e) => {
            tracing::warn!(verse_id, "Failed to subscribe to gossip topic: {e}");
        }
    }
}

/// Drain a virtual gossip topic's inbound stream (F9/A21), forwarding each
/// message as a [`GossipIncoming`] exactly like [`pump_gossip_topic`] does
/// for the real stack — the command loop cannot tell the planes apart.
/// Hub deliveries are always direct (0-hop), so the F23 sender-identity
/// gate applies to sim envelopes exactly as to direct iroh deliveries.
async fn pump_virtual_gossip_topic(
    mut receiver: tokio::sync::mpsc::Receiver<crate::virtual_transport::VirtualGossipMessage>,
    verse_id: String,
    inbound_tx: tokio::sync::mpsc::Sender<GossipIncoming>,
) {
    while let Some(message) = receiver.recv().await {
        let incoming = GossipIncoming {
            verse_id: verse_id.clone(),
            from: message.from,
            direct: message.direct,
            content: message.content,
        };
        if inbound_tx.send(incoming).await.is_err() {
            break; // sync loop gone — shutdown
        }
    }
}

/// Drain one topic's gossip event stream, forwarding messages to the sync
/// loop's aggregated inbound channel. Membership events (Joined /
/// NeighborUp / NeighborDown) are intentionally not consumed — nothing acts
/// on them today. A pump is a separate task, so the awaiting send below is
/// backpressure, not a self-drain (§self-drain applies to the select loop
/// itself).
async fn pump_gossip_topic(
    mut receiver: GossipReceiver,
    verse_id: String,
    inbound_tx: tokio::sync::mpsc::Sender<GossipIncoming>,
) {
    use futures_lite::StreamExt;
    loop {
        match receiver.next().await {
            Some(Ok(GossipTopicEvent::Gossip(GossipEvent::Received(message)))) => {
                let incoming = GossipIncoming {
                    verse_id: verse_id.clone(),
                    from: message.delivered_from,
                    // F23 identity check input: only a DIRECT delivery's
                    // `delivered_from` is the envelope author — the scope
                    // says whether the message took 0 hops from its
                    // publisher (iroh-gossip `DeliveryScope::is_direct`).
                    direct: message.scope.is_direct(),
                    content: message.content,
                };
                if inbound_tx.send(incoming).await.is_err() {
                    break; // sync loop gone — shutdown
                }
            }
            Some(Ok(GossipTopicEvent::Lagged)) => {
                tracing::warn!(verse_id, "gossip topic receiver lagged — messages missed");
            }
            Some(Ok(_)) => {} // Joined / NeighborUp / NeighborDown — not consumed
            Some(Err(e)) => {
                tracing::warn!(verse_id, "gossip topic stream error: {e}");
            }
            None => break, // topic closed
        }
    }
}

/// Unsubscribe from a verse gossip topic.
///
/// This is called when closing a verse replica: drop the sender half and
/// abort the receiver pump — both halves gone leaves the topic in
/// iroh-gossip 0.35.
fn unsubscribe_from_verse_gossip_topic(
    gossip_senders: &mut HashMap<String, TopicSender>,
    gossip_pumps: &mut HashMap<String, tokio::task::AbortHandle>,
    verse_id: &str,
) {
    let topic_key = derive_gossip_topic(verse_id);
    if let Some(pump) = gossip_pumps.remove(&topic_key) {
        pump.abort();
    }
    if gossip_senders.remove(&topic_key).is_some() {
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
    gossip_senders: &HashMap<String, TopicSender>,
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

    // Get the verse topic sender
    let topic_key = derive_gossip_topic(verse_id);
    let Some(sender) = gossip_senders.get(&topic_key) else {
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

        if let Err(e) = sender.broadcast(payload.into()).await {
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
    send_sync_event(
        evt_tx,
        SyncEvent::TilesetMetaReceived {
            peer_id: peer_id.to_string(),
            tileset_id: tileset_id.to_string(),
            meta_json: meta_json.to_string(),
            total_chunks: 0,
            approx_size_bytes: 0,
        },
    );
}

/// Handle [`SyncCommand::RequestChunk`].
///
/// Requests a chunk from a peer via iroh-blobs.
/// This is a stub - full implementation would use actual blob transfer.
fn handle_request_chunk(peer_id: &str, tileset_id: &str, chunk_seq: u32, evt_tx: &SyncEventSender) {
    tracing::debug!(peer_id = %peer_id, tileset_id = %tileset_id, chunk_seq, "RequestChunk (stub)");

    // Stub: emit failure since we can't actually transfer
    send_sync_event(
        evt_tx,
        SyncEvent::ChunkFailed {
            tileset_id: tileset_id.to_string(),
            chunk_seq,
            reason: "chunk transfer not implemented in stub".to_string(),
        },
    );
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
    use crate::replicator::MockVerseReplicator;
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

    /// A test double for the F8/A19 seam: a virtual replica wrapping the
    /// in-memory [`MockVerseReplicator`] (the same contract the sim lab's
    /// `SimVerseReplicator` implements over the shared hub).
    struct FakeVirtualReplica {
        inner: Arc<MockVerseReplicator>,
        open_failed: std::sync::Mutex<Option<String>>,
    }

    impl VerseReplicator for FakeVirtualReplica {
        fn write_row(
            &self,
            table: &str,
            record_id: &str,
            data: &[u8],
        ) -> ReplicatorFuture<'_, anyhow::Result<()>> {
            self.inner.write_row(table, record_id, data)
        }

        fn subscribe(
            &self,
        ) -> ReplicatorFuture<'_, anyhow::Result<tokio::sync::mpsc::Receiver<RowChange>>> {
            self.inner.subscribe()
        }

        fn snapshot(&self) -> ReplicatorFuture<'_, anyhow::Result<Vec<RowChange>>> {
            self.inner.snapshot()
        }

        fn close(&self) -> ReplicatorFuture<'_, anyhow::Result<()>> {
            self.inner.close()
        }
    }

    impl crate::virtual_transport::VirtualReplica for FakeVirtualReplica {
        fn open_document(&self) -> ReplicatorFuture<'_, anyhow::Result<()>> {
            Box::pin(async { Ok(()) })
        }

        fn is_doc_backed(&self) -> bool {
            true
        }

        fn start_sync(
            &self,
            _peers: Vec<iroh::NodeAddr>,
        ) -> ReplicatorFuture<'_, anyhow::Result<()>> {
            Box::pin(async { Ok(()) })
        }

        fn mark_open_failed(&self, reason: String) {
            *self.open_failed.lock().unwrap() = Some(reason);
        }

        fn open_error(&self) -> Option<String> {
            self.open_failed.lock().unwrap().clone()
        }
    }

    struct FakeTransportFactory {
        replica: Arc<MockVerseReplicator>,
        opened: std::sync::atomic::AtomicUsize,
    }

    impl crate::virtual_transport::VirtualTransportFactory for FakeTransportFactory {
        fn open_replica(
            &self,
            _verse_id: &str,
            _namespace_id: &str,
            _namespace_secret: Option<String>,
            _local_did: &str,
        ) -> Box<dyn crate::virtual_transport::VirtualReplica> {
            self.opened
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Box::new(FakeVirtualReplica {
                inner: self.replica.clone(),
                open_failed: std::sync::Mutex::new(None),
            })
        }

        fn is_available(&self) -> bool {
            true
        }

        fn describe(&self) -> &'static str {
            "virtual (test)"
        }
    }

    /// A19 (test): a sync thread on a virtual transport opens replicas from
    /// the factory, runs the SAME open/write/close lifecycle over the real
    /// command loop, and binds **no iroh endpoint** (the "no real network"
    /// clause — pinned by `bound_endpoint_count` staying put across the run).
    #[test]
    fn virtual_transport_runs_the_full_replica_lifecycle_without_binding_an_endpoint() {
        let _env_guard = SYNC_THREAD_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let p2p_dir = hermetic_p2p_env();
        let endpoints_before = crate::endpoint::bound_endpoint_count();

        // The write leg reads the blob from disk, so the store must be an
        // FsBlobStore (MockBlobStore has no paths — handle_write_row_entry
        // would warn-and-drop).
        let blob_dir = p2p_dir.path().join("blobs");
        let store: BlobStoreHandle =
            Arc::new(crate::blob_store::FsBlobStore::new(blob_dir).expect("fs blob store"));
        let secret = iroh::SecretKey::from_bytes(&[96u8; 32]);
        let (cmd_tx, cmd_rx) = crossbeam::channel::bounded(8);
        let (evt_tx, evt_rx) = crossbeam::channel::bounded(16);

        let factory = Arc::new(FakeTransportFactory {
            replica: Arc::new(MockVerseReplicator::new("did:key:virtual")),
            opened: std::sync::atomic::AtomicUsize::new(0),
        });

        let handle = spawn_sync_thread_with_transport(
            secret,
            store.clone(),
            cmd_rx,
            evt_tx,
            "did:key:virtual".to_string(),
            None,
            None,
            Some(factory.clone()),
        );

        // Virtual startup: Started reports online (the transport is live)
        // with no dialable address, and health is the Disabled fixed point.
        let started = evt_rx.recv_timeout(std::time::Duration::from_secs(15));
        assert!(
            matches!(
                started,
                Ok(SyncEvent::Started {
                    online: true,
                    node_addr: None,
                })
            ),
            "expected virtual Started, got {started:?}"
        );
        let health = evt_rx.recv_timeout(std::time::Duration::from_secs(2));
        assert!(
            matches!(
                health,
                Ok(SyncEvent::RelayHealthChanged {
                    health: RelayHealth::Disabled,
                })
            ),
            "expected Disabled relay health in virtual mode, got {health:?}"
        );

        // The open runs through the factory — the same command loop, the
        // same handle_open_verse_replica open sequence.
        cmd_tx
            .send(SyncCommand::OpenVerseReplica {
                verse_id: "v-sim".to_string(),
                namespace_id: "0".repeat(64),
                namespace_secret: None,
                bootstrap_peers: Vec::new(),
            })
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert_eq!(
            factory.opened.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "the factory built exactly one replica"
        );

        // A write flows through the real write path (blob store → row
        // entry → factory replica).
        let bytes = br#"{"verse_id":"v-sim","name":"Sim Verse"}"#;
        let hash = store.add_blob(bytes).expect("blob added");
        cmd_tx
            .send(SyncCommand::WriteRowEntry {
                verse_id: "v-sim".to_string(),
                table: "verse".to_string(),
                record_id: "v-sim".to_string(),
                content_hash: hash,
            })
            .unwrap();
        let wrote = std::time::Instant::now();
        while !factory.replica.has_entry("verse", "v-sim") {
            assert!(
                wrote.elapsed() < std::time::Duration::from_secs(5),
                "the write never reached the virtual replica"
            );
            std::thread::sleep(std::time::Duration::from_millis(50));
        }

        cmd_tx
            .send(SyncCommand::CloseVerseReplica {
                verse_id: "v-sim".to_string(),
            })
            .unwrap();
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

        // The "no real network" clause, pinned: no endpoint was bound.
        assert_eq!(
            crate::endpoint::bound_endpoint_count(),
            endpoints_before,
            "a virtual-transport sync thread must not bind an iroh endpoint"
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

    /// A deterministic test node identity (same construction the
    /// distributed-query tests use).
    fn test_node_id(seed: u8) -> iroh::NodeId {
        iroh::SecretKey::from_bytes(&[seed; 32]).public()
    }

    #[test]
    fn subscribe_to_verse_gossip_topic_no_host_is_noop() {
        let gossip_host: Option<Gossip> = None;
        let no_virtual: Option<Arc<dyn VirtualTransportFactory>> = None;
        let mut gossip_senders: HashMap<String, TopicSender> = HashMap::new();
        let mut gossip_pumps: HashMap<String, tokio::task::AbortHandle> = HashMap::new();
        let (tx, _rx) = tokio::sync::mpsc::channel::<GossipIncoming>(8);

        // Should not panic, just skip
        subscribe_to_verse_gossip_topic(
            None,
            &gossip_host,
            &no_virtual,
            "did:local",
            test_node_id(3),
            &mut gossip_senders,
            &mut gossip_pumps,
            tx,
            "verse-test",
            &[],
        );

        // No topics should be created without a host
        assert!(gossip_senders.is_empty(), "no topics without gossip host");
        assert!(gossip_pumps.is_empty(), "no pumps without gossip host");
    }

    /// F9/A21 seam: a virtual transport WITHOUT a gossip plane (the default
    /// `join_gossip_topic`) leaves the verse honestly topic-less; with one,
    /// the virtual sender + pump land in the maps.
    #[test]
    fn subscribe_to_verse_gossip_topic_virtual_plane() {
        use crate::virtual_transport::{
            VirtualGossipMessage, VirtualGossipTopic, VirtualTransportFactory,
        };

        /// A factory with no gossip plane (the pre-F9 default shape).
        struct NoGossipFactory;
        impl VirtualTransportFactory for NoGossipFactory {
            fn open_replica(
                &self,
                _verse_id: &str,
                _namespace_id: &str,
                _namespace_secret: Option<String>,
                _local_did: &str,
            ) -> Box<dyn crate::virtual_transport::VirtualReplica> {
                unimplemented!("the gossip seam test never opens a replica")
            }
            fn is_available(&self) -> bool {
                true
            }
            fn describe(&self) -> &'static str {
                "virtual (no gossip)"
            }
        }

        /// A factory whose gossip plane is a loopback topic: broadcasts land
        /// on the subscriber's own inbound stream — a test double that
        /// self-delivers by its own construction (the sim hub does not).
        struct LoopbackFactory;
        struct LoopbackTopic {
            tx: tokio::sync::mpsc::Sender<VirtualGossipMessage>,
            rx: std::sync::Mutex<Option<tokio::sync::mpsc::Receiver<VirtualGossipMessage>>>,
            node: iroh::NodeId,
        }
        impl VirtualGossipTopic for LoopbackTopic {
            fn broadcast(&self, content: bytes::Bytes) -> Result<(), String> {
                self.tx
                    .try_send(VirtualGossipMessage {
                        from: self.node,
                        direct: true,
                        content,
                    })
                    .map_err(|e| format!("loopback topic send failed: {e}"))
            }
            fn take_inbound(&self) -> Option<tokio::sync::mpsc::Receiver<VirtualGossipMessage>> {
                self.rx.lock().unwrap_or_else(|e| e.into_inner()).take()
            }
        }
        impl VirtualTransportFactory for LoopbackFactory {
            fn open_replica(
                &self,
                _verse_id: &str,
                _namespace_id: &str,
                _namespace_secret: Option<String>,
                _local_did: &str,
            ) -> Box<dyn crate::virtual_transport::VirtualReplica> {
                unimplemented!("the gossip seam test never opens a replica")
            }
            fn is_available(&self) -> bool {
                true
            }
            fn describe(&self) -> &'static str {
                "virtual (loopback gossip)"
            }
            fn join_gossip_topic(
                &self,
                _topic_key: &str,
                _local_did: &str,
                local_node: iroh::NodeId,
            ) -> Option<Arc<dyn VirtualGossipTopic>> {
                let (tx, rx) = tokio::sync::mpsc::channel(16);
                Some(Arc::new(LoopbackTopic {
                    tx,
                    rx: std::sync::Mutex::new(Some(rx)),
                    node: local_node,
                }))
            }
        }

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime");
        rt.block_on(async {
            let gossip_host: Option<Gossip> = None;
            let local_node = test_node_id(3);

            // (a) No gossip plane: honest absence.
            let no_plane: Option<Arc<dyn VirtualTransportFactory>> =
                Some(Arc::new(NoGossipFactory));
            let mut senders: HashMap<String, TopicSender> = HashMap::new();
            let mut pumps: HashMap<String, tokio::task::AbortHandle> = HashMap::new();
            let (tx, _rx) = tokio::sync::mpsc::channel::<GossipIncoming>(8);
            subscribe_to_verse_gossip_topic(
                None,
                &gossip_host,
                &no_plane,
                "did:local",
                local_node,
                &mut senders,
                &mut pumps,
                tx,
                "verse-test",
                &[],
            );
            assert!(
                senders.is_empty(),
                "a factory without a gossip plane leaves the verse topic-less"
            );

            // (b) With a gossip plane: sender + pump registered, a broadcast
            // arrives on the aggregated inbound stream as a direct delivery
            // from our own node identity (the loopback double's construction).
            let plane: Option<Arc<dyn VirtualTransportFactory>> = Some(Arc::new(LoopbackFactory));
            let (tx, mut inbound_rx) = tokio::sync::mpsc::channel::<GossipIncoming>(8);
            subscribe_to_verse_gossip_topic(
                None,
                &gossip_host,
                &plane,
                "did:local",
                local_node,
                &mut senders,
                &mut pumps,
                tx.clone(),
                "verse-test",
                &[],
            );
            let sender = senders
                .get(&derive_gossip_topic("verse-test"))
                .expect("virtual sender registered");
            sender
                .broadcast(bytes::Bytes::from_static(b"{\"type\":\"x\"}"))
                .await
                .expect("virtual broadcast succeeds");
            let incoming =
                tokio::time::timeout(std::time::Duration::from_secs(5), inbound_rx.recv())
                    .await
                    .expect("pump forwards within the budget")
                    .expect("inbound stream open");
            assert_eq!(incoming.verse_id, "verse-test");
            assert_eq!(incoming.from, local_node);
            assert!(incoming.direct, "hub deliveries are 0-hop (direct)");
            assert_eq!(incoming.content.as_ref(), b"{\"type\":\"x\"}");

            // The maps own one subscription; re-subscribing is a no-op.
            subscribe_to_verse_gossip_topic(
                None,
                &gossip_host,
                &plane,
                "did:local",
                local_node,
                &mut senders,
                &mut pumps,
                tx.clone(),
                "verse-test",
                &[],
            );
            assert_eq!(senders.len(), 1, "re-subscribe is a no-op");
        });
    }

    #[test]
    fn unsubscribe_from_verse_gossip_topic_no_host_is_noop() {
        let mut gossip_senders: HashMap<String, TopicSender> = HashMap::new();
        let mut gossip_pumps: HashMap<String, tokio::task::AbortHandle> = HashMap::new();

        // Pre-populate (simulating prior subscription)
        // Note: Can't actually insert valid TopicId without real host,
        // but we test the cleanup path

        // Should not panic
        unsubscribe_from_verse_gossip_topic(&mut gossip_senders, &mut gossip_pumps, "verse-test");

        assert!(
            gossip_senders.is_empty(),
            "map should be empty after unsubscribe"
        );
        assert!(gossip_pumps.is_empty(), "pump map should be empty too");
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
        let gossip_senders: HashMap<String, TopicSender> = HashMap::new();

        let ads_json = r#"[{"tileset_id": "ts-001", "chunk_count": 10, "size_bytes": 1000}]"#;

        // Should not panic, just skip
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(handle_advertise_tilesets(
            &gossip_host,
            &gossip_senders,
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

        handle_inbound_row_change(
            &mut VerseFabric::default(),
            "verse-1",
            &change,
            "did:key:local",
            &evt_tx,
            &Some(db_tx),
        );

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

        handle_inbound_row_change(
            &mut VerseFabric::default(),
            "verse-9",
            &change,
            "did:key:local",
            &evt_tx,
            &Some(db_tx),
        );

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

        handle_inbound_row_change(
            &mut VerseFabric::default(),
            "verse-2",
            &change,
            "did:key:local",
            &evt_tx,
            &None,
        );

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
        handle_inbound_row_change(
            &mut VerseFabric::default(),
            "verse-3",
            &change,
            "did:key:local",
            &evt_tx,
            &Some(db_tx),
        );
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
        let blob_store: BlobStoreHandle = Arc::new(MockBlobStore::new());
        let mut pending = PendingWrites::default();
        let (evt_tx, evt_rx) = crossbeam::channel::bounded(8);
        assert!(evt_rx.try_recv().is_err(), "no events yet");

        handle_open_verse_replica(
            &mut replicas,
            &mut inbound_pumps,
            inbound_tx.clone(),
            &ReplicaTransport::Iroh(engine_holder.clone()),
            "verse-1",
            "0".repeat(64).as_str(),
            None,
            &[],
            "did:key:local",
            &evt_tx,
            &None,
            &blob_store,
            &mut pending,
            &mut VerseFabric::default(),
            PeerDeclaration::default(),
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
            &ReplicaTransport::Iroh(engine_holder),
            "verse-1",
            "0".repeat(64).as_str(),
            None,
            &[],
            "did:key:local",
            &evt_tx,
            &None,
            &blob_store,
            &mut pending,
            &mut VerseFabric::default(),
            PeerDeclaration::default(),
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
        let blob_store: BlobStoreHandle = Arc::new(MockBlobStore::new());
        let mut pending = PendingWrites::default();
        let (evt_tx, _evt_rx) = crossbeam::channel::bounded(8);

        handle_open_verse_replica(
            &mut replicas,
            &mut inbound_pumps,
            inbound_tx,
            &ReplicaTransport::Iroh(engine_holder),
            "verse-pump",
            "0".repeat(64).as_str(),
            None,
            &[],
            "did:key:local",
            &evt_tx,
            &None,
            &blob_store,
            &mut pending,
            &mut VerseFabric::default(),
            PeerDeclaration::default(),
        )
        .await;

        let repl = replicas.get("verse-pump").expect("replica present");
        repl.write_row("node", "node-1", br#"{"name":"written locally"}"#)
            .await
            .expect("mock write succeeds");

        // The pump must forward the write into the aggregated inbound stream.
        // The open also published our `__peers` declaration row (F6), so drain
        // that sync-plane row first.
        let mut node_row = None;
        for _ in 0..4 {
            let (verse_id, change) = inbound_rx
                .recv()
                .await
                .expect("pump forwarded rows into the inbound stream");
            if change.table == "node" {
                node_row = Some((verse_id, change));
                break;
            }
        }
        let (verse_id, change) = node_row.expect("pump forwarded the node row");
        assert_eq!(verse_id, "verse-pump");
        assert_eq!(change.record_id, "node-1");
        assert_eq!(change.data, br#"{"name":"written locally"}"#.to_vec());

        // The seam filters our own author (mock echo) before any DB apply.
        let (evt_tx, evt_rx) = crossbeam::channel::bounded(8);
        handle_inbound_row_change(
            &mut VerseFabric::default(),
            &verse_id,
            &change,
            "did:key:local",
            &evt_tx,
            &None,
        );
        assert!(
            evt_rx.try_recv().is_err(),
            "own-author rows are filtered at the seam"
        );

        handle_close_verse_replica(&mut replicas, &mut inbound_pumps, "verse-pump").await;
    }

    // -------------------------------------------------------------------------
    // M2/F6: timeseries fabric wiring (A13 mode-switching, A14 placement)
    // -------------------------------------------------------------------------

    /// A reading published while the verse is in `balanced` mode plans its
    /// shard across R peers and publishes a `__shards/{key}` ledger row into
    /// the verse's doc — mode honored by placement (A13/A14) at the sync-thread
    /// seam, with the settings learned from the verse manifest row itself.
    #[tokio::test]
    async fn write_row_entry_publishes_shard_ledger_per_mode() {
        use crate::sharding::ShardLedgerEntry;
        use fe_runtime::timeseries::TimeseriesMode;

        let tmp = tempfile::TempDir::new().unwrap();
        let blob_store: BlobStoreHandle =
            Arc::new(crate::FsBlobStore::new(tmp.path().join("blobs")).expect("fs blob store"));
        let mut replicas: HashMap<String, Box<dyn VerseReplicator>> = HashMap::new();
        let mut inbound_pumps: HashMap<String, tokio::task::AbortHandle> = HashMap::new();
        let (inbound_tx, _inbound_rx) = tokio::sync::mpsc::channel::<(String, RowChange)>(16);
        let engine_holder = Arc::new(IrohDocsEngineHolder::new()); // offline → mock
        let (evt_tx, _evt_rx) = crossbeam::channel::bounded(8);
        let mut pending = PendingWrites::default();

        // Two peers declared: the local one and a remote one → R=2 is reachable.
        let mut fabric = VerseFabric::default();
        fabric.note_peer_declaration("did:key:peer-b", PeerDeclaration::default());

        handle_open_verse_replica(
            &mut replicas,
            &mut inbound_pumps,
            inbound_tx,
            &ReplicaTransport::Iroh(engine_holder),
            "v-shard",
            "0".repeat(64).as_str(),
            None,
            &[],
            "did:key:local",
            &evt_tx,
            &None,
            &blob_store,
            &mut pending,
            &mut fabric,
            PeerDeclaration::default(),
        )
        .await;

        // The verse manifest carries the fabric settings (balanced, R=2).
        let verse_row = br#"{"verse_id":"v-shard","name":"Sharded","ts_mode":"balanced","ts_replication_factor":2,"ts_bucket_width_ms":86400000}"#;
        let verse_hash = blob_store.add_blob(verse_row).expect("verse blob");
        handle_write_row_entry(
            &replicas,
            &mut pending,
            &mut fabric,
            &blob_store,
            "did:key:local",
            "v-shard",
            "verse",
            "v-shard",
            &verse_hash,
        )
        .await;
        assert_eq!(
            fabric.settings.mode,
            TimeseriesMode::Balanced,
            "settings learned from the outbound verse manifest"
        );
        assert_eq!(fabric.settings.replication_factor, 2);

        // A reading maps to exactly one shard → the ledger row is published.
        let reading = br#"{"reading_id":"r-1","node_id":"a1","petal_id":"p1","recorded_at_ms":1752580800000}"#;
        let reading_hash = blob_store.add_blob(reading).expect("reading blob");
        handle_write_row_entry(
            &replicas,
            &mut pending,
            &mut fabric,
            &blob_store,
            "did:key:local",
            "v-shard",
            "iot_reading",
            "r-1",
            &reading_hash,
        )
        .await;

        let snap = replicas
            .get("v-shard")
            .expect("replica present")
            .snapshot()
            .await
            .expect("snapshot");
        let ledger_row = snap
            .iter()
            .find(|c| c.table == SHARD_TABLE)
            .expect("a shard ledger row was published to the doc");
        let entry: ShardLedgerEntry =
            serde_json::from_slice(&ledger_row.data).expect("ledger json");
        assert_eq!(entry.shard, "p1/a1/20284");
        assert_eq!(entry.hosts.len(), 2, "balanced R=2 places two hosts");
        assert_eq!(entry.mode, TimeseriesMode::Balanced);
        assert!(entry.hosts.contains(&"did:key:local".to_string()));
        assert!(entry.hosts.contains(&"did:key:peer-b".to_string()));

        // Re-publishing the same reading never re-plans (one ledger row).
        handle_write_row_entry(
            &replicas,
            &mut pending,
            &mut fabric,
            &blob_store,
            "did:key:local",
            "v-shard",
            "iot_reading",
            "r-1",
            &reading_hash,
        )
        .await;
        let snap = replicas.get("v-shard").unwrap().snapshot().await.unwrap();
        let ledger_rows = snap.iter().filter(|c| c.table == SHARD_TABLE).count();
        assert_eq!(ledger_rows, 1, "the ledger row is written once per shard");
    }

    /// A timeseries row for a shard this peer does not host is NOT applied
    /// (A13 transfer routing, receive side) — while `mirror` retains
    /// everything, and an unknown shard retains (the safe default).
    #[tokio::test]
    async fn inbound_timeseries_row_retention_follows_mode() {
        let (evt_tx, evt_rx) = crossbeam::channel::bounded(8);
        let (db_tx, db_rx) = crossbeam::channel::bounded::<DbCommand>(8);
        let reading = br#"{"reading_id":"r-1","node_id":"a1","petal_id":"p1","recorded_at_ms":1752580800000}"#;
        let change = RowChange {
            table: "iot_reading".to_string(),
            record_id: "r-1".to_string(),
            content_hash: [0u8; 32],
            author_id: "did:key:peer-b".to_string(),
            timestamp: 1,
            is_tombstone: false,
            data: reading.to_vec(),
        };

        // sharded mode with a converged ledger placing the shard on peer-b
        // only → the local peer is not a host → skip.
        let mut fabric = VerseFabric::default();
        fabric.note_verse_row(&serde_json::json!({
            "ts_mode": "sharded", "ts_bucket_width_ms": 86_400_000
        }));
        fabric.note_shard_row(&serde_json::json!({
            "shard": "p1/a1/20284", "hosts": ["did:key:peer-b"],
            "mode": "sharded", "replication_factor": 1, "bucket_width_ms": 86_400_000,
            "range_start_ms": 0, "range_end_ms": 0, "row_count": 1, "size_bytes": 1
        }));
        handle_inbound_row_change(
            &mut fabric,
            "v-1",
            &change,
            "did:key:local",
            &evt_tx,
            &Some(db_tx.clone()),
        );
        assert!(
            evt_rx.try_recv().is_err(),
            "a non-hosted shard's row is not applied (no RowApplied)"
        );
        assert!(
            db_rx.try_recv().is_err(),
            "no DB apply for a non-hosted shard"
        );

        // The same row when the local peer IS the host applies normally.
        let mut fabric = VerseFabric::default();
        fabric.note_verse_row(&serde_json::json!({
            "ts_mode": "sharded", "ts_bucket_width_ms": 86_400_000
        }));
        fabric.note_shard_row(&serde_json::json!({
            "shard": "p1/a1/20284", "hosts": ["did:key:local"],
            "mode": "sharded", "replication_factor": 1, "bucket_width_ms": 86_400_000,
            "range_start_ms": 0, "range_end_ms": 0, "row_count": 1, "size_bytes": 1
        }));
        handle_inbound_row_change(
            &mut fabric,
            "v-1",
            &change,
            "did:key:local",
            &evt_tx,
            &Some(db_tx),
        );
        assert!(
            matches!(evt_rx.try_recv(), Ok(SyncEvent::RowApplied { .. })),
            "a hosted shard's row applies"
        );
        assert!(
            db_rx.try_recv().is_ok(),
            "hosted shard row rides the DB apply path"
        );

        // Mirror mode (default) retains everything — pre-F6 behavior.
        let (evt_tx, evt_rx) = crossbeam::channel::bounded(8);
        let mut mirror = VerseFabric::default();
        handle_inbound_row_change(&mut mirror, "v-1", &change, "did:key:local", &evt_tx, &None);
        assert!(
            matches!(evt_rx.try_recv(), Ok(SyncEvent::RowApplied { .. })),
            "mirror mode retains every shard"
        );
    }

    /// `__shards`/`__peers` rows are sync-plane: the seam consumes them into
    /// the fabric and NEVER forwards them to the DB thread.
    #[tokio::test]
    async fn sync_plane_rows_never_reach_the_db_thread() {
        let (evt_tx, evt_rx) = crossbeam::channel::bounded(8);
        let (db_tx, db_rx) = crossbeam::channel::bounded::<DbCommand>(8);
        let mut fabric = VerseFabric::default();

        let peer_row = RowChange {
            table: PEER_DECL_TABLE.to_string(),
            record_id: "did:key:peer-b".to_string(),
            content_hash: [0u8; 32],
            author_id: "did:key:peer-b".to_string(),
            timestamp: 1,
            is_tombstone: false,
            data: br#"{"capacity_bytes":1000,"seeder":true}"#.to_vec(),
        };
        handle_inbound_row_change(
            &mut fabric,
            "v-1",
            &peer_row,
            "did:key:local",
            &evt_tx,
            &Some(db_tx.clone()),
        );
        assert_eq!(
            fabric
                .peers
                .get("did:key:peer-b")
                .and_then(|p| p.capacity_bytes),
            Some(1000),
            "peer declaration consumed into the fabric"
        );

        let shard_row = RowChange {
            table: SHARD_TABLE.to_string(),
            record_id: "p1/a1/1".to_string(),
            content_hash: [0u8; 32],
            author_id: "did:key:peer-b".to_string(),
            timestamp: 2,
            is_tombstone: false,
            data: br#"{"shard":"p1/a1/1","hosts":["did:key:peer-b"],"mode":"sharded","replication_factor":1,"bucket_width_ms":1000,"range_start_ms":1000,"range_end_ms":2000,"row_count":1,"size_bytes":10}"#.to_vec(),
        };
        handle_inbound_row_change(
            &mut fabric,
            "v-1",
            &shard_row,
            "did:key:local",
            &evt_tx,
            &Some(db_tx),
        );
        assert!(fabric.shards.contains_key("p1/a1/1"), "ledger row consumed");
        assert!(
            evt_rx.try_recv().is_err(),
            "sync-plane rows emit no RowApplied"
        );
        assert!(
            db_rx.try_recv().is_err(),
            "sync-plane rows never reach the DB thread"
        );
    }

    // -------------------------------------------------------------------------
    // F4: startup reconciliation (snapshot → inbound apply path)
    // -------------------------------------------------------------------------

    /// A replica's `snapshot()` enumerates its current entries as RowChanges,
    /// with payload bytes and tombstone flags intact — the reconciliation
    /// pass's input.
    #[tokio::test]
    async fn mock_snapshot_returns_current_entries() {
        use crate::replicator::MockVerseReplicator;
        let repl = MockVerseReplicator::new("did:key:peer");
        repl.write_row("node", "node-1", br#"{"name":"live"}"#)
            .await
            .unwrap();
        repl.write_row("node", "node-2", b"").await.unwrap();

        let snap = repl.snapshot().await.unwrap();
        assert_eq!(snap.len(), 2, "both entries enumerated");
        let live = snap.iter().find(|c| c.record_id == "node-1").unwrap();
        assert_eq!(live.table, "node");
        assert_eq!(live.data, br#"{"name":"live"}"#.to_vec());
        assert!(!live.is_tombstone);
        let tomb = snap.iter().find(|c| c.record_id == "node-2").unwrap();
        assert!(tomb.is_tombstone, "empty entry is a deletion marker");
        assert!(tomb.data.is_empty());
    }

    /// `seed_reconciliation` replays a replica's entries through the inbound
    /// apply path — applied DIRECTLY (F20 finding 1: never by awaiting a
    /// send into the aggregated inbound stream, whose only drainer is the
    /// command loop that is running this pass). Survivors emit `RowApplied`
    /// and ride a `DbCommand::ApplyReplicatedRow`; own-author rows are
    /// filtered (loop prevention) — the second-chance convergence path.
    #[tokio::test]
    async fn seed_reconciliation_applies_snapshot_directly() {
        use crate::replicator::ReplicatorFuture;

        fn change(table: &str, record_id: &str, author: &str, data: &[u8]) -> RowChange {
            RowChange {
                table: table.to_string(),
                record_id: record_id.to_string(),
                content_hash: [0u8; 32],
                author_id: author.to_string(),
                timestamp: 1,
                is_tombstone: false,
                data: data.to_vec(),
            }
        }
        // Two peer-authored rows + one own-author row (a snapshot entry we
        // wrote ourselves — filtered by the seam like any own echo).
        let entries = vec![
            change(
                "verse",
                "verse-1",
                "did:key:peer",
                br#"{"name":"manifest"}"#,
            ),
            change("node", "node-1", "did:key:peer", br#"{"name":"stranded"}"#),
            change("node", "node-own", "did:key:local", br#"{"name":"ours"}"#),
        ];
        struct FixedSnapshot(Vec<RowChange>);
        impl VerseReplicator for FixedSnapshot {
            fn write_row(
                &self,
                _table: &str,
                _record_id: &str,
                _data: &[u8],
            ) -> ReplicatorFuture<'_, anyhow::Result<()>> {
                Box::pin(async { Ok(()) })
            }
            fn subscribe(
                &self,
            ) -> ReplicatorFuture<'_, anyhow::Result<tokio::sync::mpsc::Receiver<RowChange>>>
            {
                Box::pin(async { unreachable!("not used in this test") })
            }
            fn snapshot(&self) -> ReplicatorFuture<'_, anyhow::Result<Vec<RowChange>>> {
                Box::pin(async move { Ok(self.0.clone()) })
            }
            fn close(&self) -> ReplicatorFuture<'_, anyhow::Result<()>> {
                Box::pin(async { Ok(()) })
            }
        }

        let (evt_tx, evt_rx) = crossbeam::channel::bounded(16);
        let (db_tx, db_rx) = crossbeam::channel::bounded::<DbCommand>(16);
        let fixed = FixedSnapshot(entries);
        seed_reconciliation(
            &fixed,
            "verse-1",
            "did:key:local",
            &evt_tx,
            &Some(db_tx),
            &mut VerseFabric::default(),
        )
        .await;

        // Events: the two peer-authored rows emit RowApplied; the own-author
        // row is filtered at the seam (no event, no DbCommand).
        let mut applied_ids = Vec::new();
        while let Ok(evt) = evt_rx.try_recv() {
            if let SyncEvent::RowApplied { record_id, .. } = evt {
                applied_ids.push(record_id);
            }
        }
        applied_ids.sort();
        assert_eq!(
            applied_ids,
            vec!["node-1".to_string(), "verse-1".to_string()],
            "peer-authored rows emit RowApplied; own-author rows are filtered"
        );
        let mut cmd_ids = Vec::new();
        while let Ok(cmd) = db_rx.try_recv() {
            if let DbCommand::ApplyReplicatedRow { record_id, .. } = cmd {
                cmd_ids.push(record_id);
            }
        }
        cmd_ids.sort();
        assert_eq!(
            cmd_ids,
            vec!["node-1".to_string(), "verse-1".to_string()],
            "peer-authored rows ride DbCommand::ApplyReplicatedRow; own-author rows never re-apply"
        );
    }

    /// An empty replica (nothing ever replicated in) is a silent no-op.
    #[tokio::test]
    async fn seed_reconciliation_empty_replica_is_noop() {
        use crate::replicator::MockVerseReplicator;
        let repl = MockVerseReplicator::new("did:key:peer");
        let (evt_tx, evt_rx) = crossbeam::channel::bounded(4);
        let (db_tx, db_rx) = crossbeam::channel::bounded::<DbCommand>(4);
        seed_reconciliation(
            &repl,
            "verse-empty",
            "did:key:local",
            &evt_tx,
            &Some(db_tx),
            &mut VerseFabric::default(),
        )
        .await;
        assert!(evt_rx.try_recv().is_err(), "no events — nothing forwarded");
        assert!(db_rx.try_recv().is_err(), "no DbCommands — nothing applied");
    }

    /// F20/M1 finding 4: on an ONLINE stack, an `open_document` failure
    /// leaves a loud NON-replicating replica — the failed replicator stays
    /// registered (no mock install), writes and snapshots error instead of
    /// silently succeeding in memory, no inbound pump is spawned, and a
    /// `SyncEvent::ReplicaOpenFailed` fires for the host to surface.
    // The env-mutex guard is deliberately held across awaits: it must
    // serialize this env-mutating test against every other one for the
    // test's whole duration (nothing inside this test contends on it).
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn online_open_failure_leaves_loud_non_replicating_replica() {
        let _env_guard = SYNC_THREAD_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        std::env::set_var(crate::relay_config::RELAY_CONFIG_ENV_VAR, "disabled");
        let tmp = tempfile::TempDir::new().unwrap();
        let secret = iroh::SecretKey::from_bytes(&[63u8; 32]);
        let endpoint =
            match crate::endpoint::SyncEndpoint::new(secret, &RelayConfig::from_env()).await {
                Ok(ep) => ep,
                Err(e) => {
                    tracing::warn!("skipping online test: endpoint bind failed ({e})");
                    return;
                }
            };
        let stack = Arc::new(
            DocsStack::spawn(endpoint.inner().clone(), tmp.path().join("p2p"))
                .await
                .expect("docs stack"),
        );
        let holder = Arc::new(IrohDocsEngineHolder::online(stack));

        let mut replicas: HashMap<String, Box<dyn VerseReplicator>> = HashMap::new();
        let mut inbound_pumps: HashMap<String, tokio::task::AbortHandle> = HashMap::new();
        let (inbound_tx, _inbound_rx) = tokio::sync::mpsc::channel::<(String, RowChange)>(16);
        let (evt_tx, evt_rx) = crossbeam::channel::bounded(8);
        let (db_tx, db_rx) = crossbeam::channel::bounded::<DbCommand>(8);
        let blob_store: BlobStoreHandle = Arc::new(MockBlobStore::new());
        let mut pending = PendingWrites::default();

        // A malformed secret forces a real open_document error on an ONLINE
        // stack — exactly the finding-4 shape.
        handle_open_verse_replica(
            &mut replicas,
            &mut inbound_pumps,
            inbound_tx,
            &ReplicaTransport::Iroh(holder),
            "verse-bad",
            "0".repeat(64).as_str(),
            Some("not-hex".to_string()),
            &[],
            "did:key:local",
            &evt_tx,
            &Some(db_tx),
            &blob_store,
            &mut pending,
            &mut VerseFabric::default(),
            PeerDeclaration::default(),
        )
        .await;

        // Loud: the failure is an observable event.
        match evt_rx.try_recv() {
            Ok(SyncEvent::ReplicaOpenFailed { ref verse_id, .. }) if verse_id == "verse-bad" => {}
            other => panic!("expected ReplicaOpenFailed for verse-bad, got {other:?}"),
        }
        // No pump, no reconciliation, no silent application.
        assert!(evt_rx.try_recv().is_err(), "no further events");
        assert!(db_rx.try_recv().is_err(), "nothing applied");
        assert!(
            !inbound_pumps.contains_key("verse-bad"),
            "no inbound pump for a failed online open"
        );
        // The failed replica stays registered (host can close it) and is
        // loudly non-replicating: writes error, never a mock success.
        let repl = replicas
            .get("verse-bad")
            .expect("failed replica stays registered");
        assert!(
            repl.write_row("node", "n1", b"{}").await.is_err(),
            "writes on a failed online open must error, not publish to a mock"
        );
        assert!(
            repl.snapshot().await.is_err(),
            "snapshot on a failed online open must error"
        );
    }

    /// F20/M1 finding 4 counterpart: the mock fallback is OFFLINE-ONLY —
    /// with an unavailable stack the same open failure keeps the sanctioned
    /// in-memory mock replica (usable writes, a live pump, no failure event).
    #[tokio::test]
    async fn offline_open_failure_keeps_mock_fallback() {
        let mut replicas: HashMap<String, Box<dyn VerseReplicator>> = HashMap::new();
        let mut inbound_pumps: HashMap<String, tokio::task::AbortHandle> = HashMap::new();
        let (inbound_tx, _inbound_rx) = tokio::sync::mpsc::channel::<(String, RowChange)>(16);
        let engine_holder = Arc::new(IrohDocsEngineHolder::new()); // offline
        let (evt_tx, evt_rx) = crossbeam::channel::bounded(8);
        let (db_tx, db_rx) = crossbeam::channel::bounded::<DbCommand>(8);
        let blob_store: BlobStoreHandle = Arc::new(MockBlobStore::new());
        let mut pending = PendingWrites::default();

        handle_open_verse_replica(
            &mut replicas,
            &mut inbound_pumps,
            inbound_tx,
            &ReplicaTransport::Iroh(engine_holder),
            "verse-offline",
            "0".repeat(64).as_str(),
            Some("not-hex".to_string()),
            &[],
            "did:key:local",
            &evt_tx,
            &Some(db_tx),
            &blob_store,
            &mut pending,
            &mut VerseFabric::default(),
            PeerDeclaration::default(),
        )
        .await;

        // No failure event — the offline mock path is the designed fallback.
        assert!(
            !matches!(evt_rx.try_recv(), Ok(SyncEvent::ReplicaOpenFailed { .. })),
            "no ReplicaOpenFailed when the stack is offline (mock is sanctioned)"
        );
        assert!(
            db_rx.try_recv().is_err(),
            "empty mock snapshot applies nothing"
        );
        assert!(
            inbound_pumps.contains_key("verse-offline"),
            "mock replica still pumps inbound events"
        );
        let repl = replicas
            .get("verse-offline")
            .expect("mock replica registered");
        assert!(
            repl.write_row("node", "n1", b"{}").await.is_ok(),
            "offline mock writes succeed in memory by design"
        );
    }

    /// F20/M1 finding 1 regression: a replica whose doc holds MORE entries
    /// than the aggregated inbound channel's capacity (256) must not wedge
    /// the sync thread on open. The reconciliation pass applies the snapshot
    /// directly through the inbound apply path (it never awaits a send into
    /// the stream that only the command loop drains), so the loop keeps
    /// processing commands — including `Shutdown` — and the replayed
    /// entries still apply. The pre-fix code deadlocked the loop forever on
    /// the 257th queued entry (an `await` whose receiver was the awaiting
    /// loop itself), starving ALL inbound applies, teardown, and any
    /// blocking crossbeam senders.
    // The env-mutex guard is deliberately held across awaits: it must
    // serialize this env-mutating test against every other one for the
    // test's whole duration (nothing inside this test contends on it).
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn reconciliation_snapshot_larger_than_inbound_capacity_does_not_deadlock() {
        let _env_guard = SYNC_THREAD_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        std::env::set_var(crate::relay_config::RELAY_CONFIG_ENV_VAR, "disabled");
        let tmp = tempfile::TempDir::new().unwrap();
        const ROWS: usize = 300; // > the 256-capacity aggregated inbound channel

        // Peer A: a test-owned online stack authoring 300 rows in the doc.
        let secret_a = iroh::SecretKey::from_bytes(&[61u8; 32]);
        let endpoint =
            match crate::endpoint::SyncEndpoint::new(secret_a, &RelayConfig::from_env()).await {
                Ok(ep) => ep,
                Err(e) => {
                    tracing::warn!("skipping online test: endpoint bind failed ({e})");
                    return;
                }
            };
        let stack_a = Arc::new(
            DocsStack::spawn(endpoint.inner().clone(), tmp.path().join("a"))
                .await
                .expect("docs stack A"),
        );
        let holder_a = Arc::new(IrohDocsEngineHolder::online(stack_a.clone()));
        let ns_secret = [17u8; 32];
        let ns_id = hex::encode(fe_database::derive_namespace_id(&ns_secret));
        let ns_secret_hex = hex::encode(ns_secret);
        let alice = IrohDocsReplicator::new(
            "verse-big".to_string(),
            ns_id.clone(),
            ns_secret_hex.clone(),
            "did:key:alice".to_string(),
            holder_a,
        );
        alice.open_document().await.expect("alice opens her doc");
        alice
            .start_sync(Vec::new())
            .await
            .expect("alice serves her doc");
        for i in 0..ROWS {
            let payload = format!("{{\"node_id\":\"row-{i:03}\",\"name\":\"row {i}\"}}");
            alice
                .write_row("node", &format!("row-{i:03}"), payload.as_bytes())
                .await
                .expect("alice authors row");
        }
        let addr_a = endpoint.inner().node_addr().await.expect("alice addr");

        // Peer B: a REAL sync thread (the system under test).
        let secret_b = iroh::SecretKey::from_bytes(&[62u8; 32]);
        let local_did_b =
            fe_identity::did_key::did_key_from_public_key_bytes(secret_b.public().as_bytes())
                .expect("B did:key");
        let store: BlobStoreHandle = Arc::new(MockBlobStore::new());
        let (cmd_tx, cmd_rx) = crossbeam::channel::bounded(64);
        let (evt_tx, evt_rx) = crossbeam::channel::bounded(4096);
        let (db_tx, db_rx) = crossbeam::channel::bounded::<DbCommand>(4096);
        let sync = spawn_sync_thread(
            secret_b,
            store,
            cmd_rx,
            evt_tx,
            local_did_b,
            Some(db_tx),
            Some(tmp.path().join("b")),
        );
        let started = {
            let rx = evt_rx.clone();
            tokio::task::spawn_blocking(move || rx.recv_timeout(std::time::Duration::from_secs(20)))
                .await
                .expect("event-wait task")
                .expect("Started event")
        };
        assert!(
            matches!(started, SyncEvent::Started { online: true, .. }),
            "this test requires an online stack, got {started:?}"
        );

        // Wait for a count of RowApplied events for the verse, skipping
        // unrelated traffic, until `want` is reached or the deadline passes.
        //
        // The crossbeam recv runs on `spawn_blocking` and is AWAITED: this
        // is a current-thread runtime, and alice's QUIC endpoint shares it
        // — a blocking recv parked inline would starve her transport and
        // the sync would never converge.
        async fn wait_row_applied(
            evt_rx: &crossbeam::channel::Receiver<SyncEvent>,
            want: usize,
            verse_id: &str,
        ) -> usize {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
            let mut got = 0usize;
            while got < want && std::time::Instant::now() < deadline {
                let rx = evt_rx.clone();
                let next = tokio::task::spawn_blocking(move || {
                    rx.recv_timeout(std::time::Duration::from_millis(250))
                })
                .await
                .expect("event-wait task");
                match next {
                    Ok(SyncEvent::RowApplied { verse_id: v, .. }) if v == verse_id => got += 1,
                    Ok(_) => continue,
                    Err(crossbeam::channel::RecvTimeoutError::Timeout) => continue,
                    Err(crossbeam::channel::RecvTimeoutError::Disconnected) => break,
                }
            }
            got
        }

        // 1) Live convergence: B opens the replica with the write capability
        //    and dials A; all ROWS converge through the real transport.
        cmd_tx
            .send(SyncCommand::OpenVerseReplica {
                verse_id: "verse-big".to_string(),
                namespace_id: ns_id.clone(),
                namespace_secret: Some(ns_secret_hex.clone()),
                bootstrap_peers: vec![serde_json::to_string(&addr_a).unwrap()],
            })
            .expect("open command");
        let live = wait_row_applied(&evt_rx, ROWS, "verse-big").await;
        assert_eq!(live, ROWS, "live pump converged all {ROWS} rows");
        while db_rx.try_recv().is_ok() {} // baseline: live applies drained

        // 2) THE REGRESSION SHAPE: re-open the replica. The seed
        //    reconciliation now faces a {ROWS}-entry snapshot — larger than
        //    the 256-capacity aggregated inbound channel — while running
        //    inline in the command loop's select arm. Pre-fix, this arm
        //    deadlocked forever; post-fix the replay applies directly and
        //    the loop returns to service.
        cmd_tx
            .send(SyncCommand::OpenVerseReplica {
                verse_id: "verse-big".to_string(),
                namespace_id: ns_id.clone(),
                namespace_secret: Some(ns_secret_hex.clone()),
                bootstrap_peers: vec![serde_json::to_string(&addr_a).unwrap()],
            })
            .expect("re-open command");
        let replayed = wait_row_applied(&evt_rx, ROWS, "verse-big").await;
        assert_eq!(
            replayed, ROWS,
            "reconciliation replayed all {ROWS} snapshot entries past channel capacity"
        );

        // 3) Proof the loop is NOT wedged: Shutdown must still be processed
        //    (A8 teardown) and the thread must exit (asserted via the
        //    Stopped event with a timeout, so a regression fails instead of
        //    hanging the suite).
        cmd_tx.send(SyncCommand::Shutdown).expect("shutdown send");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            let rx = evt_rx.clone();
            let next = tokio::task::spawn_blocking(move || {
                rx.recv_timeout(std::time::Duration::from_millis(500))
            })
            .await
            .expect("event-wait task");
            match next {
                Ok(SyncEvent::Stopped) => break,
                Ok(_) => continue,
                Err(crossbeam::channel::RecvTimeoutError::Timeout)
                    if std::time::Instant::now() < deadline =>
                {
                    continue
                }
                Err(other) => panic!("sync thread did not stop cleanly: {other:?}"),
            }
        }
        sync.join().expect("sync thread exited cleanly");
    }

    // -------------------------------------------------------------------------
    // F21/M2: the verse-manifest open race (emit-before-open)
    // -------------------------------------------------------------------------

    /// The pending-writes queue is bounded: past the cap the OLDEST entry is
    /// dropped so the queue retains the newest versions per key (the doc
    /// still converges to the latest content on flush), and `take` returns
    /// one verse's writes in FIFO order.
    #[test]
    fn pending_writes_cap_drops_oldest_and_takes_fifo() {
        let mut pending = PendingWrites::default();
        for i in 0..(PENDING_WRITES_CAP + 8) {
            pending.push(
                &format!("verse-{}", i % 4),
                PendingWrite {
                    table: "node".to_string(),
                    record_id: format!("row-{i:04}"),
                    content_hash: [i as u8; 32],
                },
            );
        }
        assert_eq!(
            pending.len(),
            PENDING_WRITES_CAP,
            "queue bounded at the cap"
        );
        // The oldest 8 entries (row-0000 … row-0007) were dropped.
        let taken = pending.take("verse-0");
        assert!(!taken.iter().any(|w| w.record_id == "row-0000"));
        assert!(taken.iter().any(|w| w.record_id == "row-0012"));
        assert!(pending.len() < PENDING_WRITES_CAP, "take drains its verse");

        // FIFO order survives interleaved verses.
        let mut pending = PendingWrites::default();
        for (verse, id) in [("a", 1), ("b", 1), ("a", 2), ("b", 2), ("a", 3)] {
            pending.push(
                verse,
                PendingWrite {
                    table: "node".to_string(),
                    record_id: format!("row-{verse}-{id}"),
                    content_hash: [id as u8; 32],
                },
            );
        }
        let taken = pending.take("a");
        assert_eq!(
            taken
                .iter()
                .map(|w| w.record_id.clone())
                .collect::<Vec<_>>(),
            ["row-a-1", "row-a-2", "row-a-3"],
            "per-verse FIFO order preserved"
        );
        assert_eq!(pending.len(), 2, "other verses untouched by take");
    }

    /// F21 at the offline seam: a write that arrives while the verse's
    /// replica is not open is RETAINED (never warn-and-dropped), and the
    /// replica open flushes it through the normal write path — READ-BACK via
    /// the replica's own snapshot. Writes arriving AFTER the open publish
    /// directly and never queue.
    #[tokio::test]
    async fn write_before_open_retained_then_flushed_through_offline_mock() {
        let tmp = tempfile::TempDir::new().unwrap();
        let blob_store: BlobStoreHandle =
            Arc::new(crate::FsBlobStore::new(tmp.path().join("blobs")).expect("fs blob store"));
        let manifest =
            br#"{"verse_id":"v-race","name":"Offline Race Verse","default_access":"viewer"}"#;
        let manifest_hash = blob_store.add_blob(manifest).expect("manifest blob");

        let mut replicas: HashMap<String, Box<dyn VerseReplicator>> = HashMap::new();
        let mut inbound_pumps: HashMap<String, tokio::task::AbortHandle> = HashMap::new();
        let (inbound_tx, _inbound_rx) = tokio::sync::mpsc::channel::<(String, RowChange)>(16);
        let engine_holder = Arc::new(IrohDocsEngineHolder::new()); // offline → mock
        let (evt_tx, _evt_rx) = crossbeam::channel::bounded(8);
        let mut pending = PendingWrites::default();

        // THE RACE SHAPE: the manifest write is emitted while no replica is
        // open (exactly what create_verse_handler → VerseCreated does — the
        // open arrives later).
        handle_write_row_entry(
            &replicas,
            &mut pending,
            &mut VerseFabric::default(),
            &blob_store,
            "did:key:local",
            "v-race",
            "verse",
            "v-race",
            &manifest_hash,
        )
        .await;
        assert_eq!(
            pending.len(),
            1,
            "no open replica — write retained, not dropped"
        );

        // The open that races it: flushes the retained write.
        handle_open_verse_replica(
            &mut replicas,
            &mut inbound_pumps,
            inbound_tx.clone(),
            &ReplicaTransport::Iroh(engine_holder),
            "v-race",
            "0".repeat(64).as_str(),
            None,
            &[],
            "did:key:local",
            &evt_tx,
            &None,
            &blob_store,
            &mut pending,
            &mut VerseFabric::default(),
            PeerDeclaration::default(),
        )
        .await;
        assert!(pending.is_empty(), "the open flushed the retained write");

        // READ-BACK: the manifest row is in the replica's doc.
        let snap = replicas
            .get("v-race")
            .expect("replica registered")
            .snapshot()
            .await
            .expect("snapshot");
        let row = snap
            .iter()
            .find(|c| c.table == "verse" && c.record_id == "v-race")
            .expect("manifest row present in the replica doc");
        assert_eq!(row.data, manifest.to_vec());

        // Post-open writes publish directly and never queue again.
        let later = br#"{"fractal_id":"f-1","verse_id":"v-race"}"#;
        let later_hash = blob_store.add_blob(later).expect("later blob");
        handle_write_row_entry(
            &replicas,
            &mut pending,
            &mut VerseFabric::default(),
            &blob_store,
            "did:key:local",
            "v-race",
            "fractal",
            "f-1",
            &later_hash,
        )
        .await;
        assert!(
            pending.is_empty(),
            "an open replica publishes directly — no queueing"
        );
        let snap = replicas
            .get("v-race")
            .unwrap()
            .snapshot()
            .await
            .expect("snapshot");
        assert!(
            snap.iter()
                .any(|c| c.table == "fractal" && c.record_id == "f-1"),
            "direct publish landed in the doc"
        );
    }

    /// F20-finding-4 interaction: a write retained before a FAILED online
    /// open is NOT flushed (a loudly non-replicating replica publishes
    /// nothing) and stays retained — a later successful (re-)open flushes it.
    // The env-mutex guard is deliberately held across awaits: it must
    // serialize this env-mutating test against every other one for the
    // test's whole duration (nothing inside this test contends on it).
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn failed_online_open_retains_pending_until_successful_reopen() {
        let _env_guard = SYNC_THREAD_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        std::env::set_var(crate::relay_config::RELAY_CONFIG_ENV_VAR, "disabled");
        let tmp = tempfile::TempDir::new().unwrap();
        let secret = iroh::SecretKey::from_bytes(&[67u8; 32]);
        let endpoint =
            match crate::endpoint::SyncEndpoint::new(secret, &RelayConfig::from_env()).await {
                Ok(ep) => ep,
                Err(e) => {
                    tracing::warn!("skipping online test: endpoint bind failed ({e})");
                    return;
                }
            };
        let stack = Arc::new(
            DocsStack::spawn(endpoint.inner().clone(), tmp.path().join("p2p"))
                .await
                .expect("docs stack"),
        );
        let holder = Arc::new(IrohDocsEngineHolder::online(stack));

        let blob_store: BlobStoreHandle =
            Arc::new(crate::FsBlobStore::new(tmp.path().join("blobs")).expect("fs blob store"));
        let manifest = br#"{"verse_id":"v-flaky","name":"Flaky Verse","default_access":"viewer"}"#;
        let manifest_hash = blob_store.add_blob(manifest).expect("manifest blob");

        let ns_secret = [29u8; 32];
        let ns_id = hex::encode(fe_database::derive_namespace_id(&ns_secret));
        let mut replicas: HashMap<String, Box<dyn VerseReplicator>> = HashMap::new();
        let mut inbound_pumps: HashMap<String, tokio::task::AbortHandle> = HashMap::new();
        let (inbound_tx, _inbound_rx) = tokio::sync::mpsc::channel::<(String, RowChange)>(16);
        let (evt_tx, evt_rx) = crossbeam::channel::bounded(8);
        let mut pending = PendingWrites::default();

        // The write arrives before any open — retained.
        handle_write_row_entry(
            &replicas,
            &mut pending,
            &mut VerseFabric::default(),
            &blob_store,
            "did:key:local",
            "v-flaky",
            "verse",
            "v-flaky",
            &manifest_hash,
        )
        .await;

        // A FAILED online open must not flush it (loud non-replicating
        // state publishes nothing).
        handle_open_verse_replica(
            &mut replicas,
            &mut inbound_pumps,
            inbound_tx.clone(),
            &ReplicaTransport::Iroh(holder.clone()),
            "v-flaky",
            ns_id.as_str(),
            Some("not-hex".to_string()),
            &[],
            "did:key:local",
            &evt_tx,
            &None,
            &blob_store,
            &mut pending,
            &mut VerseFabric::default(),
            PeerDeclaration::default(),
        )
        .await;
        assert!(
            matches!(
                evt_rx.try_recv(),
                Ok(SyncEvent::ReplicaOpenFailed { ref verse_id, .. }) if verse_id == "v-flaky"
            ),
            "failed online open is loud"
        );
        assert_eq!(
            pending.len(),
            1,
            "a failed open retains (never flushes) pending writes"
        );

        // A later successful open flushes them.
        handle_open_verse_replica(
            &mut replicas,
            &mut inbound_pumps,
            inbound_tx,
            &ReplicaTransport::Iroh(holder),
            "v-flaky",
            ns_id.as_str(),
            Some(hex::encode(ns_secret)),
            &[],
            "did:key:local",
            &evt_tx,
            &None,
            &blob_store,
            &mut pending,
            &mut VerseFabric::default(),
            PeerDeclaration::default(),
        )
        .await;
        assert!(
            pending.is_empty(),
            "successful re-open flushed the retained write"
        );
        let snap = replicas
            .get("v-flaky")
            .expect("replica registered")
            .snapshot()
            .await
            .expect("doc-backed snapshot");
        let row = snap
            .iter()
            .find(|c| c.table == "verse" && c.record_id == "v-flaky")
            .expect("manifest row published by the flush");
        assert_eq!(row.data, manifest.to_vec());
    }

    /// F21/M2 regression at the EXACT failure shape: a verse manifest row
    /// emitted BEFORE its replica open (the create_verse → VerseCreated race)
    /// must be durably present in its own doc, and a fresh peer must converge
    /// it through the real transport. Proves, on a REAL sync thread:
    ///
    /// 1. the emit-before-open write is retained, not warn-and-dropped;
    /// 2. the open republishes it (FIFO flush through the normal write path);
    /// 3. a fresh peer (cold store, joins the namespace, dials the creator)
    ///    converges the manifest through the real loopback transport — the
    ///    bootstrap-window contract (A3) preserved;
    /// 4. the manifest is DURABLE in the creator's own doc: after a clean
    ///    shutdown, a read-only reopen of the same persisted store finds it
    ///    (get_many read-back — the exact instrument seed_reconciliation
    ///    uses).
    // The env-mutex guard is deliberately held across awaits: it must
    // serialize this env-mutating test against every other one for the
    // test's whole duration (nothing inside this test contends on it).
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn manifest_written_before_replica_open_converges_to_fresh_peer_and_reads_back() {
        let _env_guard = SYNC_THREAD_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        std::env::set_var(crate::relay_config::RELAY_CONFIG_ENV_VAR, "disabled");
        let tmp = tempfile::TempDir::new().unwrap();

        // The creator: a REAL sync thread — the same spawn_sync_thread the
        // relay's VerseCreated system and the GUI's navigation ride.
        let secret_b = iroh::SecretKey::from_bytes(&[71u8; 32]);
        let local_did_b =
            fe_identity::did_key::did_key_from_public_key_bytes(secret_b.public().as_bytes())
                .expect("B did:key");
        let blob_store: BlobStoreHandle =
            Arc::new(crate::FsBlobStore::new(tmp.path().join("blobs")).expect("fs blob store"));
        let (cmd_tx, cmd_rx) = crossbeam::channel::bounded(64);
        let (evt_tx, evt_rx) = crossbeam::channel::bounded(4096);
        let sync = spawn_sync_thread(
            secret_b,
            blob_store.clone(),
            cmd_rx,
            evt_tx,
            local_did_b.clone(),
            None,
            Some(tmp.path().join("b")),
        );
        let started = {
            let rx = evt_rx.clone();
            tokio::task::spawn_blocking(move || rx.recv_timeout(std::time::Duration::from_secs(20)))
                .await
                .expect("event-wait task")
                .expect("Started event")
        };
        let SyncEvent::Started {
            online: true,
            node_addr: Some(addr_json),
        } = started
        else {
            panic!("this test requires an online stack, got {started:?}");
        };
        let addr_b: iroh::NodeAddr =
            serde_json::from_str(&addr_json).expect("creator NodeAddr JSON");

        // The verse the creator is about to make — real ULID + derived
        // namespace id, exactly the values create_verse_handler writes.
        let verse_id = ulid::Ulid::new().to_string();
        let ns_secret = [23u8; 32];
        let ns_id = hex::encode(fe_database::derive_namespace_id(&ns_secret));
        let ns_secret_hex = hex::encode(ns_secret);
        let manifest = serde_json::json!({
            "verse_id": verse_id,
            "name": "Open-Race Verse",
            "created_by": local_did_b,
            "created_at": "2026-10-07T00:00:00Z",
            "namespace_id": ns_id,
            "default_access": "viewer",
        });
        let manifest_bytes = serde_json::to_vec(&manifest).unwrap();
        let manifest_hash = blob_store.add_blob(&manifest_bytes).expect("manifest blob");

        // 1) THE RACE: the manifest write is emitted BEFORE the replica open
        //    (exactly the create_verse_handler → VerseCreated ordering).
        cmd_tx
            .send(SyncCommand::WriteRowEntry {
                verse_id: verse_id.clone(),
                table: "verse".to_string(),
                record_id: verse_id.clone(),
                content_hash: manifest_hash,
            })
            .expect("manifest write command");

        // 2) The open that races it (the VerseCreated system's / navigation's
        //    OpenVerseReplica).
        cmd_tx
            .send(SyncCommand::OpenVerseReplica {
                verse_id: verse_id.clone(),
                namespace_id: ns_id.clone(),
                namespace_secret: Some(ns_secret_hex.clone()),
                bootstrap_peers: Vec::new(),
            })
            .expect("replica open command");

        // Deterministic sequencing barrier: the command loop processes
        // commands in order, so a BlobReady emitted for this FetchBlob
        // proves the open (and its pending flush) has fully completed —
        // the fresh peer can then join without racing the open.
        let probe_hash = blob_store
            .add_blob(b"open-complete-probe")
            .expect("probe blob");
        cmd_tx
            .send(SyncCommand::FetchBlob {
                hash: probe_hash,
                verse_id: verse_id.clone(),
            })
            .expect("probe command");
        {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
            loop {
                let rx = evt_rx.clone();
                let next = tokio::task::spawn_blocking(move || {
                    rx.recv_timeout(std::time::Duration::from_millis(500))
                })
                .await
                .expect("event-wait task");
                match next {
                    Ok(SyncEvent::BlobReady { hash }) if hash == probe_hash => break,
                    Ok(_) => continue,
                    Err(crossbeam::channel::RecvTimeoutError::Timeout)
                        if std::time::Instant::now() < deadline =>
                    {
                        continue
                    }
                    Err(other) => panic!("open barrier never fired: {other:?}"),
                }
            }
        }

        // 3) A FRESH PEER joins the namespace from a cold store and converges
        //    the manifest through the real transport.
        let secret_c = iroh::SecretKey::from_bytes(&[72u8; 32]);
        let endpoint_c = crate::endpoint::SyncEndpoint::new(secret_c, &RelayConfig::from_env())
            .await
            .expect("fresh-peer endpoint");
        let stack_c = Arc::new(
            DocsStack::spawn(endpoint_c.inner().clone(), tmp.path().join("c"))
                .await
                .expect("fresh-peer docs stack"),
        );
        let holder_c = Arc::new(IrohDocsEngineHolder::online(stack_c));
        let fresh = IrohDocsReplicator::new(
            verse_id.clone(),
            ns_id.clone(),
            ns_secret_hex.clone(),
            "did:key:fresh-peer".to_string(),
            holder_c,
        );
        fresh
            .open_document()
            .await
            .expect("fresh peer imports the capability");
        fresh
            .start_sync(vec![addr_b])
            .await
            .expect("fresh peer dials the creator");

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
        let mut converged = None;
        while std::time::Instant::now() < deadline {
            let snap = fresh.snapshot().await.expect("fresh-peer snapshot");
            if let Some(row) = snap
                .iter()
                .find(|c| c.table == "verse" && c.record_id == verse_id)
            {
                converged = Some(row.data.clone());
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        let converged =
            converged.expect("fresh peer converged the manifest through the real transport");
        assert_eq!(
            converged, manifest_bytes,
            "converged manifest content matches the emitted row"
        );

        // 4) OWN-DOC DURABLE READ-BACK: shut the creator down cleanly, then
        //    reopen its persisted doc store read-only (secretless — F20
        //    aligned ids) and get_many the manifest.
        cmd_tx.send(SyncCommand::Shutdown).expect("shutdown send");
        {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
            loop {
                let rx = evt_rx.clone();
                let next = tokio::task::spawn_blocking(move || {
                    rx.recv_timeout(std::time::Duration::from_millis(500))
                })
                .await
                .expect("event-wait task");
                match next {
                    Ok(SyncEvent::Stopped) => break,
                    Ok(_) => continue,
                    Err(crossbeam::channel::RecvTimeoutError::Timeout)
                        if std::time::Instant::now() < deadline =>
                    {
                        continue
                    }
                    Err(other) => panic!("sync thread did not stop cleanly: {other:?}"),
                }
            }
        }
        sync.join().expect("sync thread exited cleanly");

        let secret_ro = iroh::SecretKey::from_bytes(&[73u8; 32]);
        let endpoint_ro = crate::endpoint::SyncEndpoint::new(secret_ro, &RelayConfig::from_env())
            .await
            .expect("read-back endpoint");
        let stack_ro = Arc::new(
            DocsStack::spawn(endpoint_ro.inner().clone(), tmp.path().join("b"))
                .await
                .expect("reopen the creator's persisted doc store"),
        );
        let holder_ro = Arc::new(IrohDocsEngineHolder::online(stack_ro));
        let reader = IrohDocsReplicator::new(
            verse_id.clone(),
            ns_id.clone(),
            String::new(), // secretless read-only reopen
            "did:key:reader".to_string(),
            holder_ro,
        );
        reader
            .open_document()
            .await
            .expect("secretless reopen of the persisted doc");
        let snap = reader.snapshot().await.expect("own-doc get_many read-back");
        let row = snap
            .iter()
            .find(|c| c.table == "verse" && c.record_id == verse_id)
            .expect("manifest row durably in its own doc despite the open race");
        assert_eq!(
            row.data, manifest_bytes,
            "durable manifest content matches the emitted row"
        );
    }
}
