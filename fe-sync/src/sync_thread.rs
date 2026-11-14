//! Sync thread — owns the iroh endpoint and processes [`SyncCommand`]s
//! (P2P Mycelium Phase D).
//!
//! Modelled after `fe-database::spawn_db_thread`: a dedicated OS thread with
//! its own single-threaded Tokio runtime.  If the iroh endpoint fails to
//! bind the thread enters **offline mode** — it stays alive, responds to
//! commands, but all network-dependent operations are no-ops.
//!
//! The command loop is a `tokio::select!` over the command channel, the
//! per-replica inbound event pumps (A2/A4), the gossip compute pumps, and
//! the home-relay status watcher (SYNC-RELAYWATCH): crossbeam's receiver has
//! no async API, so a bridge thread forwards commands into a tokio channel
//! before the select.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use fe_runtime::bevy_blob_reader::BlobFallbackSlot;
use fe_runtime::blob_store::{hash_from_hex, hash_to_hex, BlobStoreHandle};
use fe_runtime::blob_transport::{BlobTransport, Hash32, ProviderRef};
use fe_runtime::messages::DbCommand;

use iroh::Watcher as _;
use iroh_gossip::api::{Event as GossipTopicEvent, GossipReceiver};
use iroh_gossip::{Gossip, TopicId};

use crate::blob_transport::IrohBlobTransport;
use crate::distributed_query::{
    handle_gossip_incoming, handle_signed_gossip_incoming, submit_distributed_query,
    unix_now_ms, AdmittedGossip, DistributedTransport, GossipIncoming, SignedGossipIngress,
    TopicSender,
};
use crate::dialback_responder::{self, DialbackRequest, DialbackRequester, DialbackResponder, ReplySink};
use crate::gossip_auth::GossipFrameKind;

use crate::docs_engine::{p2p_data_dir, BlobAccessGate, DocsStack};
use crate::endpoint::SyncEndpoint;
use crate::event_outbox::{EventOutbox, EventSink, SHUTDOWN_FLUSH_TIMEOUT};
use crate::messages::{SyncCommand, SyncCommandReceiver, SyncEvent, SyncEventSender};
use crate::placement::Retention;
use crate::relay_config::{
    advertised_relay_entry, auto_home_relay, RelayAdvertisePolicy, RelayConfig, RelayHealth,
    RelayStatusSummary,
};
use crate::relay_reconcile::{RelayOp, RelayReconcileInputs, RelayReconciler, RelaySource};
use crate::relay_trust::evaluate_coverage;
use crate::replicator::{
    IrohDocsEngineHolder, IrohDocsReplicator, IrohPetalReplicator, PetalReplicator,
    ReplicatorFuture, RowChange, VerseReplicator,
};
use crate::sharding::{
    PeerDeclaration, ShardId, VerseFabric, GOSSIP_SIG_VERSION, HOLDERS_TABLE, PEER_DECL_TABLE,
    SHARD_TABLE,
};
use crate::tileset_transfer::{
    is_safe_tileset_id, CatalogEntry, MetaResolved, TilesetTransfer, TransferEvent,
};
use crate::verse_peers;
use crate::virtual_transport::{VirtualReplica, VirtualTransportFactory};

/// Env var carrying bootstrap peers for every opened replica: **semicolon**
/// `-separated` iroh `EndpointAddr` JSON entries (the legacy 0.35 `NodeAddr`
/// JSON shape is still accepted; a bare `EndpointId` is rejected loudly),
/// parsed with iroh's own serde/FromStr types. Semicolons because the JSON
/// itself contains commas — a comma-separated list would be ambiguous.
/// Invalid entries are skipped loudly, never fatal (see AGENTS.md
/// §relay-health for the relay config this rides alongside).
pub const BOOTSTRAP_ENV_VAR: &str = "FE_SYNC_BOOTSTRAP";

/// Inbound rows the DB-thread command channel was too full to take (keep-and-
/// count, never block the sync thread — QA-0138 keeps them for resend).
static INBOUND_APPLY_DROPS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Total inbound rows a full DB command channel refused at first send.
pub fn inbound_apply_drop_count() -> u64 {
    INBOUND_APPLY_DROPS.load(std::sync::atomic::Ordering::Relaxed)
}

/// Retention cap for pending writes: far beyond any real emit-before-open
/// race window, while bounding memory on a process that never opens some
/// verse's replica. Past the cap the OLDEST entry is dropped with a warn.
const PENDING_WRITES_CAP: usize = 1024;

/// How soon kept work — role work queued after a failure (full DB channel,
/// failed replica read; QA-0132) or the prefetch backlog (QA-0133) — is
/// retried while the loop is otherwise idle.
const KEPT_WORK_RETRY: std::time::Duration = std::time::Duration::from_secs(1);

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

/// How long a publication lease keeps a self-opened replica after its last write.
const PUBLICATION_LEASE: std::time::Duration = std::time::Duration::from_secs(120);

/// Remembered open parameters of one verse replica (QA-0120).
#[derive(Clone)]
struct ReplicaOpenParams {
    namespace_id: String,
    namespace_secret: Option<String>,
    bootstrap_peers: Vec<String>,
}

/// QA-0120 publication leases: a write retained for a closed verse replica
/// re-opens it (with the parameters of its last open or the host's
/// `RememberVerseReplica`) until [`PUBLICATION_LEASE`] after the last write,
/// so a revoke made from the verse list reaches peers now, not at the owner's
/// next visit. A host `OpenVerseReplica` takes ownership of the replica.
#[derive(Default)]
struct PublicationLeases {
    known: HashMap<String, ReplicaOpenParams>,
    /// Self-opened replicas and their expiry.
    leased: HashMap<String, std::time::Instant>,
    /// Lease opens queued on the command channel, not yet handled.
    opening: std::collections::HashSet<String>,
}

impl PublicationLeases {
    fn remember(
        &mut self,
        verse_id: String,
        namespace_id: String,
        namespace_secret: Option<String>,
        bootstrap_peers: Vec<String>,
    ) {
        if self.known.len() < 4096 || self.known.contains_key(&verse_id) {
            self.known.insert(
                verse_id,
                ReplicaOpenParams {
                    namespace_id,
                    namespace_secret,
                    bootstrap_peers,
                },
            );
        }
    }

    /// An `OpenVerseReplica` is being handled: the lease's own, or the host taking over.
    fn opened(
        &mut self,
        verse_id: &str,
        namespace_id: &str,
        namespace_secret: &Option<String>,
        bootstrap_peers: &[String],
    ) {
        if !self.opening.remove(verse_id) {
            self.leased.remove(verse_id);
            self.remember(
                verse_id.to_string(),
                namespace_id.to_string(),
                namespace_secret.clone(),
                bootstrap_peers.to_vec(),
            );
        }
    }

    /// A write was handled; returns the open command when it needs a lease.
    fn write(
        &mut self,
        verse_id: &str,
        replica_open: bool,
        now: std::time::Instant,
    ) -> Option<SyncCommand> {
        if let Some(expiry) = self.leased.get_mut(verse_id) {
            *expiry = now + PUBLICATION_LEASE;
            return None;
        }
        if replica_open || self.opening.contains(verse_id) {
            return None;
        }
        let params = self.known.get(verse_id)?.clone();
        self.opening.insert(verse_id.to_string());
        self.leased
            .insert(verse_id.to_string(), now + PUBLICATION_LEASE);
        Some(SyncCommand::OpenVerseReplica {
            verse_id: verse_id.to_string(),
            namespace_id: params.namespace_id,
            namespace_secret: params.namespace_secret,
            bootstrap_peers: params.bootstrap_peers,
        })
    }

    fn any_open(&self) -> bool {
        !self.leased.is_empty()
    }

    /// Leases past expiry, removed; their replicas are closed by the caller.
    fn expired(&mut self, now: std::time::Instant) -> Vec<String> {
        let expired: Vec<String> = self
            .leased
            .iter()
            .filter(|(verse, expiry)| **expiry <= now && !self.opening.contains(*verse))
            .map(|(verse, _)| verse.clone())
            .collect();
        for verse in &expired {
            self.leased.remove(verse);
        }
        expired
    }
}

/// Queue a lease's own open behind the host's commands (FIFO).
fn send_lease_command(tx: &tokio::sync::mpsc::WeakSender<SyncCommand>, command: SyncCommand) {
    let Some(tx) = tx.upgrade() else {
        tracing::debug!("Publication lease command dropped — sync thread stopping");
        return;
    };
    if let Err(error) = tx.try_send(command) {
        tracing::warn!("Publication lease command not queued: {error}");
    }
}

/// Send a [`SyncEvent`] back to the main thread, warning on failure —
/// never a bare `.ok()` (§warn-on-send-failure).
fn send_sync_event(evt_tx: &dyn EventSink, event: SyncEvent) {
    evt_tx.emit(event);
}

/// The iroh 0.35 `NodeAddr` JSON shape (`{node_id, relay_url,
/// direct_addresses}`) — still accepted in bootstrap config so existing
/// `FE_SYNC_BOOTSTRAP` strings keep parsing after the iroh 1.x port.
#[derive(serde::Deserialize)]
struct LegacyNodeAddr {
    node_id: iroh::EndpointId,
    #[serde(default)]
    relay_url: Option<iroh::RelayUrl>,
    #[serde(default)]
    direct_addresses: std::collections::BTreeSet<std::net::SocketAddr>,
}

impl From<LegacyNodeAddr> for iroh::EndpointAddr {
    fn from(legacy: LegacyNodeAddr) -> Self {
        let relay = legacy.relay_url.map(iroh::TransportAddr::Relay);
        let direct = legacy
            .direct_addresses
            .into_iter()
            .map(iroh::TransportAddr::Ip);
        iroh::EndpointAddr::from_parts(legacy.node_id, relay.into_iter().chain(direct))
    }
}

/// Parse one bootstrap peer entry with iroh's own types: a JSON-serialized
/// `iroh::EndpointAddr` (what `SyncEvent::Started.node_addr` emits), the
/// legacy 0.35 `NodeAddr` JSON shape, or a bare `EndpointId` (hex/base32 —
/// which carries no address and therefore cannot be dialed: that is
/// reported loudly and skipped).
fn parse_bootstrap_peer(entry: &str) -> Option<iroh::EndpointAddr> {
    let entry = entry.trim();
    if entry.is_empty() {
        return None;
    }
    let json_err = match serde_json::from_str::<iroh::EndpointAddr>(entry) {
        Ok(addr) => return Some(addr),
        Err(e) => e,
    };
    if let Ok(legacy) = serde_json::from_str::<LegacyNodeAddr>(entry) {
        return Some(legacy.into());
    }
    match entry.parse::<iroh::EndpointId>() {
        Ok(node_id) => {
            tracing::warn!(
                node_id = %node_id.fmt_short(),
                "bootstrap entry carries no address (expected EndpointAddr JSON) — skipped"
            );
            None
        }
        Err(_) => {
            tracing::warn!(
                entry = %entry.truncate_str(80),
                "invalid FE_SYNC_BOOTSTRAP entry (EndpointAddr JSON expected) — skipped: {json_err}"
            );
            None
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
/// Entries are **semicolon-separated** (an `EndpointAddr` JSON contains
/// commas, so the list separator must not). Invalid entries fail loudly at
/// startup (one warn each) and are skipped — a bad config must never take
/// the sync thread down.
fn bootstrap_peers_from_env() -> Vec<iroh::EndpointAddr> {
    std::env::var(BOOTSTRAP_ENV_VAR)
        .ok()
        .map(|raw| {
            raw.split(';')
                .filter_map(parse_bootstrap_peer)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

/// Derive an iroh-gossip `TopicId` (32 bytes) from a topic key string.
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
    fn scene_provider_roster(&self) -> Option<crate::replicator::SceneProviderRoster> {
        match self { Self::Iroh(replica) => replica.scene_provider_roster(), Self::Virtual(_) => None }
    }

    fn joined_peer_ready(&self, namespace_id: &str, issuer_did: &str) -> bool {
        match self {
            Self::Iroh(replica) => replica.joined_peer_ready(namespace_id, issuer_did),
            Self::Virtual(_) => false,
        }
    }
    fn export_join_ticket(
        &self,
        namespace_id: &str,
        role: fe_database::join_invite::JoinRole,
        issuer_did: &str,
    ) -> ReplicatorFuture<'_, anyhow::Result<crate::join_ticket::NativeJoinTicket>> {
        match self {
            Self::Iroh(replica) => replica.export_join_ticket(namespace_id, role, issuer_did),
            Self::Virtual(_) => Box::pin(async { anyhow::bail!("Native document ticket export is unavailable") }),
        }
    }
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

    fn holds_own_entry(
        &self,
        table: &str,
        record_id: &str,
        content_hash: &fe_runtime::blob_store::BlobHash,
    ) -> ReplicatorFuture<'_, bool> {
        match self {
            Self::Iroh(r) => r.holds_own_entry(table, record_id, content_hash),
            Self::Virtual(r) => r.holds_own_entry(table, record_id, content_hash),
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

    /// RP-5 §2.5: forwards to each arm's own `entries_by_author` — the
    /// iroh arm overrides it (`replicator.rs`); the virtual arm inherits
    /// the trait's default (no edit needed in `fe-sim`).
    fn entries_by_author(
        &self,
        author_id: &str,
        prefixes: &[&str],
    ) -> ReplicatorFuture<'_, anyhow::Result<Vec<RowChange>>> {
        match self {
            Self::Iroh(r) => r.entries_by_author(author_id, prefixes),
            Self::Virtual(r) => r.entries_by_author(author_id, prefixes),
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

    fn start_sync(
        &self,
        peers: Vec<iroh::EndpointAddr>,
    ) -> ReplicatorFuture<'_, anyhow::Result<()>> {
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
    spawn_sync_thread_with_sources(
        secret_key,
        blob_store,
        cmd_rx,
        evt_tx,
        local_did,
        db_cmd_tx,
        p2p_dir,
        virtual_transport,
        BlobFallbackSlot::default(),
    )
}

/// [`spawn_sync_thread_with_transport`] plus `blob_fallback`, the extension
/// byte source asked only after every peer missed (bindings.md §6
/// `sync.tileset-source`); an empty slot is peers only. It never changes the
/// transport, the relays or which peers are asked (AGENTS.md §tileset-transfer).
#[allow(clippy::too_many_arguments)]
pub fn spawn_sync_thread_with_sources(
    secret_key: iroh::SecretKey,
    blob_store: BlobStoreHandle,
    cmd_rx: SyncCommandReceiver,
    evt_tx: SyncEventSender,
    local_did: String,
    db_cmd_tx: Option<crossbeam::channel::Sender<DbCommand>>,
    p2p_dir: Option<PathBuf>,
    virtual_transport: Option<Arc<dyn VirtualTransportFactory>>,
    blob_fallback: BlobFallbackSlot,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("Failed to build sync Tokio runtime");

        rt.block_on(async move {
            // W1 residual: every emit is non-blocking; a full channel queues
            // into a backlog this loop drains (AGENTS.md §event-outbox).
            let evt_tx = EventOutbox::new(evt_tx);
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
            // EndpointId; in virtual mode it is the identity the sim hub tags
            // this peer's gossip deliveries with (F9/A21), so the F23
            // forged-attribution gate authenticates sim envelopes verbatim.
            let local_node: iroh::EndpointId = secret_key.public();
            // NET-26: the same key signs every gossip frame this node broadcasts (§gossip-signing).
            let gossip_signing_key = secret_key.clone();
            // Resolved before the bind: the endpoint re-binds the ports recorded here (QA-0145).
            let p2p_dir = p2p_dir.unwrap_or_else(p2p_data_dir);

            // Phase F.1: Create the iroh endpoint first. A virtual transport
            // (sim lab, F8/A19) skips the bind entirely — no real network, no
            // relay, no stack; replicas come from the factory instead.
            let mut relay_health: RelayHealth;
            // PAS-P3 S3: our own dialable address, embedded in outbound
            // tileset ads as `provider_addr` (the same JSON form
            // `SyncEvent::Started.node_addr` carries). `None` offline/virtual
            // — this node cannot serve tileset bundles to anyone.
            let mut my_provider_addr: Option<String> = None;
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
                None => match SyncEndpoint::new_with_stable_ports(secret_key, &relay_config, &p2p_dir).await {
                Ok(ep) => {
                    tracing::info!(
                        node_id = %ep.node_id(),
                        "Sync thread started (online)"
                    );
                    // Our own dialable address, for peers/relays joining us
                    // (serialized EndpointAddr JSON — `parse_bootstrap_peer`
                    // reads this exact form). With a relay configured this
                    // waits (bounded) for a home relay so the address
                    // carries the relay URL.
                    let node_addr = ep.node_addr().await;
                    let addr_json = match serde_json::to_string(&node_addr) {
                        Ok(json) => Some(json),
                        Err(e) => {
                            tracing::warn!("Could not serialize our endpoint addr: {e}");
                            None
                        }
                    };
                    my_provider_addr = addr_json.clone();
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

            // Real iroh 1.x docs stack (A1): Blobs (fs) + Gossip + Docs
            // (persistent redb) + Router accepting all three ALPNs, with
            // persistent stores under the data dir (a legacy 0.35 store is
            // migrated first). Absent in offline mode (endpoint bind failure,
            // store migration or stack spawn failure) — the holder then
            // reports unavailable and replicators degrade to the in-memory
            // mock. See AGENTS.md §iroh-1.x and docs_engine.rs.
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
                        // iroh 1.x binds without contacting a relay, so
                        // the post-bind health is `Unknown`; a stack failure
                        // keeps 0.35's one-step degradation (`Degraded`)
                        // rather than claiming the relay is unreachable.
                        relay_health = match relay_health {
                            RelayHealth::Unknown => RelayHealth::Degraded,
                            other => other.on_error(),
                        };
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

            // PAS-P3 S3: the tileset transfer service, over the stack's blob
            // store + endpoint — only when a stack exists. Offline/virtual
            // mode has none: every tileset command then answers honestly
            // (see `handle_download_tileset` et al.), never a silent drop
            // (`AGENTS.md` §tileset-transfer).
            let tileset_transfer: Option<Arc<TilesetTransfer>> = docs_stack.as_ref().map(|stack| {
                // S8b: a wire-supplied provider may only name relays this
                // node's own relay map already holds (§tileset-security).
                let allowed_relays: Vec<iroh::RelayUrl> = relay_config
                    .to_relay_mode()
                    .map(|mode| mode.relay_map().urls())
                    .unwrap_or_default();
                let transport: Arc<dyn BlobTransport> = Arc::new(
                    IrohBlobTransport::from_stack(stack).with_allowed_relays(allowed_relays),
                );
                // OF-6b: every HashSeq the service publishes or resolves is
                // reported to the blob-ALPN gate (root AND children).
                let gate = stack.blob_access_gate().clone();
                Arc::new(
                    TilesetTransfer::for_p2p_dir(transport, &p2p_dir)
                        .with_content_observer(Arc::new(move |content| gate.track_content(content)))
                        .with_child_source(blob_fallback.clone()),
                )
            });
            if let Some(transfer) = tileset_transfer.clone() {
                tokio::spawn(report_and_resume_tileset_downloads(
                    transfer,
                    evt_tx.clone(),
                ));
            }

            let scene_transport = docs_stack.as_ref().map(|stack| {
                let allowed_relays: Vec<iroh::RelayUrl> = relay_config
                    .to_relay_mode()
                    .map(|mode| mode.relay_map().urls())
                    .unwrap_or_default();
                Arc::new(IrohBlobTransport::from_stack(stack).with_allowed_relays(allowed_relays))
            });
            let scene_gate=docs_stack.as_ref().map(|stack|stack.blob_access_gate().scene_assets()).unwrap_or_default();
            let (scene_publication_tx,mut scene_publication_rx)=tokio::sync::mpsc::channel(128);
            let mut scene_assets=crate::scene_assets::SceneAssetJobs::new(blob_store.clone(),scene_transport,scene_gate,db_cmd_tx.clone(),evt_tx.clone(),scene_publication_tx).with_blob_source(blob_fallback.clone());

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

            // SYNC-RELAYWATCH: continuous relay-health monitoring. The
            // endpoint's home-relay status watcher feeds a select! branch
            // below, folding every status change through
            // `RelayHealth::on_relay_status` (connected → Healthy, errors →
            // Degraded → Unreachable, auth denial → Unreachable) and emitting
            // `RelayHealthChanged` on change. Absent when relays are disabled
            // (health is the `Disabled` fixed point), in virtual mode,
            // offline, and when the P2P stack failed to spawn (the watcher
            // would otherwise overwrite that loud `Degraded` with `Healthy`
            // as soon as a relay connects — replication is still down).
            // `stream()` yields the current value first, so a relay already
            // connected during `node_addr()` confirms immediately.
            // See AGENTS.md §relay-health.
            let mut relay_watch: Option<RelayStatusStream> = endpoint
                .as_ref()
                .filter(|ep| ep.relay_enabled() && docs_stack.is_some())
                .map(|ep| Box::pin(ep.home_relay_status().stream()) as RelayStatusStream);

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
            let mut joined_leases = crate::join_ticket::JoinedReplicaLeases::default();

            // A2: per-replica inbound pump handles (verse_id -> forwarder).
            let mut inbound_pumps: HashMap<String, tokio::task::AbortHandle> = HashMap::new();

            // A2/A4: aggregated inbound row stream. Every open replica's event
            // pump forwards (verse_id, RowChange) here; the select! loop drains
            // it alongside commands.
            let (inbound_tx, mut inbound_rx) =
                tokio::sync::mpsc::channel::<(String, RowChange)>(256);

            // Phase F.2: per-petal replica map.
            let mut petal_replicas: HashMap<String, Box<dyn PetalReplicator>> = HashMap::new();

            // PAS-P3 S3: tileset_id -> known providers, learned from inbound
            // ads (authenticated ones sorted first). Replaces the dead
            // Phase-F.5 `TilesetDownloadTracker` HashMap — "received" now
            // lives in `TilesetTransfer`'s persisted ledger.
            let mut tileset_providers = TilesetProviderMap::default();
            // QA-0101: last neighbor-triggered tileset advertisement per verse (rate limit).
            let mut neighbor_ads: HashMap<String, std::time::Instant> = HashMap::new();

            // F21/M2: writes retained for verses whose replica is not open
            // yet, republished after the open (see PendingWrites).
            let mut pending_writes = PendingWrites::default();
            // QA-0120: how to open each known verse, and the replicas this
            // thread opened itself to publish a write made while closed.
            let mut publication_leases = PublicationLeases::default();
            let mut lease_tick = tokio::time::interval(std::time::Duration::from_secs(5));
            lease_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

            // M2/F6: per-verse timeseries fabric (settings from the verse
            // manifest, peer declarations, shard ledger). Sync-thread state,
            // NOT DB state — the DB thread maps `__shards`/`__peers` rows to
            // NotApplicable. See AGENTS.md §sharding.
            let mut fabrics: HashMap<String, VerseFabric> = HashMap::new();

            // S7: admitted `__relays` adverts reconciled into the relay map under the
            // LOCAL policy (never replicated); configured relays are never touched.
            let relay_advertise_policy =
                RelayAdvertisePolicy::from_env().effective_for(&relay_config);
            // NET-22: Configured / Default / Advertised tiers, coverage, the adopted-relay health loop.
            let mut relay_adoption =
                RelayAdoption::new(RelayReconciler::from_relay_config(&relay_config));
            // NET-28: cached self-test verdict and the last redacted snapshot emitted (None until a request subscribes).
            let mut diagnostics_verdict: Option<&'static str> = None;
            let mut last_diagnostics: Option<fe_runtime::net_diagnostics::NetworkDiagnostics> = None;
            let (adoption_probe_tx, mut adoption_probe_rx) =
                tokio::sync::mpsc::channel::<(String, bool)>(32);
            let mut relay_adoption_tick = tokio::time::interval(RELAY_ADOPTION_TICK);
            relay_adoption_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            // NET-21: members-only relay admission, re-pushed on every role or `__peers` change.
            #[cfg(feature = "relay-server")]
            let mut relay_admission = RelayAdmissionPusher::default();

            // M2/F7: the distributed-query transport handle shared by the
            // select loop (submit + inbound routing) and the collector /
            // responder tasks. Bounded concurrency + pending-request caps
            // live inside it.
            let transport = DistributedTransport::new(
                local_did.clone(),
                db_cmd_tx.clone(),
                evt_tx.sender(),
            )
            .with_signing_key(gossip_signing_key);
            // M2/F6: the local `__peers/{local_did}` declaration (unlimited, non-seeder);
            // NET-26: it declares the gossip latch (`gossip_sig`) only when this node actually signs.
            let local_gossip_sig = transport.signing_key().map(|_| GOSSIP_SIG_VERSION);
            let mut local_declaration = PeerDeclaration {
                gossip_sig: local_gossip_sig,
                ..PeerDeclaration::default()
            };
            // NET-26: verify-then-unwrap state for inbound gossip (mode + replay guard).
            let mut gossip_ingress = SignedGossipIngress::from_env();
            // NET-29: dial-back responder (needs a signing key) and the local request table.
            let dialback_responder = transport.signing_key().cloned().map(|secret| Arc::new(DialbackResponder::new(secret)));
            let mut dialback_requester = DialbackRequester::default();
            let mut dialback_request_rx = dialback_responder::install_request_channel();

            // crossbeam's receiver has no async readiness API — bridge the
            // command channel into a tokio channel so the select! below can
            // poll it next to the inbound pumps. Ordering is preserved (one
            // forwarder); backpressure mirrors the old blocking recv (the
            // bridge blocks while the sync thread is busy).
            let (async_cmd_tx, mut async_cmd_rx) =
                tokio::sync::mpsc::channel::<SyncCommand>(256);
            // Weak: a lease's own open/close never keeps the command loop alive.
            let lease_cmd_tx = async_cmd_tx.downgrade();
            let forwarder_stop = fe_runtime::service_lifecycle::StopSignal::default();
            let command_forwarder = {
                let cmd_rx = cmd_rx;
                let stop = forwarder_stop.clone();
                std::thread::spawn(move || {
                    while !stop.is_requested() {
                        let cmd = match cmd_rx.recv_timeout(std::time::Duration::from_millis(100)) {
                            Ok(cmd) => cmd,
                            Err(crossbeam::channel::RecvTimeoutError::Timeout) => continue,
                            Err(crossbeam::channel::RecvTimeoutError::Disconnected) => break,
                        };
                        // blocking_send parks this bridge thread until the
                        // sync thread has capacity — never called from async
                        // context, which is exactly what blocking_send wants.
                        if !cmd_rx_bridge_send(&async_cmd_tx, cmd) {
                            break;
                        }
                    }
                })
            };

            // Command loop (select! over commands + inbound replica rows +
            // inbound gossip compute traffic + home-relay status).
            loop {
                evt_tx.flush();
                // RP-5 §2.5 re-read on empowerment: drains every open
                // verse's `reread_queue` (filled whenever a DID's role
                // rose) by replaying that DID's `role`/`__shards`/
                // `__holders`/`verse` entries straight from the local
                // replica through the inbound apply path. Runs at the top
                // of the loop, not inline inside the handler that raised
                // the DID's role, so a slow drain never delays that
                // handler's own return — "the select loop drains it".
                let reread_left = drain_reread_queues(&mut fabrics, &replicas, &local_did, &evt_tx, &db_cmd_tx, &docs_stack, Some(&mut scene_assets))
                    .await;
                let rejudge_left = drain_rejudge_queues(&mut fabrics, &replicas, &db_cmd_tx).await;
                let rows_left = !resend_unsent_rows(&mut fabrics, &db_cmd_tx, &mut scene_assets);
                // NET-21: after the last select arm's inbound row and the re-read/re-judge drains above.
                #[cfg(feature = "relay-server")]
                relay_admission.refresh(&fabrics);
                // S7: after the inbound batch / re-judge above changed any admitted advert.
                reconcile_advertised_relays(
                    &mut fabrics,
                    &replicas,
                    &mut relay_adoption,
                    &relay_advertise_policy,
                    relay_health,
                    endpoint.as_ref(),
                )
                .await;
                let prefetch_left = scene_assets.pump_prefetches();
                let kept_work = reread_left || rejudge_left || rows_left || prefetch_left;
                tokio::select! {
                    // QA-0132/QA-0133: kept role work and the capped prefetch
                    // backlog are retried even when no event wakes the loop.
                    _ = tokio::time::sleep(KEPT_WORK_RETRY), if kept_work => {}
                    Some(publication)=scene_publication_rx.recv()=>{
                        if scene_assets.publication_is_current(&publication) {
                            handle_write_row_entry(&replicas,&mut pending_writes,fabrics.entry(publication.verse.clone()).or_default(),&blob_store,&local_did,&publication.verse,"asset",&publication.record_id,&publication.row_hash).await;
                        }
                    }
                    // Wakes the loop to drain a backlog queued while the
                    // channel was full (§event-outbox / self-drain rule).
                    _ = evt_tx.backlog_ready() => {}
                    _ = lease_tick.tick(), if publication_leases.any_open() => {
                        // Closed inline (not queued): a host open still in the command
                        // queue then re-opens it, and can never be closed by the lease.
                        for verse_id in publication_leases.expired(std::time::Instant::now()) {
                            tracing::info!(verse_id, "Publication lease ended; closing the replica this thread opened");
                            tileset_providers.forget_verse(&verse_id);
                            scene_assets.close(&verse_id);
                            handle_close_verse_replica(&mut replicas, &mut inbound_pumps, &verse_id).await;
                            unsubscribe_from_verse_gossip_topic(&mut gossip_senders, &mut gossip_pumps, &verse_id);
                            // Replay windows (and latches) outlive the close, so a reopen cannot replay in-age frames.
                            if let Some(stack) = &docs_stack {
                                stack.blob_access_gate().forget_verse(&verse_id);
                            }
                        }
                    }
                    _ = relay_adoption_tick.tick() => {
                        // NET-22: 30 s re-plan (advert expiry, cooled failures) and adopted-relay health checks.
                        if let Some(endpoint) = endpoint.as_ref() {
                            let statuses = endpoint.home_relay_status().get();
                            relay_adoption.observe_home(&statuses);
                            relay_adoption.check_adopted(&adoption_probe_tx, std::time::Instant::now());
                        }
                        relay_adoption.on_tick(std::time::Instant::now());
                        // NET-28: re-run the (cheap, cached-report) self-test every tick; emit the redacted snapshot when it
                        // changed after a request subscribed, or once per verdict change so Settings learns the verdict unasked.
                        let fresh_verdict = endpoint.as_ref().and_then(|ep| crate::net_diagnostics::reachability_self_test(ep.inner()));
                        let verdict_changed = fresh_verdict.is_some() && fresh_verdict != diagnostics_verdict;
                        if fresh_verdict.is_some() {
                            diagnostics_verdict = fresh_verdict;
                        }
                        if last_diagnostics.is_some() || verdict_changed {
                            let redacted = network_diagnostics_snapshot(
                                false, &relay_config, relay_health, &relay_adoption, &replicas, endpoint.as_ref(), diagnostics_verdict,
                            ).await;
                            if last_diagnostics.as_ref() != Some(&redacted) {
                                last_diagnostics = Some(redacted.clone());
                                send_sync_event(&evt_tx, SyncEvent::NetworkDiagnostics(redacted));
                            }
                        }
                        // Expiry timer: an expired advert becomes a Withdrawn change, not just a read-time skip.
                        let now_ms = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map_or(0, |elapsed| elapsed.as_millis() as u64);
                        for fabric in fabrics.values_mut() {
                            fabric.expire_relay_adverts(now_ms);
                        }
                    }
                    Some((relay, healthy)) = adoption_probe_rx.recv() => {
                        relay_adoption.record_probe(&relay, healthy, std::time::Instant::now());
                    }
                    maybe_status = next_relay_status(&mut relay_watch), if relay_watch.is_some() => {
                        match maybe_status {
                            Some(statuses) => {
                                relay_health = apply_relay_status(
                                    relay_health,
                                    &statuses,
                                    &relay_adoption.reconciler,
                                    &evt_tx,
                                );
                                // A home change re-plans Default demotion and coverage on the next pass.
                                relay_adoption.observe_home(&statuses);
                            }
                            // Watcher disconnected (last endpoint clone
                            // dropped) — stop polling it.
                            None => relay_watch = None,
                        }
                    }
                    maybe_row = inbound_rx.recv() => {
                        // `None` (all pumps closed) needs no handling —
                        // commands still flow.
                        if let Some((verse_id, change)) = maybe_row {
                            let fabric = fabrics.entry(verse_id.clone()).or_default();
                            let verdict = handle_inbound_row_change(
                                fabric,
                                &verse_id,
                                &change,
                                &local_did,
                                &evt_tx,
                                &db_cmd_tx,
                            );
                            // QA-0133: only a forwarded row from an author this
                            // node's role view admits may start a blob fetch.
                            if verdict == InboundVerdict::Forwarded {
                                if let Some(hash) = scene_asset_prefetch_hash(fabric, &verse_id, &change) {
                                    scene_assets.prefetch(&verse_id, hash);
                                }
                            }
                            sync_blob_access_gate(&docs_stack, &verse_id, fabric);
                        }
                    }
                    maybe_gossip = gossip_inbound_rx.recv() => {
                        // QA-0101: an empty frame is the pump's NeighborUp signal — a
                        // peer that joins after the last advertisement must still learn
                        // this node's tilesets (at most one re-advertisement per 5 s).
                        if let Some(incoming) = maybe_gossip.as_ref().filter(|incoming| incoming.content.is_empty()) {
                            // QA-0118: the new neighbor may serve blobs no provider could.
                            scene_assets.rearm(&incoming.verse_id);
                            let due = neighbor_ads
                                .get(&incoming.verse_id)
                                .is_none_or(|last| last.elapsed() >= std::time::Duration::from_secs(5));
                            if due && neighbor_ads.len() < 1024 {
                                neighbor_ads.insert(incoming.verse_id.clone(), std::time::Instant::now());
                                handle_advertise_tilesets(
                                    &gossip_host,
                                    &gossip_senders,
                                    &tileset_transfer,
                                    my_provider_addr.as_deref(),
                                    transport.signing_key(),
                                    &incoming.verse_id,
                                )
                                .await;
                            }
                            continue;
                        }
                        if let Some(incoming) = maybe_gossip {
                            // NET-26: verify-then-unwrap before any handler; a refused frame is
                            // final (never re-parsed as legacy). See AGENTS.md §gossip-signing.
                            let verse_id = incoming.verse_id.clone();
                            let AdmittedGossip { incoming, signed } =
                                match gossip_ingress.admit(&fabrics, incoming, unix_now_ms()) {
                                    Ok(admitted) => admitted,
                                    Err(reason) => {
                                        tracing::debug!(verse_id, reason, "gossip frame refused at the signed-frame ingress");
                                        continue;
                                    }
                                };
                            match signed {
                                Some((GossipFrameKind::Compute, author_did)) => {
                                    handle_signed_gossip_incoming(
                                        &transport,
                                        &fabrics,
                                        &gossip_senders,
                                        incoming,
                                        &author_did,
                                    );
                                    continue;
                                }
                                Some((GossipFrameKind::TilesetAd, author_did)) => {
                                    handle_signed_tileset_ads(
                                        &mut tileset_providers,
                                        &evt_tx,
                                        &incoming,
                                        &author_did,
                                    );
                                    continue;
                                }
                                Some((GossipFrameKind::ReachProbe, author_did)) => {
                                    handle_reach_probe(
                                        dialback_responder.as_ref(),
                                        &fabrics,
                                        &gossip_senders,
                                        &incoming,
                                        &author_did,
                                    );
                                    continue;
                                }
                                Some((GossipFrameKind::ReachResult, author_did)) => {
                                    handle_reach_result(&mut dialback_requester, &fabrics, &incoming, &local_did, &author_did);
                                    continue;
                                }
                                Some((kind, _)) => {
                                    tracing::debug!(verse_id, kind = kind.as_wire(), "signed gossip frame has no handler in this build");
                                    continue;
                                }
                                None => {}
                            }
                            // PAS-P3 S3: tileset ads ride the same verse
                            // topic as compute envelopes, tagged `kind`
                            // (never `type`) so the two decoders can never
                            // collide. A malformed/foreign frame falls
                            // through to `handle_gossip_incoming` unchanged.
                            match decode_tileset_ad(&incoming.content) {
                                Some(ad) => {
                                    // R1 (OF-8): an ad feeds ONLY the
                                    // provider map. It never touches the
                                    // blob access gate — a root's binding is
                                    // fixed by this node's own first fetch
                                    // decision (`bind_fetched_root`), and
                                    // gossip can neither start, remap nor
                                    // narrow it.
                                    handle_inbound_tileset_ad(
                                        &mut tileset_providers,
                                        &evt_tx,
                                        &incoming,
                                        ad,
                                    );
                                }
                                None => {
                                    handle_gossip_incoming(
                                        &transport,
                                        &fabrics,
                                        &gossip_senders,
                                        incoming,
                                    );
                                }
                            }
                        }
                    }
                    Some(request) = dialback_request_rx.recv() => {
                        broadcast_dialback_request(
                            &mut dialback_requester,
                            &gossip_senders,
                            transport.signing_key(),
                            request,
                        )
                        .await;
                    }
                    maybe_cmd = async_cmd_rx.recv() => {
                        match maybe_cmd {
                            Some(SyncCommand::GetNetworkDiagnostics { detail, reply }) => {
                                // Self-test on demand; a pending report keeps the last cached verdict.
                                if let Some(verdict) = endpoint.as_ref().and_then(|ep| crate::net_diagnostics::reachability_self_test(ep.inner())) {
                                    diagnostics_verdict = Some(verdict);
                                }
                                let snapshot = network_diagnostics_snapshot(
                                    detail, &relay_config, relay_health, &relay_adoption, &replicas, endpoint.as_ref(), diagnostics_verdict,
                                ).await;
                                // The event and status resource only ever carry the redacted form.
                                let redacted = if detail {
                                    network_diagnostics_snapshot(
                                        false, &relay_config, relay_health, &relay_adoption, &replicas, endpoint.as_ref(), diagnostics_verdict,
                                    ).await
                                } else {
                                    snapshot.clone()
                                };
                                if reply.try_send(snapshot).is_err() {
                                    tracing::debug!("Network diagnostics caller is no longer waiting");
                                }
                                if last_diagnostics.as_ref() != Some(&redacted) {
                                    last_diagnostics = Some(redacted.clone());
                                    send_sync_event(&evt_tx, SyncEvent::NetworkDiagnostics(redacted));
                                }
                            }
                            Some(SyncCommand::InspectJoinedReplica { request, reply }) => {
                                let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH);
                                let outcome = match now {
                                    Ok(now) if request.deadline > std::time::Instant::now()
                                        && request.invitation.revalidate(&local_did, now.as_secs()).is_ok() => {
                                        Ok(joined_leases.owns(&crate::join_ticket::JoinedReplicaLease::for_request(&request))
                                            && replicas.get(&request.invitation.manifest().verse_id).is_some_and(|replica|
                                            replica.joined_peer_ready(&request.invitation.manifest().namespace_id, request.invitation.issuer_did())))
                                    }
                                    _ => Err("Invitation readiness deadline or identity is invalid"),
                                };
                                if reply.try_send(crate::join_ticket::JoinReplicaReadiness { request_id: request.request_id, outcome }).is_err() {
                                    tracing::debug!("Replica readiness caller is no longer waiting");
                                }
                            }
                            Some(SyncCommand::ExportJoinTicket { request, reply }) => {
                                let ticket = if request.deadline <= std::time::Instant::now() {
                                    Err("Invitation ticket request expired")
                                } else if request.issuer_did != local_did {
                                    Err("Invitation transport issuer changed")
                                } else if let Some(replica) = replicas.get(&request.verse_id) {
                                    match tokio::time::timeout(std::time::Duration::from_secs(5), replica.export_join_ticket(
                                        &request.namespace_id, request.role, &request.issuer_did,
                                    )).await {
                                        Ok(Ok(ticket)) if request.deadline > std::time::Instant::now() => Ok(ticket),
                                        _ => Err("Native invitation ticket export is unavailable"),
                                    }
                                } else {
                                    Err("Open this Verse online before generating its invitation")
                                };
                                if reply.try_send(crate::join_ticket::JoinTicketExported {request_id: request.request_id, ticket}).is_err() {
                                    tracing::debug!("Invitation ticket caller is no longer waiting");
                                }
                            }
                            Some(SyncCommand::FetchBlob { hash, verse_id }) => {
                                handle_fetch_blob(&mut scene_assets, &hash, &verse_id);
                            }
                            Some(SyncCommand::FetchSceneAsset {hash,verse_id,request_id}) => {
                                scene_assets.fetch_owned(&verse_id,hash,Some(request_id));
                            }
                            Some(SyncCommand::RememberVerseReplica { verse_id, namespace_id, namespace_secret }) => {
                                publication_leases.remember(verse_id, namespace_id, namespace_secret, Vec::new());
                            }
                            Some(command @ (SyncCommand::OpenVerseReplica { .. } | SyncCommand::JoinVerseReplica { .. })) => {
                                if let SyncCommand::OpenVerseReplica { verse_id, namespace_id, namespace_secret, bootstrap_peers } = &command {
                                    publication_leases.opened(verse_id, namespace_id, namespace_secret, bootstrap_peers);
                                }
                                let (verse_id, namespace_id, namespace_secret, cmd_peers, join_reply) = match command {
                                    SyncCommand::OpenVerseReplica { verse_id, namespace_id, namespace_secret, bootstrap_peers } =>
                                        (verse_id, namespace_id, namespace_secret, bootstrap_peers, None),
                                    SyncCommand::JoinVerseReplica { request, reply } => {
                                        match prepare_join_replica(&replicas, &replica_transport, &request, &local_did).await {
                                            Ok(peers) => (request.invitation.manifest().verse_id.clone(),
                                                request.invitation.manifest().namespace_id.clone(), None, peers,
                                                Some((request.request_id, request.deadline, request.invitation.expires_at(), reply))),
                                            Err(reason) => {
                                                if reply.try_send(crate::join_ticket::JoinReplicaOpened { request_id: request.request_id, outcome: Err(reason) }).is_err() {
                                                    tracing::debug!("Replica join caller is no longer waiting");
                                                }
                                                continue;
                                            }
                                        }
                                    }
                                    _ => unreachable!(),
                                };
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
                                // QA-0086: last-synced peers of this verse, so a
                                // relay-less restart can dial them.
                                if endpoint.is_some() {
                                    crate::peer_book::merge(&mut peers, crate::peer_book::load(&p2p_dir, &verse_id));
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
                                )
                                .await;
                                joined_leases.forget(&verse_id);
                                scene_assets.close(&verse_id);
                                let opened = handle_open_verse_replica(
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
                                if let Some((request_id, deadline, expires_at, reply)) = join_reply {
                                    let unexpired = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).is_ok_and(|now| now.as_secs() < expires_at);
                                    let outcome = if opened.native_ready && deadline > std::time::Instant::now() && unexpired {
                                        joined_leases.opened(crate::join_ticket::JoinedReplicaLease {request_id:request_id.clone(),verse_id:verse_id.clone(),namespace_id:namespace_id.clone()});
                                        Ok(())
                                    } else {
                                        handle_close_verse_replica(&mut replicas, &mut inbound_pumps, &verse_id).await;
                                        Err("Replica connection is unconfirmed; invitation admission may already be stored")
                                    };
                                    if reply.try_send(crate::join_ticket::JoinReplicaOpened { request_id, outcome }).is_err() {
                                        tracing::debug!("Replica join caller is no longer waiting");
                                    }
                                }
                                if opened.native_ready {
                                    if let Some(replica) = replicas.get(&verse_id) {
                                        scene_assets.opened_with_roster(&verse_id, &peers, replica.scene_provider_roster());
                                        // QA-0118: blobs of metadata already in the replica fetch now, not at petal entry.
                                        for hash in &opened.scene_asset_hashes {
                                            scene_assets.prefetch(&verse_id, *hash);
                                        }
                                        if let (Some(endpoint), Some(roster)) = (endpoint.as_ref(), replica.scene_provider_roster()) {
                                            crate::peer_book::spawn_recorder(endpoint.inner().clone(), roster, p2p_dir.clone(), verse_id.clone());
                                        }
                                    }
                                }
                                sync_blob_access_gate(
                                    &docs_stack,
                                    &verse_id,
                                    fabrics.entry(verse_id.clone()).or_default(),
                                );
                            }
                            Some(command @ (SyncCommand::CloseVerseReplica { .. } | SyncCommand::CloseJoinedReplica { .. })) => {
                                let verse_id = match command {
                                    SyncCommand::CloseJoinedReplica {lease} => {
                                        if !joined_leases.take(&lease) {continue;}
                                        lease.verse_id
                                    }
                                    SyncCommand::CloseVerseReplica {verse_id} => {joined_leases.forget(&verse_id);verse_id}
                                    _ => unreachable!(),
                                };
                                // S8b verse binding: ads learned on a verse
                                // stop counting once this node leaves it.
                                tileset_providers.forget_verse(&verse_id);
                                scene_assets.close(&verse_id);
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
                                // Replay windows (and latches) outlive the close, so a reopen cannot replay in-age frames.
                                // RP-5 step 6 / OF-6b: the blob-ALPN filter
                                // drops this verse's access snapshot — a
                                // closed verse's role view is no longer
                                // authoritative — but KEEPS its tileset
                                // roots tracked, so they deny rather than
                                // fall back to the untracked "admit" path.
                                if let Some(stack) = &docs_stack {
                                    stack.blob_access_gate().forget_verse(&verse_id);
                                }
                            }
                            Some(SyncCommand::WriteRowEntry {
                                verse_id,
                                table,
                                record_id,
                                content_hash,
                            }) => {
                                if table=="asset" {
                                    match crate::scene_assets::is_scene_row(&blob_store,content_hash).await {
                                        Ok(true)=>{
                                            // QA-0164: a node publication re-sends its unchanged asset row, whose
                                            // blob and row were published already (AGENTS.md §unchanged-rewrite).
                                            let published=match replicas.get(&verse_id) {
                                                Some(replica)=>replica.holds_own_entry(&table,&record_id,&content_hash).await,
                                                None=>false,
                                            };
                                            if !published {scene_assets.publish_row(&verse_id,content_hash);}
                                            continue;
                                        }
                                        Ok(false)=>{},
                                        Err(error)=>{tracing::warn!(verse_id,"asset metadata publication deferred: {error:#}");continue;}
                                    }
                                }
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
                                // QA-0120: a write for a closed verse (a revoke from the verse
                                // list) opens a bounded publication lease instead of waiting
                                // for the next visit; a write under a live lease renews it.
                                if let Some(open) = publication_leases.write(&verse_id, replicas.contains_key(&verse_id), std::time::Instant::now()) {
                                    tracing::info!(verse_id, "Write for a closed verse replica — opening a publication lease");
                                    send_lease_command(&lease_cmd_tx, open);
                                }
                                sync_blob_access_gate(
                                    &docs_stack,
                                    &verse_id,
                                    fabrics.entry(verse_id.clone()).or_default(),
                                );
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
                            Some(SyncCommand::AdvertiseTilesets { verse_id, advertisements_json: _ }) => {
                                handle_advertise_tilesets(
                                    &gossip_host,
                                    &gossip_senders,
                                    &tileset_transfer,
                                    my_provider_addr.as_deref(),
                                    transport.signing_key(),
                                    &verse_id,
                                )
                                .await;
                            }
                            Some(SyncCommand::PublishTileset { tileset_id, index_path, bundle_paths }) => {
                                spawn_publish_tileset(
                                    tileset_transfer.clone(),
                                    evt_tx.clone(),
                                    gossip_senders.clone(),
                                    my_provider_addr.clone(),
                                    transport.signing_key().cloned(),
                                    tileset_id,
                                    index_path,
                                    bundle_paths,
                                );
                            }
                            Some(SyncCommand::UnpublishTileset { tileset_id }) => {
                                spawn_unpublish_tileset(tileset_transfer.clone(), tileset_id);
                            }
                            Some(SyncCommand::DownloadTileset { tileset_id, provider }) => {
                                handle_download_tileset(
                                    &tileset_transfer,
                                    &tileset_providers,
                                    docs_stack.as_ref().map(|s| s.blob_access_gate()),
                                    tileset_id,
                                    provider,
                                    &evt_tx,
                                );
                            }
                            Some(SyncCommand::RequestTilesetMeta { peer_id, tileset_id }) => {
                                handle_request_tileset_meta(
                                    &tileset_transfer,
                                    &tileset_providers,
                                    docs_stack.as_ref().map(|s| s.blob_access_gate()),
                                    &peer_id,
                                    &tileset_id,
                                    &evt_tx,
                                );
                            }
                            Some(SyncCommand::RequestChunk { peer_id: _, tileset_id, chunk_seq, provider: _ }) => {
                                // `provider` is accepted on the wire for S7
                                // (single streaming bundles) but not yet
                                // consulted here — see `handle_request_chunk`'s
                                // doc comment.
                                handle_request_chunk(&tileset_transfer, &tileset_id, chunk_seq, &evt_tx);
                            }
                            Some(SyncCommand::CancelTilesetDownload { tileset_id }) => {
                                handle_cancel_tileset_download(&tileset_transfer, tileset_id);
                            }
                            Some(SyncCommand::ConfirmTilesetDownload {
                                tileset_id,
                                expected_chunk_hashes,
                            }) => {
                                handle_confirm_tileset_download(
                                    &tileset_transfer,
                                    tileset_id,
                                    expected_chunk_hashes,
                                    &evt_tx,
                                );
                            }
                            Some(SyncCommand::SetShardDeclaration {
                                capacity_bytes,
                                seeder,
                            }) => {
                                local_declaration = PeerDeclaration {
                                    capacity_bytes,
                                    seeder,
                                    gossip_sig: local_gossip_sig,
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

            forwarder_stop.request();
            async_cmd_rx.close();
            drop(async_cmd_rx);
            if command_forwarder.join().is_err() {
                tracing::error!("Sync command forwarder panicked during shutdown");
            }

            scene_assets.shutdown().await;

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
            evt_tx.flush_before_exit(SHUTDOWN_FLUSH_TIMEOUT).await;
        });
    })
}

/// The home-relay status stream the select loop watches (SYNC-RELAYWATCH).
type RelayStatusStream =
    std::pin::Pin<Box<dyn futures_lite::Stream<Item = Vec<iroh::endpoint::RelayStatus>> + Send>>;

/// Next home-relay status snapshot; `None` once the watcher disconnects.
/// Only polled while `watch` is `Some` (the select! branch precondition).
async fn next_relay_status(
    watch: &mut Option<RelayStatusStream>,
) -> Option<Vec<iroh::endpoint::RelayStatus>> {
    use futures_lite::StreamExt;
    watch.as_mut()?.next().await
}

/// Fold one home-relay status snapshot into the relay health, logging and
/// emitting [`SyncEvent::RelayHealthChanged`] only when the value changes
/// (see AGENTS.md §relay-health).
fn apply_relay_status(
    current: RelayHealth,
    statuses: &[iroh::endpoint::RelayStatus],
    reconciler: &RelayReconciler,
    evt_tx: &dyn EventSink,
) -> RelayHealth {
    let mut summaries: Vec<RelayStatusSummary> = Vec::with_capacity(statuses.len());
    for status in statuses {
        let mut summary = RelayStatusSummary::from_status(status);
        let advertised = summary.auth_denied && reconciler.is_inserted(status.url().as_str());
        if advertised {
            // OD-10: an advertised relay never gets the configured token; a denial is a
            // plain connection error (Degraded), never a retry with credentials.
            summary.auth_denied = false;
            summary.has_error = true;
            if current.is_problem() {
                tracing::debug!(relay = %status.url(), "advertised relay still requires authentication");
            } else {
                tracing::warn!(
                    relay = %status.url(),
                    "Advertised relay requires authentication; it is not retried with the configured relay token"
                );
            }
        }
        summaries.push(summary);
        if advertised {
            continue;
        }
        if let Some(reason) = status.auth_denied_reason() {
            // Logged once per descent into `Unreachable`, not on every
            // retry snapshot iroh publishes while the denial persists.
            if current == RelayHealth::Unreachable {
                tracing::debug!(relay = %status.url(), reason, "relay still denies our authentication");
                continue;
            }
            tracing::error!(
                relay = %status.url(),
                reason,
                "Relay server denied our authentication — it will not recover on its own"
            );
        } else if let Some(err) = status.last_error() {
            tracing::debug!(relay = %status.url(), "home relay connection error: {err}");
        }
    }
    fold_relay_health(current, &summaries, evt_tx)
}

/// The emit-on-change half of [`apply_relay_status`], over summaries (the
/// seam the SYNC-RELAYWATCH test drives — upstream `RelayStatus` has no
/// public constructor).
fn fold_relay_health(
    current: RelayHealth,
    summaries: &[RelayStatusSummary],
    evt_tx: &dyn EventSink,
) -> RelayHealth {
    let next = current.on_relay_status(summaries);
    if next != current {
        if next.is_problem() {
            tracing::warn!(from = ?current, to = ?next, "Relay health degraded (home-relay watcher)");
        } else {
            tracing::info!(from = ?current, to = ?next, "Relay health changed (home-relay watcher)");
        }
        send_sync_event(evt_tx, SyncEvent::RelayHealthChanged { health: next });
    }
    next
}

/// Feeds the embedded relay's membership gate (NET-21); see `src/AGENTS.md` §peer-relay-server.
#[cfg(feature = "relay-server")]
#[derive(Debug, Default)]
struct RelayAdmissionPusher {
    /// Last seen `relay_admission_epoch` per fabric; unchanged epochs skip the recompute.
    seen_epochs: HashMap<String, u64>,
    /// The snapshot last pushed to the gate.
    last_pushed: Option<crate::sharding::RelayAdmissionSnapshot>,
}

#[cfg(feature = "relay-server")]
impl RelayAdmissionPusher {
    /// Pushes a fresh snapshot after any role or `__peers` change, then revokes every DID that lost membership.
    fn refresh(&mut self, fabrics: &HashMap<String, VerseFabric>) {
        let unchanged = self.seen_epochs.len() == fabrics.len()
            && fabrics.iter().all(|(verse_id, fabric)| {
                self.seen_epochs.get(verse_id) == Some(&fabric.relay_admission_epoch())
            });
        if unchanged {
            return;
        }
        self.seen_epochs = fabrics
            .iter()
            .map(|(verse_id, fabric)| (verse_id.clone(), fabric.relay_admission_epoch()))
            .collect();
        // Hosted = every verse this thread holds fabric state for; the gate narrows to served verses.
        let hosted: BTreeSet<String> = fabrics.keys().cloned().collect();
        let snapshot = crate::sharding::relay_admission_snapshot(fabrics, &hosted);
        if self.last_pushed.as_ref() == Some(&snapshot) {
            return;
        }
        let lost: Vec<String> = match &self.last_pushed {
            Some(previous) => {
                let admitted = snapshot.admitted_dids();
                previous
                    .admitted_dids()
                    .into_iter()
                    .filter(|did| !admitted.contains(did))
                    .collect()
            }
            None => Vec::new(),
        };
        crate::relay_server::push_snapshot(snapshot.clone());
        // After the snapshot: a revoke stays in force until a later snapshot re-admits the DID.
        for did in &lost {
            crate::relay_server::push_revoke(did);
        }
        if !lost.is_empty() {
            tracing::info!(revoked = lost.len(), "relay admission lost members; revoked at the gate");
        }
        self.last_pushed = Some(snapshot);
    }
}

/// How often the relay planner re-runs without another trigger (advert expiry, cooled failures, health checks).
const RELAY_ADOPTION_TICK: std::time::Duration = std::time::Duration::from_secs(30);

/// Consecutive failed health checks after which an adopted (advertised) relay is removed.
const ADOPTED_RELAY_FAILURE_LIMIT: u32 = 3;

/// How long a removed unhealthy relay stays out before it may be adopted again.
const ADOPTED_RELAY_FAILED_COOLDOWN: std::time::Duration = std::time::Duration::from_secs(10 * 60);

/// Budget for one adopted-relay reachability probe (TCP connect to a resolved, public address).
const ADOPTED_RELAY_PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Consecutive-failure counter for adopted relays; a relay that fails `ADOPTED_RELAY_FAILURE_LIMIT` checks in a row is held out for a cooldown.
#[derive(Debug, Default)]
struct AdoptedRelayHealth {
    failures: HashMap<String, u32>,
    failed_until: HashMap<String, std::time::Instant>,
}

impl AdoptedRelayHealth {
    /// Fold one check result; `true` when this check marked the relay failed.
    fn record(&mut self, url: &str, healthy: bool, now: std::time::Instant) -> bool {
        if self.failed_until.contains_key(url) {
            return false;
        }
        if healthy {
            self.failures.remove(url);
            return false;
        }
        let count = self.failures.entry(url.to_string()).or_default();
        *count += 1;
        if *count < ADOPTED_RELAY_FAILURE_LIMIT {
            return false;
        }
        self.failures.remove(url);
        self.failed_until
            .insert(url.to_string(), now + ADOPTED_RELAY_FAILED_COOLDOWN);
        true
    }

    fn is_failed(&self, url: &str) -> bool {
        self.failed_until.contains_key(url)
    }

    /// Release relays whose cooldown ended; `true` when any was released.
    fn release_cooled(&mut self, now: std::time::Instant) -> bool {
        let before = self.failed_until.len();
        self.failed_until.retain(|_, until| *until > now);
        before != self.failed_until.len()
    }
}

/// The sync thread's relay adoption state (local only, never replicated); see `src/AGENTS.md` §relay-trust.
struct RelayAdoption {
    reconciler: RelayReconciler,
    /// Participating verses = replicas open at the last plan.
    open_verses_seen: BTreeSet<String>,
    /// Latest home-relay snapshot as (canonical URL, summary).
    home: Vec<(String, RelayStatusSummary)>,
    health: AdoptedRelayHealth,
    /// Adopted relays with a reachability probe in flight.
    probing: HashSet<String>,
    /// Default-tier entries taken out of the relay map while demoted, restored verbatim.
    removed_defaults: HashMap<String, Arc<iroh::RelayConfig>>,
    /// Relay allowlist last pushed to the endpoint.
    last_allowlist: BTreeSet<String>,
    /// Set by the tick, a home change or a health verdict: re-plan on the next pass.
    replan: bool,
    /// Hazard B (AC-9): open verses whose adverts omit this node's advertised home relay (NET-28 diagnostics).
    unadvertised_home_verses: BTreeSet<String>,
    /// DialOnly URL -> (resolves to public addresses, checked at).
    dial_only_verdicts: HashMap<String, (bool, std::time::Instant)>,
}

impl RelayAdoption {
    fn new(reconciler: RelayReconciler) -> Self {
        Self {
            last_allowlist: reconciler
                .configured()
                .union(reconciler.defaults())
                .cloned()
                .collect(),
            reconciler,
            open_verses_seen: BTreeSet::new(),
            home: Vec::new(),
            health: AdoptedRelayHealth::default(),
            probing: HashSet::new(),
            removed_defaults: HashMap::new(),
            replan: true,
            unadvertised_home_verses: BTreeSet::new(),
            dial_only_verdicts: HashMap::new(),
        }
    }

    /// Record a home-relay snapshot; a change triggers a re-plan.
    fn observe_home(&mut self, statuses: &[iroh::endpoint::RelayStatus]) {
        let changed = statuses.len() != self.home.len()
            || statuses.iter().zip(&self.home).any(|(status, (url, summary))| {
                status.url().as_str() != url.as_str() || RelayStatusSummary::from_status(status) != *summary
            });
        if changed {
            self.home = statuses
                .iter()
                .map(|status| (status.url().to_string(), RelayStatusSummary::from_status(status)))
                .collect();
            self.replan = true;
        }
    }

    /// The home relay iroh reports as connected, if any.
    fn home_connected(&self) -> Option<String> {
        self.home
            .iter()
            .find(|(_, summary)| summary.connected)
            .map(|(url, _)| url.clone())
    }

    /// One health check per adopted advertised relay: home status when it is a home relay, else a reachability probe.
    fn check_adopted(
        &mut self,
        probe_tx: &tokio::sync::mpsc::Sender<(String, bool)>,
        now: std::time::Instant,
    ) {
        let adopted: BTreeSet<String> = self
            .reconciler
            .inserted()
            .keys()
            .filter(|url| self.reconciler.source_of(url) == Some(RelaySource::Advertised))
            .cloned()
            .collect();
        self.health.failures.retain(|url, _| adopted.contains(url));
        for url in adopted {
            let home_verdict = self
                .home
                .iter()
                .find(|(home_url, _)| *home_url == url)
                .and_then(|(_, summary)| {
                    if summary.connected {
                        Some(true)
                    } else if summary.has_error || summary.auth_denied {
                        Some(false)
                    } else {
                        None
                    }
                });
            match home_verdict {
                Some(healthy) => self.record_check(&url, healthy, now),
                None if self.probing.insert(url.clone()) => {
                    let probe_tx = probe_tx.clone();
                    tokio::spawn(async move {
                        let healthy = probe_adopted_relay(&url).await;
                        if probe_tx.send((url, healthy)).await.is_err() {
                            tracing::warn!("adopted relay probe result dropped: the sync loop has stopped");
                        }
                    });
                }
                None => {}
            }
        }
    }

    /// Fold a finished reachability probe.
    fn record_probe(&mut self, url: &str, healthy: bool, now: std::time::Instant) {
        self.probing.remove(url);
        if self.reconciler.is_inserted(url) {
            self.record_check(url, healthy, now);
        }
    }

    fn record_check(&mut self, url: &str, healthy: bool, now: std::time::Instant) {
        if self.health.record(url, healthy, now) {
            tracing::warn!(
                relay = %url,
                failures = ADOPTED_RELAY_FAILURE_LIMIT,
                "adopted relay failed consecutive health checks; removing it from the relay map"
            );
            self.replan = true;
        }
    }

    /// The periodic tick always re-plans (advert expiry is judged at read time).
    fn on_tick(&mut self, now: std::time::Instant) {
        self.health.release_cooled(now);
        self.replan = true;
    }

    /// Relays a peer address may name: Configured, Default and inserted, plus DialOnly URLs whose resolved addresses pass the advertised-relay address policy.
    async fn allowlist(&mut self) -> BTreeSet<String> {
        let now = std::time::Instant::now();
        let mut allowed: BTreeSet<String> = self
            .reconciler
            .configured()
            .iter()
            .chain(self.reconciler.defaults())
            .chain(self.reconciler.inserted().keys())
            .cloned()
            .collect();
        let dial_only: Vec<String> = self
            .reconciler
            .known()
            .iter()
            .filter(|url| !allowed.contains(*url))
            .cloned()
            .collect();
        let mut pending: Vec<String> = dial_only
            .iter()
            .filter(|url| !verdict_is_fresh(&self.dial_only_verdicts, url, now))
            .cloned()
            .collect();
        pending.truncate(DIAL_ONLY_RESOLVE_LIMIT);
        let mut resolutions: HashMap<String, Result<(), String>> = HashMap::new();
        let mut lookups = tokio::task::JoinSet::new();
        for url in pending {
            lookups.spawn(async move {
                let verdict = dial_only_relay_allowed(&url).await;
                (url, verdict)
            });
        }
        while let Some(joined) = lookups.join_next().await {
            match joined {
                Ok((url, verdict)) => {
                    resolutions.insert(url, verdict);
                }
                Err(error) => tracing::warn!("dial-only relay lookup task failed: {error}"),
            }
        }
        // URLs past the per-pass limit are neither cached nor allowlisted this pass.
        let considered: Vec<&String> = dial_only
            .iter()
            .filter(|url| {
                resolutions.contains_key(*url) || verdict_is_fresh(&self.dial_only_verdicts, url, now)
            })
            .collect();
        allowed.extend(filter_resolvable(
            considered,
            &mut self.dial_only_verdicts,
            now,
            |url| {
                resolutions
                    .get(url)
                    .cloned()
                    .unwrap_or_else(|| Err("relay lookup did not complete".to_string()))
            },
        ));
        allowed
    }
}

/// Most DialOnly URLs resolved per reconcile pass.
const DIAL_ONLY_RESOLVE_LIMIT: usize = 8;

/// How long a DialOnly resolution verdict is trusted before it is re-resolved.
const DIAL_ONLY_VERDICT_TTL: std::time::Duration = std::time::Duration::from_secs(10 * 60);

/// Drop relays sitting out their failure cooldown from every verse's advertised set before planning.
fn drop_failed_relays(health: &AdoptedRelayHealth, advertised: &mut BTreeMap<String, BTreeSet<String>>) {
    for urls in advertised.values_mut() {
        urls.retain(|url| !health.is_failed(url));
    }
}

/// A cached verdict younger than the TTL.
fn verdict_is_fresh(
    cache: &HashMap<String, (bool, std::time::Instant)>,
    url: &str,
    now: std::time::Instant,
) -> bool {
    cache
        .get(url)
        .is_some_and(|(_, checked)| now.saturating_duration_since(*checked) < DIAL_ONLY_VERDICT_TTL)
}

/// Keep the URLs whose verdict is Ok: fresh cache entries are reused, stale or missing ones go through `resolver`; entries for other URLs are dropped.
fn filter_resolvable<'a>(
    urls: impl IntoIterator<Item = &'a String>,
    cache: &mut HashMap<String, (bool, std::time::Instant)>,
    now: std::time::Instant,
    mut resolver: impl FnMut(&str) -> Result<(), String>,
) -> BTreeSet<String> {
    let urls: Vec<&String> = urls.into_iter().collect();
    cache.retain(|url, _| urls.iter().any(|kept| *kept == url));
    let mut kept = BTreeSet::new();
    for url in urls {
        if !verdict_is_fresh(cache, url, now) {
            let verdict = resolver(url);
            if let Err(reason) = &verdict {
                tracing::warn!(relay = %url, "dial-only relay not allowlisted: {reason}");
            }
            cache.insert(url.clone(), (verdict.is_ok(), now));
        }
        if cache.get(url).is_some_and(|(resolvable, _)| *resolvable) {
            kept.insert(url.clone());
        }
    }
    kept
}

/// Builds the diagnostics snapshot from live endpoint paths and the relay adoption state (see `AGENTS.md` §net-diagnostics).
async fn network_diagnostics_snapshot(
    detail: bool,
    relay_config: &RelayConfig,
    relay_health: RelayHealth,
    adoption: &RelayAdoption,
    replicas: &HashMap<String, Box<dyn VerseReplicator>>,
    endpoint: Option<&SyncEndpoint>,
    verdict: Option<&'static str>,
) -> fe_runtime::net_diagnostics::NetworkDiagnostics {
    let mut peers = Vec::new();
    if let Some(endpoint) = endpoint {
        let mut shared: BTreeMap<String, (iroh::EndpointId, Vec<String>)> = BTreeMap::new();
        for (verse_id, replica) in replicas {
            let Some(roster) = replica.scene_provider_roster() else { continue };
            let Ok(ids) = roster.peers() else { continue };
            for id in ids {
                shared.entry(id.to_string()).or_insert_with(|| (id, Vec::new())).1.push(verse_id.clone());
            }
        }
        for (_, verse_ids) in shared.values_mut() {
            verse_ids.sort();
        }
        peers = crate::net_diagnostics::collect_peer_rows(endpoint.inner(), shared.into_values().collect(), detail).await;
    }
    crate::net_diagnostics::build_snapshot(crate::net_diagnostics::SnapshotInputs {
        relay_mode: crate::net_diagnostics::relay_mode_label(relay_config),
        home_relay: adoption
            .home_connected()
            .map(|url| fe_runtime::net_diagnostics::redact_relay_url(&url, detail)),
        relay_health,
        portmapper: "unknown",
        reachability_verdict: verdict,
        relay_server_state: if cfg!(feature = "relay-server") { "available" } else { "not-built" },
        unadvertised_home_verses: &adoption.unadvertised_home_verses,
        peers,
    })
}

/// Relay reconciliation (S7, NET-22): re-plan the three relay tiers when adverts, open verses, home status or health changed, then apply.
async fn reconcile_advertised_relays(
    fabrics: &mut HashMap<String, VerseFabric>,
    replicas: &HashMap<String, Box<dyn VerseReplicator>>,
    adoption: &mut RelayAdoption,
    policy: &RelayAdvertisePolicy,
    home_health: RelayHealth,
    endpoint: Option<&SyncEndpoint>,
) {
    // Compared as a set: a close and an open in one pass leave the count unchanged.
    let mut changed = adoption.replan
        || replicas.len() != adoption.open_verses_seen.len()
        || replicas.keys().any(|verse_id| !adoption.open_verses_seen.contains(verse_id));
    for fabric in fabrics.values_mut() {
        changed |= !fabric.drain_relay_advert_changes().is_empty();
    }
    if !changed {
        return;
    }
    adoption.replan = false;
    adoption.open_verses_seen = replicas.keys().cloned().collect();

    // Only verses this node still has open count toward a relay's refcount; failed relays sit out their cooldown.
    let mut advertised: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for fabric in fabrics.values() {
        for (verse_id, urls) in fabric.advertised_relay_urls() {
            if replicas.contains_key(&verse_id) {
                advertised.entry(verse_id).or_default().extend(urls);
            }
        }
    }
    drop_failed_relays(&adoption.health, &mut advertised);

    let coverage = evaluate_coverage(&adoption.open_verses_seen, &advertised);
    let home_connected = adoption.home_connected();
    let ops = adoption.reconciler.plan_tiers(&RelayReconcileInputs {
        policy,
        advertised: &advertised,
        home_connected: home_connected.clone(),
        coverage,
        auto_home: auto_home_relay(),
        home_health,
    });
    let mut known = 0usize;
    for op in ops {
        match op {
            RelayOp::Known(_) => known += 1,
            RelayOp::Insert(raw) => insert_planned_relay(adoption, &raw, endpoint).await,
            RelayOp::Remove(raw) => remove_planned_relay(adoption, &raw, endpoint).await,
        }
    }

    if let Some(endpoint) = endpoint {
        let allowlist = adoption.allowlist().await;
        if allowlist != adoption.last_allowlist {
            endpoint.set_relay_allowlist(
                allowlist
                    .iter()
                    .filter_map(|raw| raw.parse::<iroh::RelayUrl>().ok()),
            );
            adoption.last_allowlist = allowlist;
        }
    }

    // Hazard B (AC-9): an advertised home relay that a shared verse does not advertise leaves that verse's peers unable to find us there.
    let unadvertised: BTreeSet<String> = match home_connected.as_deref() {
        Some(home) if adoption.reconciler.source_of(home) == Some(RelaySource::Advertised) => adoption
            .open_verses_seen
            .iter()
            .filter(|verse_id| !advertised.get(*verse_id).is_some_and(|urls| urls.contains(home)))
            .cloned()
            .collect(),
        _ => BTreeSet::new(),
    };
    for verse_id in unadvertised.difference(&adoption.unadvertised_home_verses) {
        tracing::warn!(
            verse_id = %verse_id,
            home_relay = home_connected.as_deref().unwrap_or_default(),
            "home relay is an advertised relay this verse does not advertise; its peers may not reach this node through it"
        );
    }
    adoption.unadvertised_home_verses = unadvertised;

    tracing::debug!(
        known,
        inserted = adoption.reconciler.inserted().len(),
        defaults_demoted = adoption.reconciler.defaults_demoted(),
        unadvertised_home_verses = adoption.unadvertised_home_verses.len(),
        "relay tiers reconciled"
    );
}

/// Apply one planned insert: a demoted Default relay is restored verbatim, an advertised one enters tokenless after the public-address check.
async fn insert_planned_relay(adoption: &mut RelayAdoption, raw: &str, endpoint: Option<&SyncEndpoint>) {
    let Some(endpoint) = endpoint else {
        adoption.reconciler.forget_insert(raw);
        return;
    };
    let url = match raw.parse::<iroh::RelayUrl>() {
        Ok(url) => url,
        Err(error) => {
            tracing::warn!(relay = %raw, "relay not inserted: {error}");
            adoption.reconciler.forget_insert(raw);
            return;
        }
    };
    if adoption.reconciler.source_of(raw) == Some(RelaySource::Default) {
        let entry = adoption
            .removed_defaults
            .remove(raw)
            .unwrap_or_else(|| Arc::new(iroh::RelayConfig::new(url.clone(), None)));
        endpoint.inner().insert_relay(url, entry).await;
        tracing::info!(relay = %raw, "restored default relay to the relay map");
        return;
    }
    if let Err(reason) = advertised_relay_insertable(raw).await {
        tracing::warn!(relay = %raw, "advertised relay not inserted: {reason}");
        adoption.reconciler.forget_insert(raw);
        return;
    }
    let entry = advertised_relay_entry(url.clone());
    if let Some(replaced) = endpoint.inner().insert_relay(url.clone(), entry).await {
        // Never own an entry someone else put in the map (OD-10: it may carry a token); put it back.
        tracing::error!(
            relay = %url,
            token_bearing = replaced.auth_token.is_some(),
            "advertised relay collided with an existing relay entry; restoring it"
        );
        endpoint.inner().insert_relay(url, replaced).await;
        adoption.reconciler.forget_insert(raw);
        return;
    }
    let mode = adoption.reconciler.adoption().get(raw).copied();
    tracing::info!(relay = %raw, ?mode, "inserted advertised relay into the relay map");
}

/// Apply one planned removal; a Configured relay is never removed and a demoted Default entry is kept for its restore.
async fn remove_planned_relay(adoption: &mut RelayAdoption, raw: &str, endpoint: Option<&SyncEndpoint>) {
    let source = adoption.reconciler.source_of(raw);
    if source == Some(RelaySource::Configured) {
        tracing::error!(relay = %raw, "refusing to remove a configured relay");
        return;
    }
    let Some(endpoint) = endpoint else { return };
    let url = match raw.parse::<iroh::RelayUrl>() {
        Ok(url) => url,
        Err(error) => {
            tracing::warn!(relay = %raw, "relay not removed: {error}");
            return;
        }
    };
    let removed = endpoint.inner().remove_relay(&url).await;
    if source == Some(RelaySource::Default) {
        if let Some(entry) = removed {
            adoption.removed_defaults.insert(raw.to_string(), entry);
        }
        tracing::info!(relay = %raw, "demoted default relay: a fully covered advertised relay is the healthy home");
    } else {
        tracing::info!(relay = %raw, "removed advertised relay: no longer advertised, covered or healthy");
    }
}

/// Budget for resolving a pinned advertised relay's host before it enters the relay map.
const ADVERTISED_RELAY_RESOLVE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Resolve an advertised relay to its addresses, refusing anything but a public https relay whose every address is public.
async fn resolve_public_relay(raw: &str) -> Result<Vec<std::net::SocketAddr>, String> {
    use fe_runtime::relay_advertisement::{
        check_resolved_relay_address, validate_advertised_relay_url, AdvertisedRelayPolicy,
        RelayReach,
    };
    let advertised = validate_advertised_relay_url(raw).map_err(|error| error.to_string())?;
    if !advertised.home_eligible() {
        return Err("only a public https relay may enter the relay map".to_string());
    }
    let port = advertised.url().port_or_known_default().unwrap_or(443);
    // IP-literal hosts were classified by the validator; a domain is checked at resolution.
    let Some(domain) = advertised.url().domain() else {
        let host = advertised
            .url()
            .host_str()
            .unwrap_or_default()
            .trim_start_matches('[')
            .trim_end_matches(']');
        let address: std::net::IpAddr = host
            .parse()
            .map_err(|error| format!("relay host is not an address: {error}"))?;
        return Ok(vec![std::net::SocketAddr::new(address, port)]);
    };
    let resolved = tokio::time::timeout(
        ADVERTISED_RELAY_RESOLVE_TIMEOUT,
        tokio::net::lookup_host((domain, port)),
    )
    .await
    .map_err(|_| "host resolution timed out".to_string())?
    .map_err(|error| format!("host resolution failed: {error}"))?;
    let mut addresses = Vec::new();
    for address in resolved {
        match check_resolved_relay_address(address.ip(), AdvertisedRelayPolicy::process()) {
            Ok(RelayReach::Public) => addresses.push(address),
            Ok(RelayReach::Private) => {
                return Err(format!("host resolves to non-public address {}", address.ip()))
            }
            Err(error) => return Err(error.to_string()),
        }
    }
    if addresses.is_empty() {
        Err("host resolved to no address".to_string())
    } else {
        Ok(addresses)
    }
}

/// A pinned or home-eligible advert may enter the relay map only as a public https relay whose every resolved address is public.
async fn advertised_relay_insertable(raw: &str) -> Result<(), String> {
    resolve_public_relay(raw).await.map(|_| ())
}

/// A DialOnly advert may join the peer-address allowlist when every resolved address passes the process address policy (private reach only under the operator opt-in, loopback and link-local never).
async fn dial_only_relay_allowed(raw: &str) -> Result<(), String> {
    use fe_runtime::relay_advertisement::{
        check_resolved_relay_address, validate_advertised_relay_url, AdvertisedRelayPolicy,
    };
    let advertised = validate_advertised_relay_url(raw).map_err(|error| error.to_string())?;
    // IP-literal hosts were classified by the validator under the same policy.
    let Some(domain) = advertised.url().domain() else {
        return Ok(());
    };
    let port = advertised.url().port_or_known_default().unwrap_or(443);
    let resolved = tokio::time::timeout(
        ADVERTISED_RELAY_RESOLVE_TIMEOUT,
        tokio::net::lookup_host((domain, port)),
    )
    .await
    .map_err(|_| "host resolution timed out".to_string())?
    .map_err(|error| format!("host resolution failed: {error}"))?;
    let policy = AdvertisedRelayPolicy::process();
    let mut any_address = false;
    for address in resolved {
        check_resolved_relay_address(address.ip(), policy).map_err(|error| error.to_string())?;
        any_address = true;
    }
    if any_address {
        Ok(())
    } else {
        Err("host resolved to no address".to_string())
    }
}

/// One adopted-relay health check: a TCP connect, within the probe budget, to an address that passed the public-address check.
async fn probe_adopted_relay(raw: &str) -> bool {
    let Ok(addresses) = resolve_public_relay(raw).await else {
        return false;
    };
    let connect = tokio::net::TcpStream::connect(&addresses[..]);
    matches!(
        tokio::time::timeout(ADOPTED_RELAY_PROBE_TIMEOUT, connect).await,
        Ok(Ok(_))
    )
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

/// What [`handle_inbound_row_change`] did with a row (QA-0133): only a row
/// handed to the DB thread for admission may drive follow-up work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InboundVerdict {
    /// Queued on the DB thread's command channel for admission.
    Forwarded,
    /// Kept for resend behind a full DB channel (QA-0138); judged at resend.
    Kept,
    /// Consumed on the sync plane, an own echo, not retained, or dropped.
    NotForwarded,
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
    evt_tx: &dyn EventSink,
    db_cmd_tx: &Option<crossbeam::channel::Sender<DbCommand>>,
) -> InboundVerdict {
    // Feed the fabric FIRST — before the own-author filter — because our own
    // snapshot rows (replayed by seed_reconciliation) are exactly how the
    // fabric re-learns settings, peer declarations, and the shard ledger
    // after a restart. Sync-plane rows (`__shards`/`__peers`) are consumed
    // here and never forwarded to the DB thread.
    // RP-5: the fabric also builds its OWN sync-plane role view from `role`
    // rows (§2.5) — independent of, and in addition to, the DB plane's own
    // admission of the SAME rows (RP-2/RP-3's `role_grant`/pending tables).
    // `role` and `petal` are ordinary DB-plane tables and must still be
    // forwarded below; only `__shards`/`__peers` are sync-plane-only.
    // R1: a `verse`/`petal` entry signed by OUR OWN key was written by this
    // node after its DB plane admitted it — locally authoritative but
    // possibly stale (a restart replays it through here): it corrects a
    // false owner pin / maps an unmapped petal, and otherwise passes the
    // ordinary gates (`VerseFabric::note_own_replayed_*`).
    if change.author_id == local_did && !change.data.is_empty() {
        let own_row = || serde_json::from_slice::<serde_json::Value>(&change.data).ok();
        match change.table.as_str() {
            "verse" => {
                if let Some(row) = own_row() {
                    fabric.note_own_replayed_verse_row(verse_id, &row, local_did);
                }
            }
            "petal" => {
                if let Some(row) = own_row() {
                    fabric.note_own_replayed_petal_row(verse_id, &row, local_did);
                }
            }
            _ => fabric.note_entry(verse_id, change),
        }
    } else {
        fabric.note_entry(verse_id, change);
    }
    if change.table == HOLDERS_TABLE {
        // RP-6: our OWN holder declaration replayed from the snapshot is how
        // the fabric re-learns, after a restart, that it was already
        // published — so `local_holding_row` never publishes it twice.
        if change.author_id == local_did && !change.data.is_empty() {
            if let Some((shard_key, did)) = crate::sharding::parse_holder_key(&change.record_id) {
                if did == local_did {
                    fabric.note_local_holding(shard_key);
                }
            }
        }
        return InboundVerdict::NotForwarded;
    }
    if change.table == SHARD_TABLE
        || change.table == PEER_DECL_TABLE
        || change.table == crate::relay_advertise::RELAY_ADVERT_TABLE
    {
        return InboundVerdict::NotForwarded;
    }

    if change.author_id == local_did {
        tracing::debug!(
            verse_id,
            record_id = %change.record_id,
            "Inbound row echoes our own write — skipped (loop prevention)"
        );
        return InboundVerdict::NotForwarded;
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
                    return InboundVerdict::NotForwarded;
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
        return InboundVerdict::NotForwarded;
    };
    // QA-0138: rows already waiting for the DB thread go first — a newer row
    // never overtakes them, and a newer version of a kept key supersedes it.
    if change.table != fe_database::role_grants::ROLE_DOC_TABLE
        && !fabric.unsent_rows_mut().is_empty()
    {
        fabric.unsent_rows_mut().keep(change.clone());
        return InboundVerdict::Kept;
    }
    match tx.try_send(DbCommand::ApplyReplicatedRow {
        verse_id: verse_id.to_string(),
        table: change.table.clone(),
        record_id: change.record_id.clone(),
        row_bytes: change.data.clone(),
        author_did: change.author_id.clone(),
    }) {
        Ok(()) => InboundVerdict::Forwarded,
        Err(crossbeam::channel::TrySendError::Full(_)) => {
            if change.table == fe_database::role_grants::ROLE_DOC_TABLE {
                // QA-0132: a dropped revoke would leave the DB plane judging
                // with a stale role view; kept and resent ahead of re-judges.
                fabric.keep_unsent_role_row(change.clone());
                return InboundVerdict::NotForwarded;
            }
            let refused =
                INBOUND_APPLY_DROPS.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
            tracing::warn!(
                verse_id,
                table = %change.table,
                record_id = %change.record_id,
                refused_total = refused,
                "DB command channel full — inbound replicated row kept for resend"
            );
            fabric.unsent_rows_mut().keep(change.clone());
            InboundVerdict::Kept
        }
        Err(crossbeam::channel::TrySendError::Disconnected(_)) => InboundVerdict::NotForwarded,
    }
}

/// Hand the DB thread every scene key `dids` wrote, each with EVERY author's
/// replica entry for it, as ONE command (QA-0121/QA-0131). `Err` means nothing
/// was queued, so the caller keeps the DIDs (QA-0132).
async fn send_rejudge_batch(
    replicator: &dyn VerseReplicator,
    tx: &crossbeam::channel::Sender<DbCommand>,
    verse_id: &str,
    dids: &[String],
    restore_only: bool,
) -> Result<usize, String> {
    let mut nodes: std::collections::BTreeMap<String, fe_runtime::messages::RejudgeCandidates> =
        std::collections::BTreeMap::new();
    for did in dids {
        let entries = replicator
            .entries_by_author(did, &["node/"])
            .await
            .map_err(|error| format!("reading {did}'s scene rows: {error}"))?;
        nodes.extend(entries.into_iter().map(|change| (change.record_id, Vec::new())));
    }
    if nodes.is_empty() {
        return Ok(0);
    }
    let snapshot = replicator
        .snapshot()
        .await
        .map_err(|error| format!("replica snapshot: {error}"))?;
    for change in snapshot {
        if change.table == "node" && !change.data.is_empty() {
            if let Some(candidates) = nodes.get_mut(&change.record_id) {
                candidates.push((change.author_id, change.data));
            }
        }
    }
    let rows = nodes.len();
    tx.try_send(DbCommand::RejudgeReplicatedNodes {
        verse_id: verse_id.to_string(),
        nodes: nodes.into_iter().collect(),
        restore_only,
    })
    .map_err(|error| format!("DB command channel: {error}"))?;
    Ok(rows)
}

/// Resend content rows a full DB channel refused, oldest first (QA-0138),
/// prefetching the blob of each resent scene-asset row. Returns true when
/// every verse's kept rows are on the channel.
fn resend_unsent_rows(
    fabrics: &mut HashMap<String, VerseFabric>,
    db_cmd_tx: &Option<crossbeam::channel::Sender<DbCommand>>,
    scene_assets: &mut crate::scene_assets::SceneAssetJobs,
) -> bool {
    let Some(tx) = db_cmd_tx.as_ref() else {
        return true;
    };
    for (verse_id, fabric) in fabrics.iter_mut() {
        while let Some((seq, change)) = fabric.unsent_rows_mut().pop() {
            let command = DbCommand::ApplyReplicatedRow {
                verse_id: verse_id.clone(),
                table: change.table.clone(),
                record_id: change.record_id.clone(),
                row_bytes: change.data.clone(),
                author_did: change.author_id.clone(),
            };
            match tx.try_send(command) {
                Ok(()) => {
                    if let Some(hash) = scene_asset_prefetch_hash(fabric, verse_id, &change) {
                        scene_assets.prefetch(verse_id, hash);
                    }
                }
                Err(crossbeam::channel::TrySendError::Full(_)) => {
                    fabric.unsent_rows_mut().restore(seq, change);
                    return false;
                }
                Err(crossbeam::channel::TrySendError::Disconnected(_)) => {
                    fabric.unsent_rows_mut().clear();
                    return true;
                }
            }
        }
    }
    true
}

/// Resend `role` rows a full DB channel refused, oldest first (QA-0132).
/// Returns true when every verse's kept rows are on the channel.
fn resend_unsent_role_rows(
    fabrics: &mut HashMap<String, VerseFabric>,
    db_cmd_tx: &Option<crossbeam::channel::Sender<DbCommand>>,
) -> bool {
    let Some(tx) = db_cmd_tx.as_ref() else {
        return true;
    };
    for (verse_id, fabric) in fabrics.iter_mut() {
        let kept = fabric.unsent_role_rows_mut();
        while let Some(change) = kept.pop_front() {
            let command = DbCommand::ApplyReplicatedRow {
                verse_id: verse_id.clone(),
                table: change.table.clone(),
                record_id: change.record_id.clone(),
                row_bytes: change.data.clone(),
                author_did: change.author_id.clone(),
            };
            match tx.try_send(command) {
                Ok(()) => {}
                Err(crossbeam::channel::TrySendError::Full(_)) => {
                    kept.push_front(change);
                    return false;
                }
                Err(crossbeam::channel::TrySendError::Disconnected(_)) => {
                    kept.clear();
                    return true;
                }
            }
        }
    }
    true
}

/// QA-0121 re-judge on demotion: for every DID whose role fell, the DB thread
/// re-decides each scene key it wrote from EVERY author's replica entry, so a
/// row admitted before the revoke arrived converges with peers that refused
/// it. QA-0132: a DID leaves the queue only once its batch was accepted — a
/// closed replica keeps it until reopen, a failed read or full channel until
/// the retry. Returns true when kept work needs a timed retry.
async fn drain_rejudge_queues(
    fabrics: &mut HashMap<String, VerseFabric>,
    replicas: &HashMap<String, Box<dyn VerseReplicator>>,
    db_cmd_tx: &Option<crossbeam::channel::Sender<DbCommand>>,
) -> bool {
    // The DB plane must hold every kept revoke before it judges against it.
    if !resend_unsent_role_rows(fabrics, db_cmd_tx) {
        return true;
    }
    let mut retry = false;
    for (verse_id, fabric) in fabrics.iter_mut() {
        let dids = fabric.pending_rejudges();
        if dids.is_empty() {
            continue;
        }
        let Some(tx) = db_cmd_tx.as_ref() else {
            // Offline/test mode: no DB plane holds rows to re-judge.
            fabric.ack_rejudges(&dids);
            continue;
        };
        let Some(replicator) = replicas.get(verse_id) else {
            continue;
        };
        match send_rejudge_batch(replicator.as_ref(), tx, verse_id, &dids, false).await {
            Ok(rows) => {
                tracing::info!(verse_id, demoted = dids.len(), rows, "Re-judging scene rows of demoted authors");
                fabric.ack_rejudges(&dids);
            }
            Err(error) => {
                tracing::warn!(verse_id, "re-judge of demoted authors kept queued for retry: {error}");
                retry = true;
            }
        }
    }
    retry
}

/// RP-5 §2.5 "re-read on empowerment": for every open verse's queued DIDs,
/// replay their `role`/`__shards`/`__holders`/`verse`/`petal` entries straight
/// from the local replica through the SAME inbound apply path a live row takes.
/// This is how a statement the fabric held back (its grantor lacked authority)
/// — and which the fabric itself never stores — is recovered once that grantor
/// is empowered. The read is awaited directly here, never through a channel
/// only this loop drains (the §inbound-apply self-drain rule).
///
/// QA-0131: scene (`node/`) rows are NEVER replayed through the current role —
/// a row written while its author was revoked would be admitted by the
/// re-grant. They go to the DB thread as a restore-only re-judge, which admits
/// a version only if its author held Editor when it was written. QA-0132: a
/// DID is acknowledged only after both steps were accepted. Returns true when
/// kept work needs a timed retry.
async fn drain_reread_queues(
    fabrics: &mut HashMap<String, VerseFabric>,
    replicas: &HashMap<String, Box<dyn VerseReplicator>>,
    local_did: &str,
    evt_tx: &dyn EventSink,
    db_cmd_tx: &Option<crossbeam::channel::Sender<DbCommand>>,
    docs_stack: &Option<Arc<DocsStack>>,
    mut scene_assets: Option<&mut crate::scene_assets::SceneAssetJobs>,
) -> bool {
    // A restore is judged against the grants the DB plane holds: send kept ones first.
    if !resend_unsent_role_rows(fabrics, db_cmd_tx) {
        return true;
    }
    let mut retry = false;
    for (verse_id, fabric) in fabrics.iter_mut() {
        let dids = fabric.pending_rereads();
        if dids.is_empty() {
            continue;
        }
        let Some(replicator) = replicas.get(verse_id) else {
            continue;
        };
        let mut done = Vec::with_capacity(dids.len());
        for did in dids {
            match replicator
                .entries_by_author(&did, &["role/", "__shards/", "__holders/", "__relays/", "verse/", "petal/"])
                .await
            {
                Ok(entries) => {
                    for change in entries {
                        handle_inbound_row_change(
                            fabric, verse_id, &change, local_did, evt_tx, db_cmd_tx,
                        );
                    }
                    done.push(did);
                }
                Err(e) => {
                    tracing::warn!(verse_id, did, "role re-read on empowerment failed; kept for retry: {e}");
                    retry = true;
                }
            }
        }
        if !fabric.unsent_role_rows_mut().is_empty() {
            // A replayed grant the full channel refused: restore only once it is sent.
            retry = true;
            done.clear();
        }
        if let (Some(tx), false) = (db_cmd_tx.as_ref(), done.is_empty()) {
            if let Err(error) = send_rejudge_batch(replicator.as_ref(), tx, verse_id, &done, true).await {
                tracing::warn!(verse_id, "scene restore after empowerment kept queued for retry: {error}");
                retry = true;
                done.clear();
            }
        }
        // QA-0118: asset rows that arrived before their author's grant skipped
        // the row-arrival prefetch; fetch them now, and retry unavailable blobs
        // a provider may now serve under the new role view.
        if let (Some(jobs), false) = (scene_assets.as_deref_mut(), done.is_empty()) {
            for did in &done {
                match replicator.entries_by_author(did, &["asset/"]).await {
                    Ok(entries) => {
                        for change in entries {
                            if let Some(hash) = scene_asset_prefetch_hash(fabric, verse_id, &change) {
                                jobs.prefetch(verse_id, hash);
                            }
                        }
                    }
                    Err(e) => tracing::debug!(verse_id, did, "empowered author's asset rows unreadable: {e}"),
                }
            }
            jobs.rearm(verse_id);
        }
        fabric.ack_rereads(&done);
        sync_blob_access_gate(docs_stack, verse_id, fabric);
    }
    retry
}

/// Mirror this verse's current role view into the blob-ALPN request
/// filter's shared state (RP-5 step 6 / AC-21(e)). A no-op when no P2P
/// stack is online — an offline or virtual transport has no blobs ALPN to
/// gate.
fn sync_blob_access_gate(
    docs_stack: &Option<Arc<DocsStack>>,
    verse_id: &str,
    fabric: &VerseFabric,
) {
    if let Some(stack) = docs_stack {
        stack
            .blob_access_gate()
            .set_verse_access(verse_id, fabric.access_snapshot(verse_id));
    }
}

/// QA-0118: the blob a peer's replicated scene-asset metadata row references,
/// so its bytes are fetched when the row arrives rather than at petal entry.
/// QA-0133: only for an author this node's role view holds at Editor+ at the
/// row's petal (the verse when the petal is not mapped yet) — a row the DB
/// plane would refuse never drives a fetch.
fn scene_asset_prefetch_hash(
    fabric: &VerseFabric,
    verse_id: &str,
    change: &RowChange,
) -> Option<fe_runtime::blob_store::BlobHash> {
    if change.table != "asset" || change.is_tombstone || change.data.is_empty() {
        return None;
    }
    let row: serde_json::Value = serde_json::from_slice(&change.data).ok()?;
    row.get(fe_runtime::scene_asset::SCENE_ASSET_ROW_MARKER)?;
    let scope = row
        .get("petal_id")
        .and_then(serde_json::Value::as_str)
        .and_then(|petal| fabric.petal_scope(verse_id, petal))
        .unwrap_or_else(|| fe_policy::build_scope(verse_id, None, None));
    if !fabric
        .role_at(&change.author_id, &scope)
        .is_at_least(fe_policy::RoleLevel::Editor)
    {
        return None;
    }
    fe_runtime::blob_store::hash_from_hex(row.get("content_hash")?.as_str()?).ok()
}

/// Compatibility requests share the admitted, verified asynchronous scene job boundary.
pub(crate) fn handle_fetch_blob(
    scene_assets: &mut crate::scene_assets::SceneAssetJobs,
    hash: &fe_runtime::blob_store::BlobHash,
    verse_id: &str,
) {
    scene_assets.fetch(verse_id, *hash);
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
    evt_tx: &dyn EventSink,
    db_cmd_tx: &Option<crossbeam::channel::Sender<DbCommand>>,
    fabric: &mut VerseFabric,
) -> Vec<fe_runtime::blob_store::BlobHash> {
    let entries = match replicator.snapshot().await {
        Ok(entries) => entries,
        Err(e) => {
            tracing::warn!(verse_id, "Reconciliation snapshot failed: {e}");
            return Vec::new();
        }
    };
    if entries.is_empty() {
        return Vec::new();
    }
    let total = entries.len();
    let mut forwarded = 0usize;
    let mut forwarded_assets = Vec::new();
    for change in entries {
        // The literal same inbound apply path the live pump's rows take:
        // own-author rows filtered here (loop prevention — the docs author
        // is the endpoint identity, so our own snapshot entries carry our
        // DID), survivors emitted + `try_send`'d to the DB thread. The
        // fabric is fed for every row (settings/peers/ledger re-learn) and
        // the retention filter drops unhosted timeseries rows.
        let verdict =
            handle_inbound_row_change(fabric, verse_id, &change, local_did, evt_tx, db_cmd_tx);
        if verdict == InboundVerdict::Forwarded && change.table == "asset" {
            forwarded_assets.push(change);
        }
        forwarded += 1;
    }
    tracing::info!(
        verse_id,
        entries = total,
        forwarded,
        "Reconciliation: replayed replica entries through the inbound apply path"
    );
    // QA-0133: judged against the role view the WHOLE snapshot converged to,
    // so an asset row replayed before its author's grant is not skipped.
    forwarded_assets
        .iter()
        .filter_map(|change| scene_asset_prefetch_hash(fabric, verse_id, change))
        .collect()
}

/// R1: replay THIS node's own `verse/` and `petal/` entries by author after
/// the snapshot. The snapshot is latest-per-key, so a later foreign entry on
/// the same key (say, a forged self-signed manifest naming its author the
/// creator) hides ours from it; replayed by author, our own DB-admitted
/// manifest still corrects the sync-plane owner pin and our own petals still
/// map (`handle_inbound_row_change` notes own-signed rows as locally
/// admitted). Best-effort, like the snapshot.
async fn reassert_own_rows(
    replicator: &dyn VerseReplicator,
    verse_id: &str,
    local_did: &str,
    evt_tx: &dyn EventSink,
    db_cmd_tx: &Option<crossbeam::channel::Sender<DbCommand>>,
    fabric: &mut VerseFabric,
) {
    match replicator
        .entries_by_author(local_did, &["verse/", "petal/"])
        .await
    {
        Ok(entries) => {
            for change in entries {
                handle_inbound_row_change(fabric, verse_id, &change, local_did, evt_tx, db_cmd_tx);
            }
        }
        Err(e) => tracing::warn!(
            verse_id,
            "re-asserting our own manifest/petal rows failed: {e}"
        ),
    }
}

#[derive(Debug)]
struct ReplicaOpenReport {
    native_ready: bool,
    /// Scene-asset blobs the replica's metadata references (QA-0118 prefetch).
    scene_asset_hashes: Vec<fe_runtime::blob_store::BlobHash>,
}

async fn prepare_join_replica(
    replicas: &HashMap<String, Box<dyn VerseReplicator>>,
    transport: &ReplicaTransport,
    request: &crate::join_ticket::JoinReplicaRequest,
    local_did: &str,
) -> Result<Vec<String>, &'static str> {
    if replicas.contains_key(&request.invitation.manifest().verse_id) {
        return Err("This Verse replica is already open; leave it before accepting another invitation");
    }
    if request.deadline <= std::time::Instant::now() { return Err("Invitation connection request expired"); }
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| "Invitation clock is unavailable")?.as_secs();
    let ticket = crate::join_ticket::NativeJoinTicket::for_invitation(&request.invitation, local_did, now)
        .map_err(|_| "Invitation transport identity or capability is invalid")?;
    let ReplicaTransport::Iroh(holder) = transport else { return Err("A native online connection is required"); };
    let client = holder.docs_client().ok_or("P2P is offline; invitation connection did not start")?;
    let peers = ticket.peers().iter().map(serde_json::to_string).collect::<Result<Vec<_>,_>>()
        .map_err(|_| "Invitation bootstrap addresses are unavailable")?;
    tokio::time::timeout(std::time::Duration::from_secs(5), client.import_namespace(ticket.capability()))
        .await.map_err(|_| "Capability import outcome is unconfirmed")?
        .map_err(|_| "Native capability import failed")?;
    if request.deadline <= std::time::Instant::now() { return Err("Capability imported after the connection deadline; readiness is unconfirmed"); }
    let completed_at = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| "Capability import may have completed; clock is unavailable")?.as_secs();
    request.invitation.revalidate(local_did, completed_at)
        .map_err(|_| "Capability import may have completed; invitation expired before connection")?;
    Ok(peers)
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
    peers: &[iroh::EndpointAddr],
    local_did: &str,
    evt_tx: &dyn EventSink,
    db_cmd_tx: &Option<crossbeam::channel::Sender<DbCommand>>,
    blob_store: &BlobStoreHandle,
    pending: &mut PendingWrites,
    fabric: &mut VerseFabric,
    local_declaration: PeerDeclaration,
) -> ReplicaOpenReport {
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
    let mut sync_started = false;
    let mut subscribed = false;
    match replicator.open_document().await {
        Ok(()) => {}
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
                subscribed = true;
            }
            Err(e) => {
                tracing::warn!(verse_id, "Replica subscribe failed — no inbound pump: {e}");
            }
        }
    }

    if replicator.is_doc_backed() && subscribed && replicator.open_error().is_none() {
        match replicator.start_sync(peers.to_vec()).await {
            Ok(()) => sync_started = true,
            Err(_) => {
                replicator.mark_open_failed("Replica synchronization could not start".into());
                if let Some(pump) = inbound_pumps.remove(verse_id) { pump.abort(); }
                send_sync_event(evt_tx, SyncEvent::ReplicaOpenFailed {
                    verse_id: verse_id.to_owned(), reason: "Replica synchronization could not start".into(),
                });
            }
        }
    }
    let native_ready = replicator.is_doc_backed() && subscribed && sync_started
        && replicator.open_error().is_none();

    // F4 startup reconciliation (deadlock-free shape — see
    // seed_reconciliation's doc comment): replay the replica's current
    // entries through the SAME inbound apply path, applied directly. A row
    // the A3 role gate denied while a role or the verse manifest had not
    // yet converged never re-fires as an `InsertRemote`, so it would be
    // stranded in the doc store forever; this pass gives it a second
    // chance on every replica open. Own-author rows are filtered
    // downstream by the inbound handler like any other row. Skipped for a
    // failed online open (no document to snapshot).
    let mut scene_asset_hashes = Vec::new();
    if replicator.open_error().is_none() {
        scene_asset_hashes =
            seed_reconciliation(&replicator, verse_id, local_did, evt_tx, db_cmd_tx, fabric).await;
        reassert_own_rows(&replicator, verse_id, local_did, evt_tx, db_cmd_tx, fabric).await;
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
        request_local_catalog_and_roles(db_cmd_tx, verse_id);
    }
    ReplicaOpenReport {
        native_ready,
        scene_asset_hashes,
    }
}

/// Request stored catalog recovery and local role replay after replica open; see src/AGENTS.md.
fn request_local_catalog_and_roles(
    db_cmd_tx: &Option<crossbeam::channel::Sender<DbCommand>>,
    verse_id: &str,
) {
    let Some(tx) = db_cmd_tx else {
        return;
    };
    if let Err(error) = tx.try_send(DbCommand::ReemitLocalVerseCatalog {
        verse_id: verse_id.to_owned(),
    }) {
        tracing::warn!(
            verse_id,
            "local catalog recovery request not delivered: {error}"
        );
    }
    if let Err(e) = tx.try_send(DbCommand::ReemitLocalRoleStatements {
        verse_id: verse_id.to_string(),
    }) {
        tracing::warn!(
            verse_id,
            "role statement re-emit request not delivered to the DB thread: {e}"
        );
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
    let mut holder_publish: Option<(String, Vec<u8>)> = None;
    if table == "role" {
        // RP-5: feed our OWN outbound role statements (local grant/revoke)
        // into the fabric too, so a local gate (e.g. the compute requester
        // check) reflects a role we just assigned without waiting for a
        // round trip through the replica.
        fabric.note_role_row(verse_id, record_id, author_did, &data);
    }
    if let Ok(row) = serde_json::from_slice::<serde_json::Value>(&data) {
        match table {
            "verse" => {
                // OF-6d: our own outbound write — locally admitted.
                fabric.note_local_verse_row(verse_id, &row, author_did);
            }
            "petal" => {
                // OF-9: our own outbound write — locally admitted.
                fabric.note_local_petal_row(verse_id, &row);
            }
            "iot_reading" => {
                if let Some(shard) = ShardId::of_reading_row(&row, fabric.settings.bucket_width_ms)
                {
                    if let Some(ledger_bytes) = fabric.ensure_shard(&shard, data.len(), author_did)
                    {
                        ledger_publish = Some((shard.key(), ledger_bytes));
                    }
                    // RP-6 §2.6: a local reading in a shard we do not host
                    // makes us a holder of its creator copy — declared once
                    // per shard, so requesters expect (and wait for) us.
                    holder_publish = fabric.local_holding_row(&shard, author_did);
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

    // RP-6 §2.6: the holder declaration lands after the ledger row (it
    // names a shard the ledger defines) and before the reading (a requester
    // that sees the reading already expects our answer).
    if let Some((holder_key, holder_bytes)) = holder_publish {
        if let Err(e) = repl
            .write_row(HOLDERS_TABLE, &holder_key, &holder_bytes)
            .await
        {
            tracing::warn!(
                verse_id,
                holder = %holder_key,
                "Holder declaration publish failed — will re-declare on the next reading: {e}"
            );
            if let Some((shard_key, _)) = crate::sharding::parse_holder_key(&holder_key) {
                fabric.forget_local_holding(shard_key);
            }
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

/// Inverse of [`derive_gossip_topic`].
fn verse_of_gossip_topic(topic_key: &str) -> Option<&str> {
    topic_key.strip_prefix("verse:")
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
async fn subscribe_to_verse_gossip_topic(
    endpoint: Option<&SyncEndpoint>,
    gossip_host: &Option<Gossip>,
    virtual_transport: &Option<Arc<dyn VirtualTransportFactory>>,
    local_did: &str,
    local_node: iroh::EndpointId,
    gossip_senders: &mut HashMap<String, TopicSender>,
    gossip_pumps: &mut HashMap<String, tokio::task::AbortHandle>,
    gossip_inbound_tx: tokio::sync::mpsc::Sender<GossipIncoming>,
    verse_id: &str,
    peers: &[iroh::EndpointAddr],
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

    // The gossip join below dials by bare EndpointId and is ONE-SHOT: it can
    // only resolve an address the endpoint's address book already knows.
    // The docs sync (`start_sync`) dials these same peers only in the
    // background, so without this the join races the docs dial that would
    // first teach the endpoint the peer's address — a lost race leaves the
    // topic with no neighbor for the whole session (every compute broadcast
    // reaches nobody). Register every bootstrap peer's address up front so
    // the join always resolves.
    // (iroh 1.x: the endpoint's in-memory address book — `MemoryLookup` —
    // replaces 0.35's `add_node_addr`; registration is infallible.)
    if let Some(ep) = endpoint {
        for peer in peers {
            ep.add_peer_addr(peer.clone());
        }
    }

    // Bootstrap the topic with the verse's known peers (M2/F7). Without this
    // the topic has zero neighbors and a compute request broadcast can only
    // ever reach the local host — the fan-out transport needs real
    // neighbors, and the same peer set the replica dials for doc sync is the
    // one to gossip with. Empty on a fresh host (the dialed side joins via
    // its own bootstrap set).
    let bootstrap: Vec<iroh::EndpointId> = peers.iter().map(|p| p.id).collect();

    // iroh-gossip 0.101's subscribe is async: awaiting it hands the Join to
    // the gossip actor before the open sequence starts, which keeps the
    // subscribe-before-open ordering (§gossip-bootstrap) intact.
    match gossip
        .subscribe(gossip_topic_id(&topic_key), bootstrap)
        .await
    {
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
/// loop's aggregated inbound channel. NeighborUp is forwarded as an empty
/// frame (tileset re-advertisement, QA-0101); NeighborDown is not consumed. A pump is a separate task, so the awaiting send below is
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
            Some(Ok(GossipTopicEvent::Received(message))) => {
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
            Some(Ok(GossipTopicEvent::NeighborUp(peer))) => {
                // Empty content = membership signal; see the select loop (QA-0101).
                let signal = GossipIncoming {
                    verse_id: verse_id.clone(),
                    from: peer,
                    direct: true,
                    content: bytes::Bytes::new(),
                };
                if inbound_tx.send(signal).await.is_err() {
                    break;
                }
            }
            Some(Ok(GossipTopicEvent::NeighborDown(_))) => {} // not consumed
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
/// iroh-gossip.
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

/// Outbound/inbound tileset-advertisement gossip frame (S3). Tagged `kind`
/// (never `type`) so it shares a verse's compute topic with
/// [`crate::distributed_query::ComputeEnvelope`] (tagged `type`) without the
/// two decoders ever mistaking one for the other.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
struct TilesetAd {
    kind: String,
    tileset_id: String,
    /// HashSeq root (hex).
    root_hash: String,
    total_chunks: u32,
    approx_bytes: u64,
    /// `EndpointAddr` JSON the provider can be dialed at.
    provider_addr: String,
    /// Best-effort publisher DID (PAS-P5 S7a) — a TOFU pin *candidate* only,
    /// never trusted on its own; absent from an older peer's wire frame
    /// decodes as empty (`#[serde(default)]`), the same "no hint" case an
    /// empty value from this node's own publisher means.
    #[serde(default)]
    publisher_did: String,
}

/// The only value [`TilesetAd::kind`] may hold.
const TILESET_AD_KIND: &str = "tileset_ad";

/// Decode a tileset ad from gossip bytes; `None` for anything else (a
/// compute envelope, garbage, or a frame from before this campaign step).
fn decode_tileset_ad(bytes: &[u8]) -> Option<TilesetAd> {
    let ad: TilesetAd = serde_json::from_slice(bytes).ok()?;
    (ad.kind == TILESET_AD_KIND).then_some(ad)
}

/// The `provider_addr` an unsigned tileset ad claims, read with the handler's own decoder (the legacy latch check).
pub(crate) fn legacy_tileset_ad_provider(bytes: &[u8]) -> Option<String> {
    decode_tileset_ad(bytes).map(|ad| ad.provider_addr)
}

/// Most tileset ids the provider map tracks at once (ad-storm bound).
const MAX_TRACKED_TILESETS: usize = 1024;
/// Most provider entries kept per tileset id (ad-storm bound).
const MAX_PROVIDERS_PER_TILESET: usize = 16;

/// One peer's advertised source for a tileset, learned on one verse topic.
#[derive(Debug, Clone)]
struct TilesetProvider {
    /// The verse topic the ad arrived on; forgotten when that verse closes.
    verse_id: String,
    provider: ProviderRef,
    root: Hash32,
    /// F23-style admission: whether the delivery was direct AND the ad's own
    /// `provider_addr` named the authenticated sender (§transport-admission-
    /// control item 2 pattern, applied to tileset ads).
    authenticated: bool,
    /// The ad's claimed size (best-effort; not re-verified until a download
    /// actually fetches bundles).
    advertised_bytes: u64,
    /// This entry's own advertised publisher DID (empty if none) — a TOFU
    /// pin candidate (PAS-P5 S7a), carried alongside its own root so
    /// [`TilesetProviderMap::root_for`] never pairs one entry's root with a
    /// *different* entry's claimed publisher.
    publisher_did: String,
    /// Recency stamp from [`TilesetProviderMap::clock`].
    seen: u64,
}

/// The root a download should fetch, whether an authenticated provider
/// vouched for it, and that SAME entry's advertised publisher DID (PAS-P5
/// S7a critic amendment: never mixed with a different entry's claim).
#[derive(Debug, Clone, PartialEq, Eq)]
struct RootChoice {
    root: Hash32,
    authenticated: bool,
    publisher_did: String,
}

/// `tileset_id -> known (provider, root)` entries learned from inbound ads,
/// authenticated first then newest first. Never persisted — rebuilt from
/// gossip each session (the durable state is `TilesetTransfer`'s ledger).
///
/// Entries are keyed by `(verse, provider, root)`, so an unauthenticated
/// (relayed) ad can never rewrite what an authenticated ad recorded — see
/// `src/AGENTS.md` §tileset-security.
#[derive(Default)]
struct TilesetProviderMap {
    by_id: HashMap<String, Vec<TilesetProvider>>,
    clock: u64,
}

impl TilesetProviderMap {
    /// Record one inbound ad; `false` when it was refused (a relayed claim
    /// about a provider that vouched for itself directly, or a full map).
    ///
    /// An authenticated ad replaces that provider's earlier entries on the
    /// same verse (the provider republished) and every relayed claim made
    /// about it; a relayed ad only ever adds a separate hint entry.
    #[allow(clippy::too_many_arguments)] // PAS-P5 S7a added publisher_did; params mirror the ad fields
    fn record(
        &mut self,
        verse_id: &str,
        tileset_id: String,
        provider: ProviderRef,
        root: Hash32,
        authenticated: bool,
        advertised_bytes: u64,
        publisher_did: String,
    ) -> bool {
        self.clock += 1;
        let seen = self.clock;
        let map_full =
            !self.by_id.contains_key(&tileset_id) && self.by_id.len() >= MAX_TRACKED_TILESETS;
        if map_full && (!authenticated || !self.evict_one_unauthenticated_tileset()) {
            return false;
        }
        let entries = self.by_id.entry(tileset_id.clone()).or_default();
        if authenticated {
            entries
                .retain(|e| e.provider != provider || (e.authenticated && e.verse_id != verse_id));
        } else {
            if entries
                .iter()
                .any(|e| e.authenticated && e.provider == provider)
            {
                return false;
            }
            if let Some(existing) = entries
                .iter_mut()
                .find(|e| e.verse_id == verse_id && e.provider == provider && e.root == root)
            {
                existing.seen = seen;
                existing.advertised_bytes = advertised_bytes;
                existing.publisher_did = publisher_did;
                Self::order(entries);
                return true;
            }
        }
        entries.push(TilesetProvider {
            verse_id: verse_id.to_string(),
            provider,
            root,
            authenticated,
            advertised_bytes,
            publisher_did,
            seen,
        });
        while entries.len() > MAX_PROVIDERS_PER_TILESET {
            // R1: hint-only first, then an entry whose verse another entry
            // still represents, then the oldest — so a flood of hints on
            // one verse can never evict the last entry of ANOTHER verse
            // and shrink the fetch context `verses_for` binds a root to
            // (fewer verses = a wider learned binding).
            let victim = (0..entries.len())
                .min_by_key(|&i| {
                    let e = &entries[i];
                    let verse_shared = entries
                        .iter()
                        .enumerate()
                        .any(|(j, other)| j != i && other.verse_id == e.verse_id);
                    (e.authenticated, !verse_shared, e.seen)
                })
                .unwrap_or(0);
            entries.remove(victim);
        }
        let kept = entries.iter().any(|e| e.seen == seen);
        Self::order(entries);
        kept
    }

    /// Authenticated first, then newest first.
    fn order(entries: &mut [TilesetProvider]) {
        entries.sort_by_key(|e| (!e.authenticated, std::cmp::Reverse(e.seen)));
    }

    /// Drop the stalest tileset id no authenticated provider backs; `false` if none.
    fn evict_one_unauthenticated_tileset(&mut self) -> bool {
        let victim = self
            .by_id
            .iter()
            .filter(|(_, entries)| entries.iter().all(|e| !e.authenticated))
            .min_by_key(|(_, entries)| entries.iter().map(|e| e.seen).max().unwrap_or(0))
            .map(|(id, _)| id.clone());
        match victim {
            Some(id) => self.by_id.remove(&id).is_some(),
            None => false,
        }
    }

    /// Forget every ad learned on `verse_id` (its replica closed).
    fn forget_verse(&mut self, verse_id: &str) {
        for entries in self.by_id.values_mut() {
            entries.retain(|e| e.verse_id != verse_id);
        }
        self.by_id.retain(|_, entries| !entries.is_empty());
    }

    /// The root to fetch: an authenticated provider's newest root, else the newest hint.
    fn root_for(&self, tileset_id: &str) -> Option<RootChoice> {
        self.by_id
            .get(tileset_id)
            .and_then(|entries| entries.first())
            .map(|e| RootChoice {
                root: e.root,
                authenticated: e.authenticated,
                publisher_did: e.publisher_did.clone(),
            })
    }

    /// The fetch context a root is bound to (OF-6a "all mapped verses",
    /// widened by R1): every verse an entry advertised `tileset_id` on
    /// (under any root) or advertised `root` on (under any tileset id).
    /// The learned binding admits a Viewer+ of EVERY one of them, so a
    /// larger context is only ever stricter — an ad re-using a victim's
    /// root under a fresh id, or a victim's id with a fresh root, cannot
    /// make this node bind protected bytes to the attacker's verse alone.
    fn verses_for(&self, tileset_id: &str, root: Hash32) -> std::collections::BTreeSet<String> {
        self.by_id
            .iter()
            .flat_map(|(id, entries)| {
                let same_id = id == tileset_id;
                entries.iter().filter(move |e| same_id || e.root == root)
            })
            .map(|e| e.verse_id.clone())
            .collect()
    }

    /// Endpoint ids that VOUCHED for themselves — an authenticated (direct,
    /// provider == sender) entry — for `tileset_id` or for `root`.
    fn vouched_providers(&self, tileset_id: &str, root: Hash32) -> HashSet<iroh::EndpointId> {
        self.by_id
            .iter()
            .flat_map(|(id, entries)| {
                let same_id = id == tileset_id;
                entries
                    .iter()
                    .filter(move |e| e.authenticated && (same_id || e.root == root))
            })
            .filter_map(|e| {
                IrohBlobTransport::parse_provider(&e.provider)
                    .ok()
                    .map(|a| a.id)
            })
            .collect()
    }

    /// Providers that advertised `root` for `tileset_id`, authenticated first, deduplicated.
    fn candidates(&self, tileset_id: &str, root: Hash32) -> Vec<ProviderRef> {
        let mut out: Vec<ProviderRef> = Vec::new();
        for entry in self.by_id.get(tileset_id).into_iter().flatten() {
            if entry.root == root && !out.contains(&entry.provider) {
                out.push(entry.provider.clone());
            }
        }
        out
    }

    /// The best-effort advertised size of the chosen root (0 if unknown).
    fn bytes_for(&self, tileset_id: &str) -> u64 {
        self.by_id
            .get(tileset_id)
            .and_then(|entries| entries.first())
            .map(|e| e.advertised_bytes)
            .unwrap_or(0)
    }
}

/// Candidate providers for one request: an explicit override first (when
/// non-empty), then every advertised provider, de-duplicated.
fn resolve_providers(
    map: &TilesetProviderMap,
    tileset_id: &str,
    root: Hash32,
    explicit: Option<&str>,
) -> Vec<ProviderRef> {
    let mut out = Vec::new();
    if let Some(addr) = explicit.filter(|a| !a.is_empty()) {
        out.push(ProviderRef::new(addr));
    }
    for candidate in map.candidates(tileset_id, root) {
        if !out.contains(&candidate) {
            out.push(candidate);
        }
    }
    out
}

/// [`resolve_providers`] plus the tileset's advertised root and that SAME
/// entry's claimed publisher DID (PAS-P5 S7a critic amendment: never a
/// *different* entry's claim — see [`RootChoice`]) — the triple every
/// meta/download request needs. An honest named error when the root is
/// unknown (never a silent no-op); a root no authenticated provider vouched
/// for is used only when none did, and is logged as such.
fn resolve_providers_and_root(
    map: &TilesetProviderMap,
    tileset_id: &str,
    explicit: Option<&str>,
) -> Result<(Vec<ProviderRef>, Hash32, String), &'static str> {
    let choice = map
        .root_for(tileset_id)
        .ok_or("no advertised root for this tileset yet")?;
    if !choice.authenticated {
        tracing::warn!(
            tileset_id,
            "No directly-authenticated provider for this tileset — using a relayed hint's root"
        );
    }
    let root = choice.root;
    let providers = resolve_providers(map, tileset_id, root, explicit);
    if providers.is_empty() {
        return Err("no known provider for this tileset");
    }
    Ok((providers, root, choice.publisher_did))
}

/// Pure core of the inbound tileset-ad authentication check
/// (§transport-admission-control item 2's pattern, applied here): a direct
/// delivery whose claimed `provider_addr` resolves to the authenticated
/// sender's `EndpointId` is authenticated; anything relayed, malformed, or
/// mismatched is an unauthenticated hint, never the download's only source
/// of truth (the per-bundle BLAKE3 check in `TilesetTransfer` still runs
/// regardless).
fn tileset_ad_is_authenticated(
    direct: bool,
    claimed_provider_addr: &str,
    from: iroh::EndpointId,
) -> bool {
    direct
        && IrohBlobTransport::parse_provider(&ProviderRef::new(claimed_provider_addr.to_string()))
            .map(|addr| addr.id == from)
            .unwrap_or(false)
}

/// Handle one inbound tileset ad: record its provider (admission above) and
/// echo it to the UI/terrain layer as [`SyncEvent::PeerTilesetAdvertisement`].
/// Returns the ad's root when the ad was ACCEPTED (`None` when refused).
fn handle_inbound_tileset_ad(
    providers: &mut TilesetProviderMap,
    evt_tx: &dyn EventSink,
    incoming: &GossipIncoming,
    ad: TilesetAd,
) -> Option<Hash32> {
    record_tileset_ad(providers, evt_tx, incoming, ad, None)
}

/// NET-29: answer a verified `reach_probe` frame when this node acts as a prober and the author is a declared Viewer+.
fn handle_reach_probe(
    responder: Option<&Arc<DialbackResponder>>,
    fabrics: &HashMap<String, VerseFabric>,
    gossip_senders: &HashMap<String, TopicSender>,
    incoming: &GossipIncoming,
    author_did: &str,
) {
    let (Some(responder), Some(fabric)) = (responder, fabrics.get(&incoming.verse_id)) else {
        return;
    };
    let Some(sender) = gossip_senders.get(&derive_gossip_topic(&incoming.verse_id)) else {
        return;
    };
    let declared = fabric.peers.contains_key(author_did);
    let role = fabric.role_at(author_did, &fe_policy::build_scope(&incoming.verse_id, None, None));
    let sink: Arc<dyn ReplySink> = Arc::new(sender.clone());
    if let Err(refusal) = responder.dispatch(&incoming.verse_id, author_did, &incoming.content, declared, role, sink) {
        tracing::debug!(verse_id = %incoming.verse_id, ?refusal, "Dial-back request refused");
    }
}

/// NET-29: deliver a verified `reach_result` only when its prober is a declared Viewer+ peer of the arrival verse.
fn handle_reach_result(
    requester: &mut DialbackRequester,
    fabrics: &HashMap<String, VerseFabric>,
    incoming: &GossipIncoming,
    local_did: &str,
    author_did: &str,
) {
    let Some(fabric) = fabrics.get(&incoming.verse_id) else {
        return;
    };
    let declared = fabric.peers.contains_key(author_did);
    let role = fabric.role_at(author_did, &fe_policy::build_scope(&incoming.verse_id, None, None));
    if let Err(refusal) = DialbackResponder::authorize(declared, role) {
        tracing::debug!(verse_id = %incoming.verse_id, ?refusal, "Dial-back result from an unauthorized prober ignored");
        return;
    }
    requester.accept_result_in_verse(local_did, author_did, &incoming.verse_id, &incoming.content, unix_now_ms());
}

/// NET-29: sign and broadcast a local dial-back request on up to a few joined verse topics.
async fn broadcast_dialback_request(
    requester: &mut DialbackRequester,
    gossip_senders: &HashMap<String, TopicSender>,
    signer: Option<&iroh::SecretKey>,
    request: DialbackRequest,
) {
    let DialbackRequest { url, reply } = request;
    let Some(secret) = signer else {
        let _ = reply.send(None);
        return;
    };
    // A full table answers at once instead of broadcasting a probe nobody will wait for.
    if requester.is_full() {
        let _ = reply.send(None);
        return;
    }
    let mut sent_verses: Vec<String> = Vec::new();
    for (topic_key, sender) in gossip_senders {
        if sent_verses.len() >= dialback_responder::MAX_REQUEST_VERSES {
            break;
        }
        let Some(verse_id) = verse_of_gossip_topic(topic_key) else {
            continue;
        };
        match dialback_responder::build_request_frame(secret, verse_id, &url) {
            Ok(frame) => match sender.broadcast(frame.into()).await {
                Ok(()) => sent_verses.push(verse_id.to_string()),
                Err(error) => tracing::warn!(verse_id, "Failed to broadcast dial-back request: {error}"),
            },
            Err(error) => tracing::warn!(verse_id, "Failed to seal dial-back request: {error}"),
        }
    }
    if sent_verses.is_empty() || !requester.register_in_verses(url, sent_verses, reply) {
        tracing::debug!("Dial-back request not sent or the pending table is full");
    }
}

/// Most tileset ads one signed frame carries (keeps a batch under the frame-size cap).
const TILESET_ADS_PER_FRAME: usize = 256;

/// Handle the verified payload of a signed `tileset_ad` frame: a JSON batch, each ad attributed to `author_did`.
fn handle_signed_tileset_ads(
    providers: &mut TilesetProviderMap,
    evt_tx: &dyn EventSink,
    incoming: &GossipIncoming,
    author_did: &str,
) {
    let Ok(ads) = serde_json::from_slice::<Vec<TilesetAd>>(&incoming.content) else {
        tracing::debug!(verse_id = %incoming.verse_id, "Signed tileset-ad frame with a malformed batch — ignored");
        return;
    };
    for ad in ads.into_iter().take(TILESET_ADS_PER_FRAME) {
        if ad.kind == TILESET_AD_KIND {
            record_tileset_ad(providers, evt_tx, incoming, ad, Some(author_did));
        }
    }
}

/// Shared ad admission; a signed ad is authenticated when its provider is the frame's signer.
fn record_tileset_ad(
    providers: &mut TilesetProviderMap,
    evt_tx: &dyn EventSink,
    incoming: &GossipIncoming,
    ad: TilesetAd,
    signed_author: Option<&str>,
) -> Option<Hash32> {
    let Ok(root) = hash_from_hex(&ad.root_hash) else {
        tracing::debug!(tileset_id = %ad.tileset_id, "Malformed tileset ad root hash — ignored");
        return None;
    };
    // S8b: an id that could never become a download dir, or a provider that
    // could never be dialed, is dropped before it costs map space or a UI event.
    if !is_safe_tileset_id(&ad.tileset_id) {
        tracing::debug!("Tileset ad with an unsafe tileset_id — ignored");
        return None;
    }
    if IrohBlobTransport::parse_provider(&ProviderRef::new(ad.provider_addr.clone())).is_err() {
        tracing::debug!(tileset_id = %ad.tileset_id, "Tileset ad with an undialable provider — ignored");
        return None;
    }
    let authenticated = match signed_author {
        Some(author_did) => IrohBlobTransport::parse_provider(&ProviderRef::new(ad.provider_addr.clone()))
            .map(|address| crate::replicator::peer_did_key(&address.id) == author_did)
            .unwrap_or(false),
        None => tileset_ad_is_authenticated(incoming.direct, &ad.provider_addr, incoming.from),
    };
    if !providers.record(
        &incoming.verse_id,
        ad.tileset_id.clone(),
        ProviderRef::new(ad.provider_addr.clone()),
        root,
        authenticated,
        ad.approx_bytes,
        ad.publisher_did.clone(),
    ) {
        tracing::debug!(
            tileset_id = %ad.tileset_id,
            authenticated,
            "Tileset advertisement refused (superseded or map full)"
        );
        return None;
    }
    tracing::debug!(
        tileset_id = %ad.tileset_id,
        authenticated,
        "Recorded a tileset advertisement"
    );
    // The DTO fe-ui parses today (`terrain_map/events.rs`) reads a richer
    // fe-terrain shape with defaults for the fields this thin ad doesn't
    // carry (region_name, bounds, zoom) — graceful degradation until S4
    // aligns it. `tileset_id`/`approx_size_bytes` already match.
    let advertisements_json = serde_json::to_string(&serde_json::json!([{
        "tileset_id": ad.tileset_id,
        "root_hash": ad.root_hash,
        "total_chunks": ad.total_chunks,
        "approx_size_bytes": ad.approx_bytes,
    }]))
    .unwrap_or_else(|_| "[]".to_string());
    send_sync_event(
        evt_tx,
        SyncEvent::PeerTilesetAdvertisement {
            peer_id: signed_author
                .map(str::to_owned)
                .unwrap_or_else(|| crate::replicator::peer_did_key(&incoming.from)),
            advertisements_json,
        },
    );
    Some(root)
}

/// One [`TilesetAd`] built from a published catalog entry.
fn tileset_ad_from_catalog_entry(entry: &CatalogEntry, provider_addr: &str) -> TilesetAd {
    TilesetAd {
        kind: TILESET_AD_KIND.to_string(),
        tileset_id: entry.tileset_id.clone(),
        root_hash: hash_to_hex(&entry.root),
        total_chunks: entry.total_chunks,
        approx_bytes: entry.bytes,
        provider_addr: provider_addr.to_string(),
        publisher_did: entry.publisher_did.clone(),
    }
}

/// Broadcast every ad on one verse topic: signed batches of [`TILESET_ADS_PER_FRAME`] when `signer` is set.
async fn broadcast_ads_on(
    sender: &TopicSender,
    verse_id: &str,
    signer: Option<&iroh::SecretKey>,
    ads: &[TilesetAd],
) {
    if let Some(secret) = signer {
        for batch in ads.chunks(TILESET_ADS_PER_FRAME) {
            let frame = serde_json::to_vec(batch).map_err(|e| e.to_string()).and_then(|payload| {
                crate::gossip_auth::seal(secret, GossipFrameKind::TilesetAd, verse_id, unix_now_ms(), payload)
                    .map_err(str::to_owned)
            });
            match frame {
                Ok(frame) => match sender.broadcast(frame.into()).await {
                    Ok(()) => tracing::debug!(verse_id, ads = batch.len(), "Broadcasted signed tileset advertisements"),
                    Err(e) => tracing::warn!(verse_id, "Failed to broadcast signed tileset advertisements: {e}"),
                },
                Err(e) => tracing::warn!(verse_id, "Failed to seal tileset advertisements: {e}"),
            }
        }
        return;
    }
    for ad in ads {
        match serde_json::to_vec(ad) {
            Ok(payload) => {
                if let Err(e) = sender.broadcast(payload.into()).await {
                    tracing::warn!(tileset_id = %ad.tileset_id, "Failed to broadcast tileset advertisement: {e}");
                } else {
                    tracing::debug!(tileset_id = %ad.tileset_id, "Broadcasted tileset advertisement");
                }
            }
            Err(e) => {
                tracing::error!(tileset_id = %ad.tileset_id, "Failed to serialize advertisement: {e}")
            }
        }
    }
}

/// Broadcast every ad on every currently-joined verse topic (publish/
/// unpublish have no single verse to target — a newly seeded tileset is
/// relevant to every verse this node currently participates in).
async fn broadcast_ads_to_all_verses(
    gossip_senders: &HashMap<String, TopicSender>,
    signer: Option<&iroh::SecretKey>,
    ads: &[TilesetAd],
) {
    for (topic_key, sender) in gossip_senders {
        let Some(verse_id) = verse_of_gossip_topic(topic_key) else {
            tracing::warn!(topic_key, "Gossip topic key names no verse — tileset ads not broadcast on it");
            continue;
        };
        broadcast_ads_on(sender, verse_id, signer, ads).await;
    }
}

/// Handle [`SyncCommand::AdvertiseTilesets`].
///
/// Outbound ads are built from the published catalog, never from the
/// command's payload (see the variant's doc comment) — broadcast on one
/// verse's topic.
async fn handle_advertise_tilesets(
    gossip_host: &Option<Gossip>,
    gossip_senders: &HashMap<String, TopicSender>,
    transfer: &Option<Arc<TilesetTransfer>>,
    provider_addr: Option<&str>,
    signer: Option<&iroh::SecretKey>,
    verse_id: &str,
) {
    if gossip_host.is_none() {
        tracing::debug!("No gossip, skipping tileset advertise");
        return;
    }
    let Some(transfer) = transfer else {
        tracing::debug!("No tileset transfer (offline) — nothing to advertise");
        return;
    };
    let Some(provider_addr) = provider_addr else {
        tracing::debug!("No dialable address yet — skipping tileset advertise");
        return;
    };
    let catalog = transfer.catalog();
    if catalog.is_empty() {
        tracing::debug!("Catalog empty — nothing to advertise");
        return;
    }
    let topic_key = derive_gossip_topic(verse_id);
    let Some(sender) = gossip_senders.get(&topic_key) else {
        tracing::warn!(
            verse_id,
            "No gossip topic for verse, cannot advertise tilesets"
        );
        return;
    };
    let ads: Vec<TilesetAd> = catalog
        .iter()
        .map(|entry| tileset_ad_from_catalog_entry(entry, provider_addr))
        .collect();
    broadcast_ads_on(sender, verse_id, signer, &ads).await;
}

/// Forward each [`TransferEvent`] from a tileset transfer operation as a
/// [`SyncEvent`] — spawned alongside the operation driving it
/// (publish/download/resume) so the command loop's `select!` never awaits a
/// transfer inline.
async fn pump_transfer_progress(
    mut rx: tokio::sync::mpsc::UnboundedReceiver<TransferEvent>,
    evt_tx: EventOutbox,
) {
    while let Some(event) = rx.recv().await {
        send_sync_event(&evt_tx, transfer_event_to_sync_event(event));
    }
}

/// Start-up pass over persisted tilesets (spawned — never on the select!
/// loop): rebuild the catalog from `fe/tileset/` tags, report each persisted
/// FULL download (`TilesetDownloadResumed`, counts before any re-fetch),
/// hand back the ones already complete (`TilesetDownloadComplete`, so a
/// restart between completion and install still installs), then resume the
/// rest. A streamed-bundle cache is a cache, not a download, so it is never
/// reported. See `AGENTS.md` §tileset-transfer.
async fn report_and_resume_tileset_downloads(transfer: Arc<TilesetTransfer>, evt_tx: EventOutbox) {
    if let Err(e) = transfer.catalog_from_tags().await {
        tracing::warn!("Rebuilding tileset catalog from tags failed: {e}");
    }
    // OF-6b: re-derive the blob gate's child index for every persisted
    // download (published ones were reported by the catalog rebuild above).
    if let Err(e) = transfer.report_downloaded_content().await {
        tracing::warn!("Reporting persisted tileset downloads to the blob gate failed: {e}");
    }
    // AC-21(a): streamed caches persist across restarts; re-apply the budget.
    if let Err(e) = transfer.trim_streamed_caches().await {
        tracing::warn!("Trimming streamed tileset caches failed: {e}");
    }
    match transfer.pending_downloads().await {
        Ok(pending) => {
            // A full download that was never confirmed predates the S7a gate:
            // nothing will ever confirm it, so it is discarded, never installed.
            for stale in pending.iter().filter(|p| p.full_download && !p.confirmed) {
                tracing::info!(
                    tileset_id = %stale.tileset_id,
                    "Discarding an unconfirmed tileset download (never re-trust an unverified header)"
                );
                if let Err(e) = transfer.cancel(&stale.tileset_id).await {
                    tracing::warn!(tileset_id = %stale.tileset_id, "Discarding it failed: {e}");
                }
            }
            for resumed in pending.iter().filter(|p| p.full_download && p.confirmed) {
                tracing::info!(
                    tileset_id = %resumed.tileset_id,
                    received = resumed.received,
                    total = resumed.total,
                    "Found a persisted tileset download at startup"
                );
                send_sync_event(
                    &evt_tx,
                    SyncEvent::TilesetDownloadResumed {
                        tileset_id: resumed.tileset_id.clone(),
                        received: resumed.received,
                        total: resumed.total,
                    },
                );
            }
            for completed in transfer.completed_from(&pending) {
                send_sync_event(
                    &evt_tx,
                    transfer_event_to_sync_event(TransferEvent::Complete(completed)),
                );
            }
        }
        Err(e) => tracing::warn!("Listing pending tileset downloads failed: {e}"),
    }
    let (progress_tx, progress_rx) = tokio::sync::mpsc::unbounded_channel();
    let pump = tokio::spawn(pump_transfer_progress(progress_rx, evt_tx));
    if let Err(e) = transfer.resume_all(Some(&progress_tx)).await {
        tracing::warn!("Resuming tileset downloads failed: {e}");
    }
    drop(progress_tx);
    let _ = pump.await;
}

/// Pure mapping from a transfer-progress event to the wire event.
fn transfer_event_to_sync_event(event: TransferEvent) -> SyncEvent {
    match event {
        TransferEvent::ChunkStored { tileset_id, chunk } => SyncEvent::ChunkReceived {
            tileset_id,
            chunk_seq: chunk.seq,
            path: chunk.path,
            byte_len: chunk.byte_len,
        },
        TransferEvent::ChunkFailed {
            tileset_id,
            seq,
            error,
        } => SyncEvent::ChunkFailed {
            tileset_id,
            chunk_seq: seq,
            reason: error.to_string(),
        },
        TransferEvent::Complete(completed) => SyncEvent::TilesetDownloadComplete {
            tileset_id: completed.tileset_id,
            index_path: completed.index_path,
            bundle_paths: completed.bundle_paths,
        },
    }
}

/// A [`SyncEvent::TilesetMetaReceived`] for a resolved meta request.
fn meta_resolved_event(
    peer_id: String,
    resolved: &MetaResolved,
    approx_size_bytes: u64,
    ad_publisher_did: String,
) -> SyncEvent {
    SyncEvent::TilesetMetaReceived {
        peer_id,
        tileset_id: resolved.tileset_id.clone(),
        meta_json: String::from_utf8_lossy(&resolved.index_json).into_owned(),
        total_chunks: resolved.chunk_hashes.len() as u32,
        approx_size_bytes,
        root_hash: hash_to_hex(&resolved.root),
        chunk_hashes: resolved.chunk_hashes.iter().map(hash_to_hex).collect(),
        index_path: resolved.index_path.clone(),
        ad_publisher_did,
    }
}

/// An honest-failure [`SyncEvent::TilesetMetaReceived`] (never a silent
/// drop): every resolved field is empty/zero and `meta_json` names why.
fn failed_meta_event(peer_id: String, tileset_id: String, reason: &str) -> SyncEvent {
    SyncEvent::TilesetMetaReceived {
        peer_id,
        tileset_id,
        meta_json: serde_json::json!({ "error": reason }).to_string(),
        total_chunks: 0,
        approx_size_bytes: 0,
        root_hash: String::new(),
        chunk_hashes: Vec::new(),
        index_path: PathBuf::new(),
        ad_publisher_did: String::new(),
    }
}

/// [`failed_meta_event`] for the virtual-transport / offline case.
fn offline_meta_event(peer_id: String, tileset_id: String) -> SyncEvent {
    failed_meta_event(
        peer_id,
        tileset_id,
        "P2P offline — no tileset transfer available",
    )
}

/// OF-6a / R1: bind a root this node is about to fetch to its fetch context
/// ([`TilesetProviderMap::verses_for`]), BEFORE the fetch puts any of it in
/// the store (the gate then re-serves it only to a Viewer+ of all of them).
/// Only the FIRST fetch decision binds; see `BlobAccessGate::bind_fetched_root`.
///
/// Provenance rule (R1, flag since R6/OF-10c): when any of `providers` never
/// VOUCHED for this tileset/root itself (a relayed hint's named provider, or
/// an explicit override nobody vouched for), the root is bound as
/// `never_seed` — fetched for local use but never secondary-seeded. A hint
/// can name a third-party provider that serves THIS node on the strength of
/// its membership in some other verse; re-serving those bytes to the hint's
/// verse would launder them one hop past the provider's own policy.
fn bind_fetched_root(
    gate: Option<&BlobAccessGate>,
    providers_map: &TilesetProviderMap,
    tileset_id: &str,
    root: Hash32,
    providers: &[ProviderRef],
) {
    if let Some(gate) = gate {
        let verses = providers_map.verses_for(tileset_id, root);
        let vouched = providers_map.vouched_providers(tileset_id, root);
        let all_vouched = providers.iter().all(|provider| {
            IrohBlobTransport::parse_provider(provider).is_ok_and(|addr| vouched.contains(&addr.id))
        });
        gate.bind_fetched_root(root, verses.iter().map(String::as_str), all_vouched);
    }
}

/// Handle [`SyncCommand::RequestTilesetMeta`]: resolve the tileset's root
/// from a prior advertisement (or `peer_id` as an explicit provider
/// override) and fetch + verify its index. Spawned so the command loop
/// never awaits the network fetch inline.
fn handle_request_tileset_meta(
    transfer: &Option<Arc<TilesetTransfer>>,
    providers_map: &TilesetProviderMap,
    gate: Option<&BlobAccessGate>,
    peer_id: &str,
    tileset_id: &str,
    evt_tx: &EventOutbox,
) {
    let Some(transfer) = transfer.clone() else {
        send_sync_event(
            evt_tx,
            offline_meta_event(peer_id.to_string(), tileset_id.to_string()),
        );
        return;
    };
    let explicit = (!peer_id.is_empty()).then_some(peer_id);
    let (providers, root, ad_publisher_did) =
        match resolve_providers_and_root(providers_map, tileset_id, explicit) {
            Ok(ok) => ok,
            Err(reason) => {
                send_sync_event(
                    evt_tx,
                    failed_meta_event(peer_id.to_string(), tileset_id.to_string(), reason),
                );
                return;
            }
        };
    bind_fetched_root(gate, providers_map, tileset_id, root, &providers);
    let approx_size_bytes = providers_map.bytes_for(tileset_id);
    let peer_id = peer_id.to_string();
    let tileset_id = tileset_id.to_string();
    let evt_tx = evt_tx.clone();
    tokio::spawn(async move {
        match transfer.request_meta(&providers, &tileset_id, root).await {
            Ok(resolved) => send_sync_event(
                &evt_tx,
                meta_resolved_event(peer_id, &resolved, approx_size_bytes, ad_publisher_did),
            ),
            Err(e) => send_sync_event(
                &evt_tx,
                failed_meta_event(peer_id, tileset_id, &e.to_string()),
            ),
        }
    });
}

/// Handle [`SyncCommand::DownloadTileset`]: resolve the root and fetch +
/// verify its index ONLY. **No chunk is requested** — PAS-P5 S7a's
/// two-phase gate: the host must independently verify the signed header
/// (off this `TilesetMetaReceived` reply) and send
/// [`SyncCommand::ConfirmTilesetDownload`] before
/// [`handle_confirm_tileset_download`] ever calls `TilesetTransfer::download`.
/// Spawned (never awaited inline).
fn handle_download_tileset(
    transfer: &Option<Arc<TilesetTransfer>>,
    providers_map: &TilesetProviderMap,
    gate: Option<&BlobAccessGate>,
    tileset_id: String,
    provider: Option<String>,
    evt_tx: &EventOutbox,
) {
    let Some(transfer) = transfer.clone() else {
        send_sync_event(evt_tx, offline_meta_event(String::new(), tileset_id));
        return;
    };
    let (providers, root, ad_publisher_did) =
        match resolve_providers_and_root(providers_map, &tileset_id, provider.as_deref()) {
            Ok(ok) => ok,
            Err(reason) => {
                send_sync_event(evt_tx, failed_meta_event(String::new(), tileset_id, reason));
                return;
            }
        };
    bind_fetched_root(gate, providers_map, &tileset_id, root, &providers);
    let approx_size_bytes = providers_map.bytes_for(&tileset_id);
    let evt_tx = evt_tx.clone();
    tokio::spawn(async move {
        match transfer
            .request_download_meta(&providers, &tileset_id, root)
            .await
        {
            Ok(resolved) => {
                send_sync_event(
                    &evt_tx,
                    meta_resolved_event(
                        String::new(),
                        &resolved,
                        approx_size_bytes,
                        ad_publisher_did,
                    ),
                );
            }
            Err(e) => {
                tracing::warn!(tileset_id = %tileset_id, "Tileset meta resolution failed: {e}");
                send_sync_event(
                    &evt_tx,
                    failed_meta_event(String::new(), tileset_id.clone(), &e.to_string()),
                );
            }
        }
    });
}

/// Handle [`SyncCommand::ConfirmTilesetDownload`]: the host has
/// independently verified the signed header this download's
/// `TilesetMetaReceived` reply carried and matched `expected_chunk_hashes`
/// against it (PAS-P5 S7a). Record the confirmation
/// (`TilesetTransfer::confirm`, persisted so a restart never re-trusts an
/// unverified header) and only then fetch every bundle. A confirmation
/// refusal (a mismatch against what the HashSeq itself resolved) cancels
/// the download outright rather than leaving it stuck awaiting a
/// confirmation that will never match. Spawned (never awaited inline).
fn handle_confirm_tileset_download(
    transfer: &Option<Arc<TilesetTransfer>>,
    tileset_id: String,
    expected_chunk_hashes: Vec<Hash32>,
    evt_tx: &EventOutbox,
) {
    let Some(transfer) = transfer.clone() else {
        tracing::debug!(tileset_id, "ConfirmTilesetDownload ignored — P2P offline");
        return;
    };
    let evt_tx = evt_tx.clone();
    tokio::spawn(async move {
        if let Err(e) = transfer.confirm(&tileset_id, &expected_chunk_hashes).await {
            if matches!(e, crate::tileset_transfer::TransferError::NotRequested(_)) {
                // Not an install download (e.g. a streamed cache): leave it be.
                tracing::debug!(tileset_id, "ConfirmTilesetDownload ignored: {e}");
                return;
            }
            tracing::warn!(tileset_id, "Tileset download confirmation refused: {e}");
            send_sync_event(
                &evt_tx,
                failed_meta_event(String::new(), tileset_id.clone(), &e.to_string()),
            );
            if let Err(e) = transfer.cancel(&tileset_id).await {
                tracing::warn!(tileset_id, "Cancelling an unconfirmed download failed: {e}");
            }
            return;
        }
        let (progress_tx, progress_rx) = tokio::sync::mpsc::unbounded_channel();
        let pump = tokio::spawn(pump_transfer_progress(progress_rx, evt_tx.clone()));
        if let Err(e) = transfer.download(&tileset_id, Some(&progress_tx)).await {
            tracing::warn!(tileset_id = %tileset_id, "Confirmed tileset download failed: {e}");
            // Never a silent stop (e.g. `DownloadTooLarge`): the UI's
            // tracker reads an empty-root meta reply as Failed.
            send_sync_event(
                &evt_tx,
                failed_meta_event(String::new(), tileset_id.clone(), &e.to_string()),
            );
        }
        drop(progress_tx);
        let _ = pump.await;
    });
}

/// Handle [`SyncCommand::RequestChunk`]: fetch one already-meta-resolved
/// bundle. Spawned (never awaited inline).
///
/// **Known limitation (S7 follow-up):** an explicit `provider` override is
/// accepted on the wire (`SyncCommand::RequestChunk::provider`) but not yet
/// consulted here — a chunk fetch always uses the providers a prior
/// `RequestTilesetMeta`/`DownloadTileset` already resolved into the
/// tileset's ledger. S7's streamed (not-installed) tileset source resolves
/// meta before it ever requests a chunk, so this is not a gap for that flow;
/// it would only matter for a chunk request with no prior meta resolution,
/// which no caller does today.
fn handle_request_chunk(
    transfer: &Option<Arc<TilesetTransfer>>,
    tileset_id: &str,
    chunk_seq: u32,
    evt_tx: &EventOutbox,
) {
    let Some(transfer) = transfer.clone() else {
        send_sync_event(
            evt_tx,
            SyncEvent::ChunkFailed {
                tileset_id: tileset_id.to_string(),
                chunk_seq,
                reason: "P2P offline — no tileset transfer available".to_string(),
            },
        );
        return;
    };
    let tileset_id = tileset_id.to_string();
    let evt_tx = evt_tx.clone();
    tokio::spawn(async move {
        match transfer.fetch_chunk(&tileset_id, chunk_seq).await {
            Ok(chunk) => send_sync_event(
                &evt_tx,
                SyncEvent::ChunkReceived {
                    tileset_id,
                    chunk_seq,
                    path: chunk.path,
                    byte_len: chunk.byte_len,
                },
            ),
            Err(e) => send_sync_event(
                &evt_tx,
                SyncEvent::ChunkFailed {
                    tileset_id,
                    chunk_seq,
                    reason: e.to_string(),
                },
            ),
        }
    });
}

/// Handle [`SyncCommand::CancelTilesetDownload`]. Spawned (never awaited
/// inline) — `TilesetTransfer::cancel` does filesystem work.
fn handle_cancel_tileset_download(transfer: &Option<Arc<TilesetTransfer>>, tileset_id: String) {
    let Some(transfer) = transfer.clone() else {
        tracing::debug!(tileset_id, "CancelTilesetDownload ignored — P2P offline");
        return;
    };
    tokio::spawn(async move {
        match transfer.cancel(&tileset_id).await {
            Ok(()) => tracing::info!(tileset_id, "Cancelled tileset download"),
            Err(e) => tracing::warn!(tileset_id, "Cancel tileset download failed: {e}"),
        }
    });
}

/// Handle [`SyncCommand::PublishTileset`]: publish, then broadcast an ad on
/// every currently-joined verse topic. Spawned (never awaited inline).
fn spawn_publish_tileset(
    transfer: Option<Arc<TilesetTransfer>>,
    evt_tx: EventOutbox,
    gossip_senders: HashMap<String, TopicSender>,
    provider_addr: Option<String>,
    signer: Option<iroh::SecretKey>,
    tileset_id: String,
    index_path: PathBuf,
    bundle_paths: Vec<PathBuf>,
) {
    let Some(transfer) = transfer else {
        tracing::debug!(tileset_id, "PublishTileset ignored — P2P offline");
        return;
    };
    tokio::spawn(async move {
        match transfer
            .publish_files(&tileset_id, &index_path, &bundle_paths)
            .await
        {
            Ok(entry) => {
                tracing::info!(tileset_id = %entry.tileset_id, bundles = entry.total_chunks, "Tileset published");
                send_sync_event(
                    &evt_tx,
                    SyncEvent::TilesetPublished {
                        tileset_id: entry.tileset_id.clone(),
                        root_hash: hash_to_hex(&entry.root),
                    },
                );
                match provider_addr {
                    Some(addr) => {
                        broadcast_ads_to_all_verses(
                            &gossip_senders,
                            signer.as_ref(),
                            &[tileset_ad_from_catalog_entry(&entry, &addr)],
                        )
                        .await;
                    }
                    None => tracing::debug!(
                        tileset_id = %entry.tileset_id,
                        "Published but no dialable address yet — not advertised"
                    ),
                }
            }
            Err(e) => tracing::warn!(tileset_id, "Tileset publish failed: {e}"),
        }
    });
}

/// Handle [`SyncCommand::UnpublishTileset`]. Spawned (never awaited inline).
fn spawn_unpublish_tileset(transfer: Option<Arc<TilesetTransfer>>, tileset_id: String) {
    let Some(transfer) = transfer else {
        tracing::debug!(tileset_id, "UnpublishTileset ignored — P2P offline");
        return;
    };
    tokio::spawn(async move {
        match transfer.unpublish(&tileset_id).await {
            Ok(was_published) => tracing::info!(tileset_id, was_published, "Tileset unpublished"),
            Err(e) => tracing::warn!(tileset_id, "Tileset unpublish failed: {e}"),
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::replicator::MockVerseReplicator;
    use fe_runtime::blob_store::mock::MockBlobStore;
    use std::sync::Arc;

    /// SYNC-RELAYWATCH acceptance: home-relay snapshots fold through the
    /// watcher's emit seam into `SyncStatus.health` (Unknown → Healthy →
    /// Degraded → Healthy), and an unchanged snapshot emits nothing.
    #[tokio::test]
    async fn relay_watch_drives_health_into_sync_status() {
        use crate::status::{
            drain_sync_events, SyncEventReceiverRes, SyncStatus, TilesetEventBuffer,
        };
        use bevy::prelude::{App, Update};

        let (evt_tx, evt_rx) = crossbeam::channel::bounded(8);
        let mut app = App::new();
        app.init_resource::<SyncStatus>();
        app.init_resource::<TilesetEventBuffer>();
        app.insert_resource(SyncEventReceiverRes(Arc::new(std::sync::Mutex::new(
            evt_rx,
        ))));
        app.add_systems(Update, drain_sync_events);

        let connected = [RelayStatusSummary {
            connected: true,
            ..Default::default()
        }];
        let errored = [RelayStatusSummary {
            has_error: true,
            ..Default::default()
        }];
        let queued = |app: &App| {
            app.world()
                .resource::<SyncEventReceiverRes>()
                .0
                .lock()
                .unwrap()
                .len()
        };
        let mut health = RelayHealth::Unknown;
        for (snapshot, expected) in [
            (&connected[..], RelayHealth::Healthy),
            (&errored[..], RelayHealth::Degraded),
            (&connected[..], RelayHealth::Healthy),
            // Still connected: no transition, so no event.
            (&connected[..], RelayHealth::Healthy),
        ] {
            let before = health;
            health = fold_relay_health(health, snapshot, &evt_tx);
            assert_eq!(health, expected);
            assert_eq!(queued(&app), usize::from(before != expected));
            app.update();
            assert_eq!(app.world().resource::<SyncStatus>().health, expected);
        }

        // DialOnly allowlist filter: a failing host is excluded, a cached verdict is not re-resolved inside the TTL.
        let good = "https://relay-a.example/".to_string();
        let bad = "https://relay-b.example/".to_string();
        let urls = [good.clone(), bad.clone()];
        let mut cache = HashMap::new();
        let start = std::time::Instant::now();
        let calls = std::cell::Cell::new(0usize);
        let mut resolver = |url: &str| {
            calls.set(calls.get() + 1);
            if url.contains("relay-b") {
                Err("resolves to loopback".to_string())
            } else {
                Ok(())
            }
        };
        let kept = filter_resolvable(&urls, &mut cache, start, &mut resolver);
        assert_eq!(kept, BTreeSet::from([good.clone()]));
        let kept = filter_resolvable(
            &urls,
            &mut cache,
            start + std::time::Duration::from_secs(60),
            &mut resolver,
        );
        assert_eq!(kept, BTreeSet::from([good.clone()]));
        assert_eq!(calls.get(), 2, "verdicts inside the TTL are served from the cache");
        let kept = filter_resolvable(
            &urls,
            &mut cache,
            start + DIAL_ONLY_VERDICT_TTL + std::time::Duration::from_secs(1),
            &mut resolver,
        );
        assert_eq!(kept, BTreeSet::from([good.clone()]));
        assert_eq!(calls.get(), 4, "both URLs are re-resolved after the TTL");

        // Live-endpoint legs (AC-6/7/8): planned ops applied to a real iroh endpoint. IP-literal public https URLs need no DNS and no private-relay opt-in.
        const PINNED_RELAY: &str = "https://203.0.113.10/";
        const CONFIGURED_RELAY: &str = "https://203.0.113.20/";
        let endpoint = SyncEndpoint::new(iroh::SecretKey::from_bytes(&[7u8; 32]), &RelayConfig::Disabled)
            .await
            .expect("endpoint binds with relays disabled");
        // iroh 1.3 has no public relay-map getter: swap in an equivalent entry, read the previous one, then restore the map.
        async fn relay_entry(endpoint: &SyncEndpoint, raw: &str) -> Option<Arc<iroh::RelayConfig>> {
            let url: iroh::RelayUrl = raw.parse().expect("relay url");
            let previous = endpoint
                .inner()
                .insert_relay(url.clone(), advertised_relay_entry(url.clone()))
                .await;
            match &previous {
                Some(entry) => {
                    endpoint.inner().insert_relay(url, entry.clone()).await;
                }
                None => {
                    endpoint.inner().remove_relay(&url).await;
                }
            }
            previous
        }
        async fn apply(adoption: &mut RelayAdoption, ops: Vec<RelayOp>, endpoint: &SyncEndpoint) {
            for op in ops {
                match op {
                    RelayOp::Insert(raw) => insert_planned_relay(adoption, &raw, Some(endpoint)).await,
                    RelayOp::Remove(raw) => remove_planned_relay(adoption, &raw, Some(endpoint)).await,
                    RelayOp::Known(_) => {}
                }
            }
        }
        let pinned_policy = RelayAdvertisePolicy::Pinned(vec![PINNED_RELAY.to_string()]);
        let plan = |adoption: &mut RelayAdoption, advertised: &BTreeMap<String, BTreeSet<String>>| {
            adoption.reconciler.plan_tiers(&RelayReconcileInputs {
                policy: &pinned_policy,
                advertised,
                home_connected: None,
                coverage: BTreeSet::new(),
                auto_home: false,
                home_health: RelayHealth::Unknown,
            })
        };
        let adverts = |rows: &[(&str, &[&str])]| -> BTreeMap<String, BTreeSet<String>> {
            rows.iter()
                .map(|(verse, urls)| (verse.to_string(), urls.iter().map(|url| url.to_string()).collect()))
                .collect()
        };
        let mut adoption = RelayAdoption::new(RelayReconciler::from_relay_config(&RelayConfig::Disabled));
        let insert_op = RelayOp::Insert(PINNED_RELAY.to_string());
        let remove_op = RelayOp::Remove(PINNED_RELAY.to_string());

        // AC-7: a pinned advert enters the live relay map without a token and without a restart.
        assert!(relay_entry(&endpoint, PINNED_RELAY).await.is_none(), "map starts without the relay");
        let both = adverts(&[("verse-a", &[PINNED_RELAY]), ("verse-b", &[PINNED_RELAY])]);
        let ops = plan(&mut adoption, &both);
        assert_eq!(ops, vec![insert_op.clone()]);
        apply(&mut adoption, ops, &endpoint).await;
        let entry = relay_entry(&endpoint, PINNED_RELAY).await.expect("pinned relay is in the live map");
        assert!(entry.auth_token.is_none(), "an adopted relay never carries a token");

        // AC-8: one verse withdrawing keeps the relay; the last withdrawal removes it.
        let ops = plan(&mut adoption, &adverts(&[("verse-b", &[PINNED_RELAY])]));
        assert!(ops.is_empty(), "refcount 1 keeps the relay: {ops:?}");
        assert!(relay_entry(&endpoint, PINNED_RELAY).await.is_some());
        let ops = plan(&mut adoption, &adverts(&[]));
        assert_eq!(ops, vec![remove_op.clone()]);
        apply(&mut adoption, ops, &endpoint).await;
        assert!(relay_entry(&endpoint, PINNED_RELAY).await.is_none(), "refcount zero removes it from the live map");

        // AC-6: three failed health checks remove an adopted relay and keep it out while cooled.
        let ops = plan(&mut adoption, &both);
        apply(&mut adoption, ops, &endpoint).await;
        assert!(relay_entry(&endpoint, PINNED_RELAY).await.is_some());
        adoption.replan = false;
        let start = std::time::Instant::now();
        for _ in 1..ADOPTED_RELAY_FAILURE_LIMIT {
            adoption.record_probe(PINNED_RELAY, false, start);
        }
        assert!(!adoption.replan && !adoption.health.is_failed(PINNED_RELAY), "below the limit nothing is removed");
        adoption.record_probe(PINNED_RELAY, false, start);
        assert!(adoption.replan && adoption.health.is_failed(PINNED_RELAY));
        // The same filter reconcile_advertised_relays applies before planning.
        let without_failed = |adoption: &RelayAdoption| {
            let mut advertised = both.clone();
            drop_failed_relays(&adoption.health, &mut advertised);
            advertised
        };
        let advertised = without_failed(&adoption);
        let ops = plan(&mut adoption, &advertised);
        assert_eq!(ops, vec![remove_op.clone()]);
        apply(&mut adoption, ops, &endpoint).await;
        assert!(relay_entry(&endpoint, PINNED_RELAY).await.is_none(), "unhealthy adopted relay left the live map");
        let advertised = without_failed(&adoption);
        let ops = plan(&mut adoption, &advertised);
        assert!(ops.is_empty(), "a cooled relay is not re-inserted: {ops:?}");
        assert!(adoption.health.release_cooled(start + ADOPTED_RELAY_FAILED_COOLDOWN + std::time::Duration::from_secs(1)));
        let advertised = without_failed(&adoption);
        let ops = plan(&mut adoption, &advertised);
        assert_eq!(ops, vec![insert_op], "after the cooldown the relay is adopted again");
        apply(&mut adoption, ops, &endpoint).await;
        assert!(relay_entry(&endpoint, PINNED_RELAY).await.is_some());

        // A configured relay keeps its token entry: the planner never plans it and a direct removal is refused.
        let configured_url: iroh::RelayUrl = CONFIGURED_RELAY.parse().expect("relay url");
        endpoint
            .inner()
            .insert_relay(
                configured_url.clone(),
                Arc::new(iroh::RelayConfig::new(configured_url, None).with_auth_token("operator-token")),
            )
            .await;
        let mut configured_adoption =
            RelayAdoption::new(RelayReconciler::new(BTreeSet::from([CONFIGURED_RELAY.to_string()])));
        let ops = plan(&mut configured_adoption, &adverts(&[("verse-a", &[CONFIGURED_RELAY])]));
        assert!(ops.is_empty(), "an advert naming a configured relay plans nothing: {ops:?}");
        apply(&mut configured_adoption, vec![RelayOp::Remove(CONFIGURED_RELAY.to_string())], &endpoint).await;
        let kept = relay_entry(&endpoint, CONFIGURED_RELAY).await.expect("configured relay survives");
        assert_eq!(kept.auth_token.as_deref(), Some("operator-token"));
        endpoint.shutdown().await;
    }

    #[test]
    fn fetch_blob_local_hit_requires_current_admission() {
        let store: BlobStoreHandle = Arc::new(MockBlobStore::new());
        let hash = store.add_blob(b"test data").unwrap();
        let (evt_tx, evt_rx) = crossbeam::channel::bounded(8);
        let (publication_tx, _) = tokio::sync::mpsc::channel(1);
        let mut jobs = crate::scene_assets::SceneAssetJobs::new(
            store,
            None,
            Default::default(),
            None,
            crate::event_outbox::EventOutbox::new(evt_tx),
            publication_tx,
        );
        handle_fetch_blob(&mut jobs, &hash, "verse-1");
        assert!(matches!(
            evt_rx.try_recv().unwrap(),
            SyncEvent::SceneAssetFetchFailed { .. }
        ));
        assert!(
            evt_rx.try_recv().is_err(),
            "presence alone never emits BlobReady"
        );
    }

    #[test]
    fn fetch_blob_miss_does_not_emit_ready() {
        let store: BlobStoreHandle = Arc::new(MockBlobStore::new());
        let (evt_tx, evt_rx) = crossbeam::channel::bounded(8);
        let (publication_tx, _) = tokio::sync::mpsc::channel(1);
        let mut jobs = crate::scene_assets::SceneAssetJobs::new(
            store,
            None,
            Default::default(),
            None,
            crate::event_outbox::EventOutbox::new(evt_tx),
            publication_tx,
        );
        handle_fetch_blob(&mut jobs, &[0xABu8; 32], "verse-2");
        assert!(matches!(
            evt_rx.try_recv().unwrap(),
            SyncEvent::SceneAssetFetchFailed { .. }
        ));
        assert!(evt_rx.try_recv().is_err());
        // QA-0118: a row-arrival prefetch for a closed Verse is a silent no-op
        // (no failure noise, no queued job).
        jobs.prefetch("verse-2", [0xACu8; 32]);
        assert!(evt_rx.try_recv().is_err());
        assert_eq!(jobs.pending_job_count(), 0);
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

        // NET-28: the diagnostics command answers with a Disabled relay mode and zero peers.
        let (diagnostics_tx, diagnostics_rx) = crossbeam::channel::bounded(1);
        cmd_tx
            .send(SyncCommand::GetNetworkDiagnostics { detail: false, reply: diagnostics_tx })
            .unwrap();
        let diagnostics = diagnostics_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("diagnostics reply");
        assert_eq!(diagnostics.node.relay_mode, "disabled");
        assert!(diagnostics.peers.is_empty());

        cmd_tx.send(SyncCommand::Shutdown).unwrap();
        handle.join().expect("sync thread panicked");
        assert!(matches!(
            cmd_tx.try_send(SyncCommand::Shutdown),
            Err(crossbeam::channel::TrySendError::Disconnected(_))
        ), "the joined runtime must not leave a forwarder owning its receiver");

        // Drain any further RelayHealthChanged events (e.g. a P2P
        // stack-spawn failure in a sandboxed CI network) until the final
        // Stopped event.
        let stopped = loop {
            match evt_rx.recv_timeout(std::time::Duration::from_secs(2)) {
                Ok(SyncEvent::RelayHealthChanged { .. } | SyncEvent::NetworkDiagnostics(_)) => continue,
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
            _peers: Vec<iroh::EndpointAddr>,
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
    fn test_node_id(seed: u8) -> iroh::EndpointId {
        iroh::SecretKey::from_bytes(&[seed; 32]).public()
    }

    #[test]
    fn subscribe_to_verse_gossip_topic_no_host_is_noop() {
        let gossip_host: Option<Gossip> = None;
        let no_virtual: Option<Arc<dyn VirtualTransportFactory>> = None;
        let mut gossip_senders: HashMap<String, TopicSender> = HashMap::new();
        let mut gossip_pumps: HashMap<String, tokio::task::AbortHandle> = HashMap::new();
        let (tx, _rx) = tokio::sync::mpsc::channel::<GossipIncoming>(8);

        // Should not panic, just skip (returns before any await).
        futures_lite::future::block_on(subscribe_to_verse_gossip_topic(
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
        ));

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
            node: iroh::EndpointId,
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
                local_node: iroh::EndpointId,
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
            )
            .await;
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
            )
            .await;
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
            )
            .await;
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
    // Phase 5 / PAS-P3 S3: Tileset P2P Tests
    // -------------------------------------------------------------------------

    #[test]
    fn handle_advertise_tilesets_no_host_is_noop() {
        let gossip_host: Option<Gossip> = None;
        let gossip_senders: HashMap<String, TopicSender> = HashMap::new();
        let transfer: Option<Arc<TilesetTransfer>> = None;

        // Should not panic, just skip (no gossip host at all — the
        // "no transfer" and "no transfer" paths are table-tested below).
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(handle_advertise_tilesets(
            &gossip_host,
            &gossip_senders,
            &transfer,
            None,
            None,
            "verse-1",
        ));
    }

    #[test]
    fn provider_map_prefers_authenticated_roots_and_ignores_relayed_claims_about_them() {
        let mut map = TilesetProviderMap::default();
        let hint = ProviderRef::new("hint-addr");
        let real = ProviderRef::new("real-addr");
        assert!(map.record(
            "v1",
            "ts-1".to_string(),
            hint.clone(),
            [1u8; 32],
            false,
            10,
            "did:key:zhint".to_string(),
        ));
        assert!(map.record(
            "v1",
            "ts-1".to_string(),
            real.clone(),
            [2u8; 32],
            true,
            20,
            "did:key:zreal".to_string(),
        ));

        let choice = map.root_for("ts-1").unwrap();
        assert_eq!(choice.root, [2u8; 32]);
        assert!(choice.authenticated);
        assert_eq!(
            choice.publisher_did, "did:key:zreal",
            "PAS-P5 S7a critic: the pin candidate is the SAME entry root_for \
             chose, never a different entry's claim (checked below against \
             the hint's own did)"
        );
        assert_ne!(choice.publisher_did, "did:key:zhint");
        assert_eq!(map.bytes_for("ts-1"), 20);
        assert_eq!(map.candidates("ts-1", [2u8; 32]), vec![real.clone()]);
        assert_eq!(map.candidates("ts-1", [1u8; 32]), vec![hint]);

        // A relayed re-ad naming the authenticated provider is refused outright.
        assert!(!map.record(
            "v1",
            "ts-1".to_string(),
            real,
            [9u8; 32],
            false,
            999,
            "did:key:zattacker".to_string(),
        ));
        assert_eq!(map.root_for("ts-1").unwrap().root, [2u8; 32]);
        assert_eq!(map.root_for("ts-1").unwrap().publisher_did, "did:key:zreal");
        assert_eq!(map.bytes_for("ts-1"), 20);

        assert!(map.candidates("ts-unknown", [2u8; 32]).is_empty());
        assert_eq!(map.root_for("ts-unknown"), None);
        assert_eq!(map.bytes_for("ts-unknown"), 0);
    }

    #[test]
    fn resolve_providers_prefers_explicit_override_and_dedupes() {
        let mut map = TilesetProviderMap::default();
        let known = ProviderRef::new("known-addr");
        map.record(
            "v1",
            "ts-1".to_string(),
            known.clone(),
            [3u8; 32],
            true,
            0,
            String::new(),
        );

        // An explicit override that is ALSO already known appears once, first.
        let resolved = resolve_providers(&map, "ts-1", [3u8; 32], Some("known-addr"));
        assert_eq!(resolved, vec![known.clone()]);

        // A brand-new override is prepended ahead of the known candidate.
        let explicit_new = resolve_providers(&map, "ts-1", [3u8; 32], Some("override-addr"));
        assert_eq!(explicit_new, vec![ProviderRef::new("override-addr"), known]);

        // No root known at all -> an honest named error, not an empty Ok.
        assert_eq!(
            resolve_providers_and_root(&map, "ts-unknown", None),
            Err("no advertised root for this tileset yet")
        );
    }

    /// S8b mandatory finding #1 (automated review of S3 `e5d8928`): an
    /// authenticated ad for root A followed by a RELAYED ad naming the same
    /// provider with root B must not poison the entry — `root_for` still
    /// answers A and a real download fetches A's bytes, never B's. BND-10d:
    /// the root pins an extension source's bytes too — a bound
    /// `sync.tileset-source` is asked only for the child the peer missed, and
    /// only bytes hashing to that child land.
    #[test]
    fn a_relayed_ad_cannot_replace_an_authenticated_root() {
        let alice_id = iroh::SecretKey::from_bytes(&[121u8; 32]).public();
        let relay_neighbor = iroh::SecretKey::from_bytes(&[122u8; 32]).public();
        let alice_addr = serde_json::to_string(&iroh::EndpointAddr::new(alice_id)).unwrap();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        rt.block_on(async {
            let network = fe_runtime::blob_transport::InMemoryNetwork::new();
            let alice = network.transport(&alice_addr);
            let bob = network.transport("bob");
            let publish = |transport: fe_runtime::blob_transport::InMemoryTransport,
                           body: &'static [u8]| async move {
                let index = transport
                    .add(bytes::Bytes::from_static(b"{}"))
                    .await
                    .unwrap();
                let bundle = transport
                    .add(bytes::Bytes::from_static(body))
                    .await
                    .unwrap();
                transport.add_hash_seq(&[index, bundle]).await.unwrap()
            };
            let root_a = publish(alice.clone(), b"genuine bundle").await;
            let root_b = publish(alice.clone(), b"poisoned bundle").await;

            let (evt_tx, _evt_rx) = crossbeam::channel::unbounded();
            let mut map = TilesetProviderMap::default();
            let ad = |root: Hash32| TilesetAd {
                kind: TILESET_AD_KIND.to_string(),
                tileset_id: "valley".to_string(),
                root_hash: hash_to_hex(&root),
                total_chunks: 1,
                approx_bytes: 14,
                provider_addr: alice_addr.clone(),
                publisher_did: String::new(),
            };
            let incoming = |from, direct| GossipIncoming {
                verse_id: "verse-1".to_string(),
                from,
                direct,
                content: bytes::Bytes::new(),
            };
            handle_inbound_tileset_ad(&mut map, &evt_tx, &incoming(alice_id, true), ad(root_a));
            handle_inbound_tileset_ad(
                &mut map,
                &evt_tx,
                &incoming(relay_neighbor, false),
                ad(root_b),
            );

            let choice = map.root_for("valley").expect("a root is known");
            assert_eq!(
                choice.root, root_a,
                "the relayed ad must not replace root A"
            );
            assert!(choice.authenticated);

            let (providers, root, _ad_publisher_did) =
                resolve_providers_and_root(&map, "valley", None).unwrap();
            assert_eq!(root, root_a);
            let dir = tempfile::TempDir::new().unwrap();
            let transfer = TilesetTransfer::new(Arc::new(bob), dir.path());
            let report = transfer
                .start_download(&providers, "valley", root, None)
                .await
                .expect("the download runs");
            let complete = report.complete.expect("the download completes");
            assert_eq!(complete.root, root_a);
            assert_eq!(
                std::fs::read(&complete.bundle_paths[0]).unwrap(),
                b"genuine bundle"
            );

            /// A bound source answering per child hash, recording every ask.
            #[derive(Default)]
            struct ChildAnswers {
                answers: std::sync::Mutex<HashMap<Hash32, &'static [u8]>>,
                asked: std::sync::Mutex<Vec<Hash32>>,
            }
            impl fe_runtime::bevy_blob_reader::BlobFallback for ChildAnswers {
                fn source_blob(
                    &self,
                    _request: fe_runtime::bevy_blob_reader::BlobSourceRequest,
                ) -> fe_runtime::bevy_blob_reader::FallbackFuture<'_> {
                    Box::pin(async { None })
                }
                fn source_tileset_child(
                    &self,
                    request: fe_runtime::bevy_blob_reader::TilesetChildRequest,
                ) -> fe_runtime::bevy_blob_reader::FallbackFuture<'_> {
                    self.asked.lock().unwrap().push(request.child_hash);
                    let answer = self
                        .answers
                        .lock()
                        .unwrap()
                        .get(&request.child_hash)
                        .copied()
                        .map(bytes::Bytes::from_static);
                    Box::pin(async move { answer })
                }
            }

            // Alice now withholds the bundle: carol's peer pass misses, then her source is asked.
            let bundle = fe_runtime::blob_transport::content_hash(b"genuine bundle");
            alice.withhold(bundle);
            let source = Arc::new(ChildAnswers::default());
            source.answers.lock().unwrap().insert(bundle, b"poisoned bundle");
            let slot = BlobFallbackSlot::default();
            slot.install(source.clone());
            let carol_dir = tempfile::TempDir::new().unwrap();
            let carol = TilesetTransfer::new(Arc::new(network.transport("carol")), carol_dir.path())
                .with_child_source(slot);
            let forged = carol
                .start_download(&providers, "valley", root, None)
                .await
                .expect("the download runs");
            assert!(forged.complete.is_none(), "forged source bytes never complete a download");
            assert!(
                matches!(
                    forged.failed.as_slice(),
                    [(0, crate::tileset_transfer::ChunkError::Unavailable(_))]
                ),
                "the peers' miss stands: {:?}",
                forged.failed
            );
            assert!(!carol.download_dir("valley").unwrap().join("0.bundle").exists());

            source.answers.lock().unwrap().insert(bundle, b"genuine bundle");
            let sourced = carol.download("valley", None).await.unwrap();
            let complete = sourced.complete.expect("the source completes the download");
            assert_eq!(
                std::fs::read(&complete.bundle_paths[0]).unwrap(),
                b"genuine bundle"
            );
            assert_eq!(
                *source.asked.lock().unwrap(),
                vec![bundle, bundle],
                "only the missed bundle child is asked; the index came from the peer"
            );
        });
    }

    /// Final review: the start-up pass reports persisted FULL downloads only
    /// (a streamed-bundle cache is not a download the UI should track), and
    /// hands a download that completed before the restart back as
    /// `TilesetDownloadComplete` so the host still assembles + installs it.
    #[test]
    fn the_startup_pass_reports_downloads_and_hands_back_completed_ones() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let network = fe_runtime::blob_transport::InMemoryNetwork::new();
            let alice = network.transport("alice");
            let publish = |transport: fe_runtime::blob_transport::InMemoryTransport,
                           body: &'static [u8]| async move {
                let index = transport
                    .add(bytes::Bytes::from_static(b"{}"))
                    .await
                    .unwrap();
                let bundle = transport
                    .add(bytes::Bytes::from_static(body))
                    .await
                    .unwrap();
                transport.add_hash_seq(&[index, bundle]).await.unwrap()
            };
            let full_root = publish(alice.clone(), b"full bundle").await;
            let cache_root = publish(alice.clone(), b"cache bundle").await;
            let providers = [alice.provider_ref()];
            let dir = tempfile::TempDir::new().unwrap();
            {
                let bob = TilesetTransfer::new(Arc::new(network.transport("bob")), dir.path());
                let report = bob
                    .start_download(&providers, "full", full_root, None)
                    .await
                    .unwrap();
                assert!(report.complete.is_some());
                bob.request_meta(&providers, "cache", cache_root)
                    .await
                    .unwrap();
                bob.fetch_chunk("cache", 0).await.unwrap();
            }

            let restarted = Arc::new(TilesetTransfer::new(
                Arc::new(network.transport("bob-restarted")),
                dir.path(),
            ));
            let (evt_tx, evt_rx) = crossbeam::channel::unbounded();
            report_and_resume_tileset_downloads(restarted, EventOutbox::new(evt_tx)).await;
            let events: Vec<SyncEvent> = evt_rx.try_iter().collect();

            assert!(
                events
                    .iter()
                    .all(|e| !format!("{e:?}").contains("\"cache\"")),
                "a streamed cache is never reported: {events:?}"
            );
            assert!(events.iter().any(|e| matches!(e,
                SyncEvent::TilesetDownloadResumed { tileset_id, received: 1, total: 1 }
                    if tileset_id == "full")));
            let completed: Vec<_> = events
                .iter()
                .filter_map(|e| match e {
                    SyncEvent::TilesetDownloadComplete {
                        tileset_id,
                        bundle_paths,
                        ..
                    } => Some((tileset_id.clone(), bundle_paths.clone())),
                    _ => None,
                })
                .collect();
            assert_eq!(completed.len(), 1, "{events:?}");
            assert_eq!(completed[0].0, "full");
            assert_eq!(std::fs::read(&completed[0].1[0]).unwrap(), b"full bundle");
        });
    }

    /// Final review: a full download whose ledger was never confirmed (it
    /// predates the S7a gate) is neither reported nor handed back for
    /// install at start-up — it is discarded, since nothing will ever confirm it.
    #[test]
    fn an_unconfirmed_full_download_is_discarded_at_startup_never_installed() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let network = fe_runtime::blob_transport::InMemoryNetwork::new();
            let alice = network.transport("alice");
            let index = alice.add(bytes::Bytes::from_static(b"{}")).await.unwrap();
            let bundle = alice
                .add(bytes::Bytes::from_static(b"pre-gate bundle"))
                .await
                .unwrap();
            let root = alice.add_hash_seq(&[index, bundle]).await.unwrap();
            let dir = tempfile::TempDir::new().unwrap();
            let ledger_path = {
                let bob = TilesetTransfer::new(Arc::new(network.transport("bob")), dir.path());
                bob.start_download(&[alice.provider_ref()], "legacy", root, None)
                    .await
                    .unwrap();
                bob.download_dir("legacy")
                    .unwrap()
                    .join(crate::tileset_transfer::LEDGER_FILE)
            };
            // A pre-S7a ledger has no `confirmed` field (serde default false).
            let mut ledger: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&ledger_path).unwrap()).unwrap();
            ledger.as_object_mut().unwrap().remove("confirmed");
            std::fs::write(&ledger_path, serde_json::to_vec(&ledger).unwrap()).unwrap();

            let restarted = Arc::new(TilesetTransfer::new(
                Arc::new(network.transport("bob-restarted")),
                dir.path(),
            ));
            let (evt_tx, evt_rx) = crossbeam::channel::unbounded();
            report_and_resume_tileset_downloads(Arc::clone(&restarted), EventOutbox::new(evt_tx))
                .await;
            let events: Vec<SyncEvent> = evt_rx.try_iter().collect();
            assert!(
                events.iter().all(|e| !matches!(
                    e,
                    SyncEvent::TilesetDownloadComplete { .. }
                        | SyncEvent::TilesetDownloadResumed { .. }
                )),
                "an unconfirmed download is never handed to the installer: {events:?}"
            );
            assert!(
                !ledger_path.exists(),
                "the stale download is discarded from disk"
            );
            assert!(restarted.pending_downloads().await.unwrap().is_empty());
        });
    }

    /// S8b verse binding + ad-storm bounds: ads learned on a verse are
    /// forgotten when it closes, an unsafe id or undialable provider never
    /// enters the map, and a relayed flood cannot grow it past its caps.
    #[test]
    fn tileset_ads_are_verse_bound_validated_and_capped() {
        let alice_id = iroh::SecretKey::from_bytes(&[123u8; 32]).public();
        let alice_addr = serde_json::to_string(&iroh::EndpointAddr::new(alice_id)).unwrap();
        let (evt_tx, evt_rx) = crossbeam::channel::unbounded();
        let mut map = TilesetProviderMap::default();
        let ad = |tileset_id: &str, provider_addr: &str| TilesetAd {
            kind: TILESET_AD_KIND.to_string(),
            tileset_id: tileset_id.to_string(),
            root_hash: hash_to_hex(&[5u8; 32]),
            total_chunks: 1,
            approx_bytes: 1,
            provider_addr: provider_addr.to_string(),
            publisher_did: String::new(),
        };
        let on_verse = |verse: &str| GossipIncoming {
            verse_id: verse.to_string(),
            from: alice_id,
            direct: true,
            content: bytes::Bytes::new(),
        };

        for (tileset_id, provider) in [
            ("../escape", alice_addr.as_str()),
            ("CON", alice_addr.as_str()),
            ("ok", "not-an-endpoint-addr"),
        ] {
            handle_inbound_tileset_ad(
                &mut map,
                &evt_tx,
                &on_verse("verse-a"),
                ad(tileset_id, provider),
            );
        }
        assert!(map.by_id.is_empty(), "invalid ads never enter the map");
        assert!(evt_rx.try_recv().is_err(), "invalid ads never reach the UI");

        handle_inbound_tileset_ad(
            &mut map,
            &evt_tx,
            &on_verse("verse-a"),
            ad("valley", &alice_addr),
        );
        assert!(evt_rx.try_recv().is_ok(), "a valid ad is echoed to the UI");
        assert!(map.root_for("valley").is_some());
        map.forget_verse("verse-b");
        assert!(
            map.root_for("valley").is_some(),
            "another verse closing keeps it"
        );
        map.forget_verse("verse-a");
        assert_eq!(map.root_for("valley"), None, "closing its verse forgets it");

        for i in 0..(MAX_PROVIDERS_PER_TILESET * 2) {
            map.record(
                "v",
                "flood".to_string(),
                ProviderRef::new(format!("p{i}")),
                [i as u8; 32],
                false,
                1,
                String::new(),
            );
        }
        assert_eq!(map.by_id["flood"].len(), MAX_PROVIDERS_PER_TILESET);
        for i in 0..(MAX_TRACKED_TILESETS + 10) {
            map.record(
                "v",
                format!("id-{i}"),
                ProviderRef::new("p"),
                [1u8; 32],
                false,
                1,
                String::new(),
            );
        }
        assert!(map.by_id.len() <= MAX_TRACKED_TILESETS);
        assert!(
            map.record(
                "v",
                "authed-late".to_string(),
                ProviderRef::new("q"),
                [2u8; 32],
                true,
                1,
                String::new(),
            ),
            "an authenticated ad still finds room by evicting a hint-only id"
        );
        assert!(map.by_id.len() <= MAX_TRACKED_TILESETS);
    }

    /// R1 (OF-8 fetch-context integrity): the verse set a fetched root is
    /// bound to can never be SHRUNK by an attacker — a hint flood on their
    /// own verse cannot evict the last entry of the victim's verse, and
    /// re-advertising the victim's root under a fresh tileset id still binds
    /// it to the victim's verse too. The end-to-end consequence is driven
    /// through the real gate: the attacker's verse viewers stay refused.
    #[test]
    fn an_attacker_cannot_shrink_a_fetched_roots_bound_verse_set() {
        let provider_addr = |seed: u8| {
            let id = iroh::SecretKey::from_bytes(&[seed; 32]).public();
            ProviderRef::new(serde_json::to_string(&iroh::EndpointAddr::new(id)).unwrap())
        };
        let viewers = |did: &str| crate::docs_engine::VerseAccessSnapshot {
            owner_did: None,
            open_to_all: false,
            viewer_dids: [did.to_string()].into_iter().collect(),
        };
        let root = [42u8; 32];

        // (a) A hint flood on the attacker's verse never evicts the victim
        // verse's last (hint) entry.
        let mut flooded = TilesetProviderMap::default();
        assert!(flooded.record(
            "victim-verse",
            "valley".to_string(),
            provider_addr(1),
            root,
            false,
            1,
            String::new(),
        ));
        for i in 0..(MAX_PROVIDERS_PER_TILESET * 3) {
            flooded.record(
                "attacker-verse",
                "valley".to_string(),
                ProviderRef::new(format!("attacker-{i}")),
                root,
                false,
                1,
                String::new(),
            );
        }
        let bound = flooded.verses_for("valley", root);
        assert!(
            bound.contains("victim-verse"),
            "the flood evicted the victim verse: {bound:?}"
        );
        assert!(bound.contains("attacker-verse"));

        // (b) The victim's root re-advertised under a FRESH id on the
        // attacker's verse, naming the victim's (vouched) publisher: the
        // fetch context still carries the victim verse, so the attacker's
        // verse viewers are refused through the real gate.
        let mut map = TilesetProviderMap::default();
        let publisher = provider_addr(1);
        assert!(map.record(
            "victim-verse",
            "valley".to_string(),
            publisher.clone(),
            root,
            true,
            1,
            String::new(),
        ));
        map.record(
            "attacker-verse",
            "fresh-id".to_string(),
            publisher.clone(),
            root,
            false,
            1,
            String::new(),
        );
        let gate = BlobAccessGate::default();
        gate.set_verse_access("victim-verse", viewers("did:key:victim-viewer"));
        gate.set_verse_access("attacker-verse", viewers("did:key:attacker"));
        bind_fetched_root(Some(&gate), &map, "fresh-id", root, &[publisher]);
        assert!(
            !gate.admits_hash("did:key:attacker", &root),
            "the attacker's verse viewers must stay refused"
        );
    }

    /// R1 provenance rule: a relayed hint on the attacker's verse names a
    /// THIRD-PARTY provider (who would serve this node on the strength of
    /// its membership elsewhere) and the attacker self-vouches as a provider
    /// too. The root is fetched for local use, but bound with the
    /// unvouched marker, so it is never re-served — to the attacker's verse
    /// viewers or anyone. A fetch whose providers all vouched is re-served
    /// to the context's viewers as before.
    #[test]
    fn a_root_fetched_through_an_unvouched_provider_is_never_secondary_seeded() {
        let provider_addr = |seed: u8| {
            let id = iroh::SecretKey::from_bytes(&[seed; 32]).public();
            ProviderRef::new(serde_json::to_string(&iroh::EndpointAddr::new(id)).unwrap())
        };
        let open_to = |did: &str| crate::docs_engine::VerseAccessSnapshot {
            owner_did: None,
            open_to_all: false,
            viewer_dids: [did.to_string()].into_iter().collect(),
        };
        let (attacker, third_party) = (provider_addr(7), provider_addr(8));
        let laundered = [43u8; 32];
        let mut map = TilesetProviderMap::default();
        map.record(
            "attacker-verse",
            "bait".to_string(),
            attacker.clone(),
            laundered,
            true,
            1,
            String::new(),
        );
        map.record(
            "attacker-verse",
            "bait".to_string(),
            third_party.clone(),
            laundered,
            false,
            1,
            String::new(),
        );
        let (providers, root, _) =
            resolve_providers_and_root(&map, "bait", None).expect("resolves");
        assert_eq!(root, laundered);
        assert!(providers.contains(&third_party));

        let gate = BlobAccessGate::default();
        gate.set_verse_access("attacker-verse", open_to("did:key:attacker"));
        bind_fetched_root(Some(&gate), &map, "bait", root, &providers);
        assert!(
            !gate.admits_hash("did:key:attacker", &laundered),
            "bytes fetched through an unvouched provider were re-served to the hint's verse"
        );

        // Control: every provider vouched — secondary seeding works.
        let honest = [44u8; 32];
        map.record(
            "home-verse",
            "valley".to_string(),
            provider_addr(9),
            honest,
            true,
            1,
            String::new(),
        );
        gate.set_verse_access("home-verse", open_to("did:key:home-viewer"));
        let (providers, root, _) =
            resolve_providers_and_root(&map, "valley", None).expect("resolves");
        bind_fetched_root(Some(&gate), &map, "valley", root, &providers);
        assert!(gate.admits_hash("did:key:home-viewer", &honest));
    }

    /// Flow test (S3 plan): direct + matching provider -> authenticated;
    /// relayed -> hint; malformed input never authenticates; a non-ad JSON
    /// frame (shaped like a `ComputeEnvelope`) decodes to `None` so it falls
    /// through to `handle_gossip_incoming` unchanged.
    #[test]
    fn tileset_ad_codec_and_authentication_table() {
        let compute_frame = serde_json::to_vec(&serde_json::json!({
            "type": "response",
            "request_id": "r-1",
            "from_did": "did:key:somebody",
            "partial": {
                "per_shard": [],
                "rows": [],
                "failed": false,
            },
        }))
        .unwrap();
        assert!(
            decode_tileset_ad(&compute_frame).is_none(),
            "a `type`-tagged compute envelope must never decode as a tileset ad"
        );
        assert!(decode_tileset_ad(b"not json at all").is_none());

        let alice = iroh::SecretKey::from_bytes(&[101u8; 32]).public();
        let bob = iroh::SecretKey::from_bytes(&[102u8; 32]).public();
        let alice_addr = iroh::EndpointAddr::new(alice);
        let provider_json = serde_json::to_string(&alice_addr).unwrap();

        let ad = TilesetAd {
            kind: TILESET_AD_KIND.to_string(),
            tileset_id: "ts-1".to_string(),
            root_hash: hash_to_hex(&[7u8; 32]),
            total_chunks: 3,
            approx_bytes: 999,
            provider_addr: provider_json.clone(),
            publisher_did: "did:key:zpublisher".to_string(),
        };
        let decoded = decode_tileset_ad(&serde_json::to_vec(&ad).unwrap())
            .expect("a well-formed tileset_ad frame decodes");
        assert_eq!(decoded, ad);

        // PAS-P5 S7a: an older peer's wire frame (no `publisher_did` field at
        // all) still decodes — the field defaults to empty, the same "no TOFU
        // pin hint" case a deliberately-empty one means.
        let legacy_frame = serde_json::json!({
            "kind": TILESET_AD_KIND,
            "tileset_id": "ts-legacy",
            "root_hash": hash_to_hex(&[8u8; 32]),
            "total_chunks": 1,
            "approx_bytes": 1,
            "provider_addr": provider_json,
        });
        let legacy = decode_tileset_ad(&serde_json::to_vec(&legacy_frame).unwrap())
            .expect("a pre-S7a ad frame still decodes");
        assert_eq!(legacy.publisher_did, "");

        for (direct, claimed, from, expected, label) in [
            (
                true,
                provider_json.as_str(),
                alice,
                true,
                "direct + matching provider",
            ),
            (
                true,
                provider_json.as_str(),
                bob,
                false,
                "direct but a different sender — forged",
            ),
            (
                false,
                provider_json.as_str(),
                alice,
                false,
                "relayed — claim-based, never authenticated",
            ),
            (true, "not-json", alice, false, "malformed provider address"),
        ] {
            assert_eq!(
                tileset_ad_is_authenticated(direct, claimed, from),
                expected,
                "{label}"
            );
        }
    }

    /// Flow test (S3 plan): a sync thread with no `TilesetTransfer` (virtual
    /// transport, so no `DocsStack`) answers `DownloadTileset`/`RequestChunk`
    /// honestly instead of hanging or silently dropping the command. The
    /// real two-peer download flow is S4's harness scenario.
    #[test]
    fn offline_thread_answers_tileset_commands_honestly() {
        let _env_guard = SYNC_THREAD_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let p2p_dir = hermetic_p2p_env();
        let store: BlobStoreHandle = Arc::new(MockBlobStore::new());
        let secret = iroh::SecretKey::from_bytes(&[103u8; 32]);
        let (cmd_tx, cmd_rx) = crossbeam::channel::bounded(8);
        let (evt_tx, evt_rx) = crossbeam::channel::bounded(16);
        let factory = Arc::new(FakeTransportFactory {
            replica: Arc::new(MockVerseReplicator::new("did:key:virtual-offline")),
            opened: std::sync::atomic::AtomicUsize::new(0),
        });

        let handle = spawn_sync_thread_with_transport(
            secret,
            store,
            cmd_rx,
            evt_tx,
            "did:key:virtual-offline".to_string(),
            None,
            Some(p2p_dir.path().to_path_buf()),
            Some(factory),
        );
        let started = evt_rx.recv_timeout(std::time::Duration::from_secs(15));
        assert!(matches!(started, Ok(SyncEvent::Started { .. })));
        let _health = evt_rx.recv_timeout(std::time::Duration::from_secs(2));

        cmd_tx
            .send(SyncCommand::DownloadTileset {
                tileset_id: "ts-virtual".to_string(),
                provider: None,
            })
            .unwrap();
        match evt_rx.recv_timeout(std::time::Duration::from_secs(5)) {
            Ok(SyncEvent::TilesetMetaReceived {
                tileset_id,
                meta_json,
                root_hash,
                chunk_hashes,
                ..
            }) => {
                assert_eq!(tileset_id, "ts-virtual");
                assert!(meta_json.contains("offline"), "got: {meta_json}");
                assert!(root_hash.is_empty());
                assert!(chunk_hashes.is_empty());
            }
            other => panic!("expected an honest offline TilesetMetaReceived, got {other:?}"),
        }

        cmd_tx
            .send(SyncCommand::RequestChunk {
                peer_id: String::new(),
                tileset_id: "ts-virtual".to_string(),
                chunk_seq: 0,
                provider: None,
            })
            .unwrap();
        match evt_rx.recv_timeout(std::time::Duration::from_secs(5)) {
            Ok(SyncEvent::ChunkFailed {
                tileset_id, reason, ..
            }) => {
                assert_eq!(tileset_id, "ts-virtual");
                assert!(reason.contains("offline"), "got: {reason}");
            }
            other => panic!("expected an honest offline ChunkFailed, got {other:?}"),
        }

        cmd_tx.send(SyncCommand::Shutdown).unwrap();
        handle.join().expect("sync thread panicked");
        restore_p2p_env();
    }

    // -------------------------------------------------------------------------
    // A2/A4: bootstrap peer parsing + inbound apply seam
    // -------------------------------------------------------------------------

    #[test]
    fn parse_bootstrap_peer_accepts_endpoint_addr_json() {
        // Round-trip through iroh's own serde types — the exact form
        // `SyncEvent::Started.node_addr` emits.
        let addr = iroh::EndpointAddr::new(iroh::SecretKey::from_bytes(&[7u8; 32]).public())
            .with_relay_url("https://relay.example.com".parse().unwrap())
            .with_ip_addr("127.0.0.1:4433".parse().unwrap());
        let json = serde_json::to_string(&addr).unwrap();
        assert_eq!(parse_bootstrap_peer(&json), Some(addr));
    }

    #[test]
    fn parse_bootstrap_peer_accepts_legacy_035_node_addr_json() {
        // Existing FE_SYNC_BOOTSTRAP strings (iroh 0.35 `NodeAddr` JSON)
        // keep parsing — into the equivalent EndpointAddr.
        let id = iroh::SecretKey::from_bytes(&[10u8; 32]).public();
        let legacy = format!(
            r#"{{"node_id":"{id}","relay_url":"https://relay.example.com/","direct_addresses":["127.0.0.1:4433"]}}"#
        );
        let expected = iroh::EndpointAddr::new(id)
            .with_relay_url("https://relay.example.com/".parse().unwrap())
            .with_ip_addr("127.0.0.1:4433".parse().unwrap());
        assert_eq!(parse_bootstrap_peer(&legacy), Some(expected));
    }

    #[test]
    fn parse_bootstrap_peer_rejects_bare_endpoint_id() {
        // A bare EndpointId carries no address — undialable, so skipped loudly.
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
        let addr = iroh::EndpointAddr::new(iroh::SecretKey::from_bytes(&[9u8; 32]).public())
            .with_ip_addr("127.0.0.1:4433".parse().unwrap());
        let json = serde_json::to_string(&addr).unwrap();
        // Semicolon-separated: EndpointAddr JSON contains commas, so the list
        // separator must not be a comma (this test pins that exact contract).
        std::env::set_var(BOOTSTRAP_ENV_VAR, format!("{json};not-a-peer;;{json}"));

        let peers = bootstrap_peers_from_env();
        std::env::remove_var(BOOTSTRAP_ENV_VAR);
        assert_eq!(peers.len(), 2, "both valid entries parse, garbage skipped");
        assert!(peers.contains(&addr));

        // QA-0117: the remembered peers join that bootstrap set without ever
        // pushing it past the 16-provider cap scene assets enforce on open.
        use crate::peer_book::{self, BookEntry, MAX_PEERS};
        let peer = |seed: u8, port: u16| {
            iroh::EndpointAddr::new(iroh::SecretKey::from_bytes(&[seed; 32]).public())
                .with_ip_addr(format!("127.0.0.1:{port}").parse().unwrap())
        };
        let dir = tempfile::TempDir::new().unwrap();
        let now = 2_000_000_000;
        let saturated: Vec<_> = (1..=16u8)
            .map(|seed| BookEntry { addr: peer(seed, 5000 + u16::from(seed)), last_seen: now - u64::from(seed) })
            .collect();
        peer_book::store(dir.path(), "verse-book", &saturated).unwrap();
        let mut bootstrap = vec![peer(40, 6000)];
        peer_book::merge(&mut bootstrap, peer_book::load(dir.path(), "verse-book"));
        assert_eq!(bootstrap.len(), MAX_PEERS, "explicit peer kept, book trimmed to the cap");
        assert_eq!(bootstrap[0], peer(40, 6000));
        assert_eq!(bootstrap[1], peer(1, 5001), "most recently synced remembered peer first");
        // A full book evicts its least recently synced entry not currently synced.
        let mut book = peer_book::load_entries(dir.path(), "verse-book", now);
        let stale_but_synced = peer(16, 5016).id;
        assert!(peer_book::record(&mut book, peer(41, 6001), &[stale_but_synced], now));
        assert_eq!(book.len(), MAX_PEERS);
        assert!(book.iter().any(|entry| entry.addr.id == stale_but_synced), "a live peer is never evicted");
        assert!(!book.iter().any(|entry| entry.addr.id == peer(15, 5015).id), "the oldest idle peer was evicted");
        // Week-old entries drop on load; the legacy bare-address book still loads.
        let week_old = vec![BookEntry { addr: peer(2, 5002), last_seen: now - 8 * 24 * 60 * 60 }];
        peer_book::store(dir.path(), "verse-old", &week_old).unwrap();
        assert!(peer_book::load_entries(dir.path(), "verse-old", now).is_empty());
        let legacy_path = dir.path().join("peer-book").join(format!(
            "{}.json",
            hex::encode(blake3::hash(b"verse-legacy").as_bytes())
        ));
        std::fs::write(&legacy_path, serde_json::to_vec(&vec![peer(3, 5003)]).unwrap()).unwrap();
        assert_eq!(peer_book::load(dir.path(), "verse-legacy"), vec![peer(3, 5003)]);

        // QA-0136: twenty synced collaborators keep a live roster (the recorder and
        // scene fetches used to fail past 16); each fetch asks a 16-provider window
        // after the explicit peer, and rotating windows reach every collaborator.
        let crowd: Vec<_> = (60..80u8).map(|seed| peer(seed, 7000).id).collect();
        let roster = crate::replicator::SceneProviderRoster::live_for_test(crowd.clone());
        assert_eq!(roster.peers().unwrap().len(), 20);
        let explicit = vec![crate::blob_transport::IrohBlobTransport::provider_ref(&peer(40, 6000)).unwrap()];
        let mut asked = std::collections::HashSet::new();
        for rotation in 0..20 {
            let window = crate::scene_assets::provider_window(&explicit, roster.peers().unwrap(), rotation).unwrap();
            assert_eq!(window.len(), 16);
            assert_eq!(window[0], explicit[0], "explicit providers are asked first");
            asked.extend(window.iter().filter_map(|provider| {
                crate::blob_transport::IrohBlobTransport::parse_provider(provider).ok().map(|address| address.id)
            }));
        }
        assert!(crowd.iter().all(|id| asked.contains(id)), "every collaborator is eventually asked");
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

    /// R1 owner integrity across a restart: the replica replays a forged
    /// SELF-signed manifest (an attacker naming itself creator, opening the
    /// verse to everyone) and then THIS node's own invite-pinned manifest
    /// entry. The own-signed entry is locally authoritative: it corrects the
    /// owner and drops the false owner's open access. A stale own entry
    /// replayed after the genuine owner's newer settings does not override
    /// them (it passes the ordinary authority gate).
    #[test]
    fn a_replayed_own_manifest_corrects_a_forged_owner_pin_without_overriding_newer_settings() {
        let (evt_tx, _evt_rx) = crossbeam::channel::unbounded();
        let local = "did:key:joiner";
        let manifest = |author: &str, created_by: &str, extra: serde_json::Value| {
            let mut row = serde_json::json!({ "verse_id": "v-1", "created_by": created_by });
            row.as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            RowChange {
                table: "verse".to_string(),
                record_id: "v-1".to_string(),
                content_hash: [0u8; 32],
                author_id: author.to_string(),
                timestamp: 0,
                is_tombstone: false,
                data: serde_json::to_vec(&row).unwrap(),
            }
        };
        let mut fabric = VerseFabric::default();
        handle_inbound_row_change(
            &mut fabric,
            "v-1",
            &manifest(
                "did:key:attacker",
                "did:key:attacker",
                serde_json::json!({ "default_access": "viewer" }),
            ),
            local,
            &evt_tx,
            &None,
        );
        assert_eq!(fabric.to_dump_json()["owner_did"], "did:key:attacker");
        assert!(fabric.access_snapshot("v-1").open_to_all);

        handle_inbound_row_change(
            &mut fabric,
            "v-1",
            &manifest(
                local,
                "did:key:owner",
                serde_json::json!({ "default_access": "none" }),
            ),
            local,
            &evt_tx,
            &None,
        );
        assert_eq!(fabric.to_dump_json()["owner_did"], "did:key:owner");
        assert!(
            !fabric.access_snapshot("v-1").open_to_all,
            "the false owner's open access survived"
        );

        // The genuine owner's newer settings, then our stale entry again.
        handle_inbound_row_change(
            &mut fabric,
            "v-1",
            &manifest(
                "did:key:owner",
                "did:key:owner",
                serde_json::json!({ "ts_mode": "sharded" }),
            ),
            local,
            &evt_tx,
            &None,
        );
        assert_eq!(
            fabric.settings.mode,
            fe_runtime::timeseries::TimeseriesMode::Sharded
        );
        handle_inbound_row_change(
            &mut fabric,
            "v-1",
            &manifest(
                local,
                "did:key:owner",
                serde_json::json!({ "ts_mode": "mirror" }),
            ),
            local,
            &evt_tx,
            &None,
        );
        assert_eq!(
            fabric.settings.mode,
            fe_runtime::timeseries::TimeseriesMode::Sharded,
            "a stale own replay must not override a newer authorized edit"
        );
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

    /// QA-0138: a full DB channel never blocks the sync thread and never loses a
    /// row: the row is kept (and counted), later rows queue behind it, a newer
    /// version of a kept key supersedes it, and the resend delivers in order.
    #[test]
    fn inbound_row_change_keeps_and_resends_on_full_db_channel() {
        let (evt_tx, _evt_rx) = crossbeam::channel::bounded(8);
        let (db_tx, db_rx) = crossbeam::channel::bounded(1);
        let change = |record_id: &str, name: &str| RowChange {
            table: "node".to_string(),
            record_id: record_id.to_string(),
            content_hash: [4u8; 32],
            author_id: "did:key:peer-c".to_string(),
            timestamp: 7,
            is_tombstone: false,
            data: format!(r#"{{"name":"{name}"}}"#).into_bytes(),
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

        let db = Some(db_tx);
        let mut fabrics = HashMap::from([("verse-3".to_string(), VerseFabric::default())]);
        let before = inbound_apply_drop_count();
        for (record_id, name) in [("node-full", "first"), ("node-other", "other"), ("node-full", "latest")] {
            let verdict = handle_inbound_row_change(
                fabrics.get_mut("verse-3").unwrap(),
                "verse-3",
                &change(record_id, name),
                "did:key:local",
                &evt_tx,
                &db,
            );
            assert!(verdict == InboundVerdict::Kept, "a full channel keeps the row");
        }
        assert!(
            inbound_apply_drop_count() > before,
            "a full DB channel must keep-and-count, never block the sync thread"
        );

        let (publication_tx, _) = tokio::sync::mpsc::channel(1);
        let mut jobs = crate::scene_assets::SceneAssetJobs::new(
            Arc::new(MockBlobStore::new()),
            None,
            Default::default(),
            None,
            crate::event_outbox::EventOutbox::new(crossbeam::channel::bounded(8).0),
            publication_tx,
        );
        assert!(!resend_unsent_rows(&mut fabrics, &db, &mut jobs), "still full: kept for retry");
        db_rx.try_recv().unwrap();
        let mut delivered = Vec::new();
        while !resend_unsent_rows(&mut fabrics, &db, &mut jobs) {
            delivered.push(db_rx.try_recv().unwrap());
        }
        delivered.extend(db_rx.try_iter());
        let delivered: Vec<(String, String)> = delivered
            .into_iter()
            .map(|command| match command {
                DbCommand::ApplyReplicatedRow { record_id, row_bytes, .. } => {
                    (record_id, String::from_utf8(row_bytes).unwrap())
                }
                other => panic!("unexpected command {other:?}"),
            })
            .collect();
        assert_eq!(
            delivered,
            vec![
                ("node-other".to_string(), r#"{"name":"other"}"#.to_string()),
                ("node-full".to_string(), r#"{"name":"latest"}"#.to_string()),
            ],
            "the superseded version is never applied; order of the live entries holds"
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

        let mut leases = crate::join_ticket::JoinedReplicaLeases::default();
        let old = crate::join_ticket::JoinedReplicaLease {request_id:"old-join".into(),verse_id:"verse-1".into(),namespace_id:"0".repeat(64)};
        let mut newer = old.clone();newer.request_id="new-join".into();
        leases.opened(newer.clone());
        assert!(!leases.take(&old) && replicas.contains_key("verse-1"),"Late old-generation cleanup cannot close the newer pipeline");
        let mut wrong_namespace = newer.clone();wrong_namespace.namespace_id="1".repeat(64);
        assert!(!leases.take(&wrong_namespace),"Cleanup also pins the exact native namespace");
        leases.forget("verse-1");
        assert!(!leases.take(&newer),"An ordinary reopen relinquishes the old join lease");
        leases.opened(newer.clone());
        assert!(leases.take(&newer),"Only exact current ownership admits the normal close path");
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
        fabric.note_verse_row(
            "v-1",
            &serde_json::json!({
                "created_by": "did:key:local", "ts_mode": "sharded", "ts_bucket_width_ms": 86_400_000
            }),
            "did:key:local",
        );
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
        fabric.note_verse_row(
            "v-1",
            &serde_json::json!({
                "created_by": "did:key:local", "ts_mode": "sharded", "ts_bucket_width_ms": 86_400_000
            }),
            "did:key:local",
        );
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

        // RP-5: a `__shards` row needs Editor+ at the shard's petal scope
        // (`AGENTS.md` §transport-admission-control item 5, CLOSED). Admit
        // the owner, the petal map, and peer-b's Editor grant first so this
        // test's shard row converges — the point under test is DB-thread
        // non-forwarding, not the gate itself (that has its own dedicated
        // tests in `sharding.rs`).
        let manifest_row = RowChange {
            table: "verse".to_string(),
            record_id: "v-1".to_string(),
            content_hash: [0u8; 32],
            author_id: "did:key:owner".to_string(),
            timestamp: 0,
            is_tombstone: false,
            data: serde_json::to_vec(&serde_json::json!({
                "verse_id": "v-1", "created_by": "did:key:owner",
            }))
            .unwrap(),
        };
        handle_inbound_row_change(
            &mut fabric,
            "v-1",
            &manifest_row,
            "did:key:local",
            &evt_tx,
            &None,
        );
        let petal_row = RowChange {
            table: "petal".to_string(),
            record_id: "p1".to_string(),
            content_hash: [0u8; 32],
            author_id: "did:key:owner".to_string(),
            timestamp: 0,
            is_tombstone: false,
            data: serde_json::to_vec(&serde_json::json!({ "petal_id": "p1", "fractal_id": "f1" }))
                .unwrap(),
        };
        handle_inbound_row_change(
            &mut fabric,
            "v-1",
            &petal_row,
            "did:key:local",
            &evt_tx,
            &None,
        );
        let petal_scope = fe_policy::build_scope("v-1", Some("f1"), Some("p1"));
        let grant_record_id = format!("{petal_scope}/did:key:peer-b/did:key:owner");
        let grant_row = RowChange {
            table: "role".to_string(),
            record_id: grant_record_id,
            content_hash: [0u8; 32],
            author_id: "did:key:owner".to_string(),
            timestamp: 0,
            is_tombstone: false,
            data: serde_json::to_vec(&serde_json::json!({
                "scope": petal_scope, "peer_did": "did:key:peer-b", "grantor_did": "did:key:owner",
                "role": "editor", "revoked": false, "hlc": 1,
            }))
            .unwrap(),
        };
        handle_inbound_row_change(
            &mut fabric,
            "v-1",
            &grant_row,
            "did:key:local",
            &evt_tx,
            &None,
        );
        while evt_rx.try_recv().is_ok() {}

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

    /// RP-6 §2.6: a local reading in a shard the local peer does NOT host
    /// publishes `__holders/{shard}/{local_did}` once (after the ledger,
    /// before the reading), never again for the same shard — and after a
    /// restart the fabric re-learns the declaration from our own snapshot
    /// row, so the next reading still publishes nothing new. Holder rows are
    /// sync-plane only (never forwarded to the DB thread).
    #[tokio::test]
    async fn holder_declaration_is_published_once_per_shard_and_relearned_after_restart() {
        let tmp = tempfile::TempDir::new().unwrap();
        let blob_store: BlobStoreHandle =
            Arc::new(crate::FsBlobStore::new(tmp.path().join("blobs")).expect("fs blob store"));
        let local = "did:key:local";
        let mock = MockVerseReplicator::new(local);
        let mut writes = mock.subscribe().await.expect("observe every write");
        let mut replicas: HashMap<String, Box<dyn VerseReplicator>> = HashMap::new();
        replicas.insert("v-hold".to_string(), Box::new(mock));
        let mut pending = PendingWrites::default();

        // The ledger (converged from peer-b, the shard's only host) and the
        // verse settings the reading's bucket depends on.
        let seed_fabric = |fabric: &mut VerseFabric| {
            fabric.note_verse_row(
                "v-hold",
                &serde_json::json!({
                    "created_by": local, "ts_mode": "sharded", "ts_bucket_width_ms": 86_400_000
                }),
                local,
            );
            fabric.note_shard_row(&serde_json::json!({
                "shard": "p1/a1/20284", "hosts": ["did:key:peer-b"],
                "mode": "sharded", "replication_factor": 1, "bucket_width_ms": 86_400_000,
                "range_start_ms": 0, "range_end_ms": 0, "row_count": 1, "size_bytes": 1
            }));
        };
        let mut fabric = VerseFabric::default();
        seed_fabric(&mut fabric);

        let write_reading = |reading_id: &'static str| {
            let row = format!(
                r#"{{"reading_id":"{reading_id}","node_id":"a1","petal_id":"p1","recorded_at_ms":1752580800000}}"#
            );
            blob_store.add_blob(row.as_bytes()).expect("reading blob")
        };
        let first = write_reading("r-1");
        let second = write_reading("r-2");
        let third = write_reading("r-3");
        for (reading_id, hash) in [("r-1", &first), ("r-2", &second)] {
            handle_write_row_entry(
                &replicas,
                &mut pending,
                &mut fabric,
                &blob_store,
                local,
                "v-hold",
                "iot_reading",
                reading_id,
                hash,
            )
            .await;
        }

        let mut published: Vec<RowChange> = Vec::new();
        while let Ok(change) = writes.try_recv() {
            published.push(change);
        }
        let holder_writes: Vec<&RowChange> = published
            .iter()
            .filter(|c| c.table == HOLDERS_TABLE)
            .collect();
        assert_eq!(
            holder_writes.len(),
            1,
            "declared once per shard, not once per reading"
        );
        assert_eq!(holder_writes[0].record_id, "p1/a1/20284/did:key:local");
        let holder_pos = published
            .iter()
            .position(|c| c.table == HOLDERS_TABLE)
            .unwrap();
        let reading_pos = published
            .iter()
            .position(|c| c.table == "iot_reading")
            .expect("the reading was published");
        assert!(
            holder_pos < reading_pos,
            "the declaration lands before the reading"
        );

        // Restart: a fresh fabric replays our own snapshot rows.
        let (evt_tx, _evt_rx) = crossbeam::channel::bounded(64);
        let (db_tx, db_rx) = crossbeam::channel::bounded::<DbCommand>(64);
        let mut restarted = VerseFabric::default();
        seed_reconciliation(
            replicas.get("v-hold").unwrap().as_ref(),
            "v-hold",
            local,
            &evt_tx,
            &Some(db_tx),
            &mut restarted,
        )
        .await;
        assert!(
            restarted.declared_holdings().contains("p1/a1/20284"),
            "the declaration is re-learned from our own snapshot row"
        );
        assert!(
            db_rx.try_iter().all(|cmd| !matches!(
                cmd,
                DbCommand::ApplyReplicatedRow { ref table, .. } if table == HOLDERS_TABLE
            )),
            "holder rows never reach the DB thread"
        );
        seed_fabric(&mut restarted);
        handle_write_row_entry(
            &replicas,
            &mut pending,
            &mut restarted,
            &blob_store,
            local,
            "v-hold",
            "iot_reading",
            "r-3",
            &third,
        )
        .await;
        let after_restart: Vec<RowChange> = std::iter::from_fn(|| writes.try_recv().ok()).collect();
        assert!(
            after_restart.iter().all(|c| c.table != HOLDERS_TABLE),
            "no re-declaration after a restart"
        );
        assert!(after_restart.iter().any(|c| c.table == "iot_reading"));
    }

    // -------------------------------------------------------------------------
    // RP-5 §2.5: re-read on empowerment (role statements held, then
    // recovered straight from the replica once their grantor is empowered)
    // -------------------------------------------------------------------------

    /// R7: a role statement whose grantor had no authority when it arrived
    /// is never retained by the fabric (it is held only in the replica).
    /// Once the owner empowers that grantor, `drain_reread_queues` replays
    /// the grantor's entries straight from the replica through the inbound
    /// apply path, and the held statement is admitted.
    #[tokio::test]
    async fn a_held_role_statement_is_reread_from_the_replica_when_its_grantor_is_empowered() {
        let (evt_tx, _evt_rx) = crossbeam::channel::bounded(32);
        let mut fabrics: HashMap<String, VerseFabric> = HashMap::new();
        let mut replicas: HashMap<String, Box<dyn VerseReplicator>> = HashMap::new();

        let owner = "did:key:owner";
        let m = "did:key:m";
        let x = "did:key:x";
        let verse_id = "v-r7";
        let verse_scope = fe_policy::build_scope(verse_id, None, None);

        // M's statement lives in the replica (as it would after a real
        // inbound delivery), even though the fabric cannot admit it yet.
        let replicator = MockVerseReplicator::new(m);
        replicator
            .write_row(
                "role",
                &format!("{verse_scope}/{x}/{m}"),
                &serde_json::to_vec(&serde_json::json!({
                    "scope": verse_scope, "peer_did": x, "grantor_did": m,
                    "role": "editor", "revoked": false, "hlc": 1,
                }))
                .unwrap(),
            )
            .await
            .unwrap();
        replicas.insert(verse_id.to_string(), Box::new(replicator));

        let fabric = fabrics.entry(verse_id.to_string()).or_default();
        fabric.note_entry(
            verse_id,
            &RowChange {
                table: "verse".into(),
                record_id: verse_id.into(),
                content_hash: [0; 32],
                author_id: owner.into(),
                timestamp: 0,
                is_tombstone: false,
                data: serde_json::to_vec(&serde_json::json!({ "created_by": owner })).unwrap(),
            },
        );
        // Pinning the owner raises them to Owner everywhere, so they are
        // queued for the §2.5 re-read (statements they made before the
        // manifest landed were discarded — refine R3, `EffectiveRoles::diff`).
        assert_eq!(fabric.drain_reread_queue(), vec![owner.to_string()]);
        // M's statement arrives inbound (the live pump) — M has no
        // authority yet, so it is discarded, never stored.
        fabric.note_entry(
            verse_id,
            &RowChange {
                table: "role".into(),
                record_id: format!("{verse_scope}/{x}/{m}"),
                content_hash: [0; 32],
                author_id: m.into(),
                timestamp: 0,
                is_tombstone: false,
                data: serde_json::to_vec(&serde_json::json!({
                    "scope": verse_scope, "peer_did": x, "grantor_did": m,
                    "role": "editor", "revoked": false, "hlc": 1,
                }))
                .unwrap(),
            },
        );
        assert_eq!(
            fabric.role_at(x, &verse_scope),
            fe_policy::RoleLevel::None,
            "M's statement must not be retained before M has any authority"
        );
        assert!(fabric.drain_reread_queue().is_empty());

        // The owner grants M Manager — M is now empowered and queued for
        // the re-read.
        fabric.note_entry(
            verse_id,
            &RowChange {
                table: "role".into(),
                record_id: format!("{verse_scope}/{m}/{owner}"),
                content_hash: [0; 32],
                author_id: owner.into(),
                timestamp: 0,
                is_tombstone: false,
                data: serde_json::to_vec(&serde_json::json!({
                    "scope": verse_scope, "peer_did": m, "grantor_did": owner,
                    "role": "manager", "revoked": false, "hlc": 2,
                }))
                .unwrap(),
            },
        );

        drain_reread_queues(
            &mut fabrics,
            &replicas,
            "did:key:local",
            &evt_tx,
            &None,
            &None,
            None,
        )
        .await;

        let fabric = fabrics.get(verse_id).expect("fabric still present");
        assert_eq!(
            fabric.role_at(x, &verse_scope),
            fe_policy::RoleLevel::Editor,
            "the re-read must recover M's held statement from the replica once M is empowered"
        );

        // QA-0133: a scene-asset row drives a prefetch only when it was forwarded
        // and its author holds Editor here; a refused author's row never does.
        let blob = [7u8; 32];
        let asset_row = |author: &str, marked: bool| {
            let mut row = serde_json::json!({
                "asset_id": "asset-1", "petal_id": "p-unmapped",
                "content_hash": fe_runtime::blob_store::hash_to_hex(&blob),
            });
            if marked {
                row[fe_runtime::scene_asset::SCENE_ASSET_ROW_MARKER] = serde_json::json!(1);
            }
            RowChange {
                table: "asset".into(),
                record_id: "asset-1".into(),
                content_hash: [2u8; 32],
                author_id: author.into(),
                timestamp: 0,
                is_tombstone: false,
                data: serde_json::to_vec(&row).unwrap(),
            }
        };
        assert_eq!(scene_asset_prefetch_hash(fabric, verse_id, &asset_row(x, true)), Some(blob));
        assert_eq!(scene_asset_prefetch_hash(fabric, verse_id, &asset_row(x, false)), None);
        assert_eq!(
            scene_asset_prefetch_hash(fabric, verse_id, &asset_row("did:key:stranger", true)),
            None,
            "a row the role view refuses never fetches"
        );
        assert_eq!(
            handle_inbound_row_change(
                fabrics.get_mut(verse_id).unwrap(),
                verse_id,
                &asset_row("did:key:local", true),
                "did:key:local",
                &evt_tx,
                &None,
            ),
            InboundVerdict::NotForwarded,
            "an own echo is never forwarded, so never prefetched"
        );

        // QA-0132: M's revoke of X arrives while the DB channel is full and this
        // node's replica of the verse is closed. The revoke row is kept and resent
        // FIRST, the demotion waits for the reopen (no timer spin), a full channel
        // keeps it for the retry, and only an accepted batch acknowledges it.
        let role_row = |revoked: bool, hlc: u64| RowChange {
            table: "role".into(),
            record_id: format!("{verse_scope}/{x}/{m}"),
            content_hash: [0; 32],
            author_id: m.into(),
            timestamp: 0,
            is_tombstone: false,
            data: serde_json::to_vec(&serde_json::json!({
                "scope": verse_scope, "peer_did": x, "grantor_did": m,
                "role": "editor", "revoked": revoked, "hlc": hlc,
            }))
            .unwrap(),
        };
        let x_replica = MockVerseReplicator::new(x);
        x_replica
            .write_row("node", "n1", br#"{"node_id":"n1"}"#)
            .await
            .unwrap();
        x_replica
            .write_row("asset", "asset-1", &asset_row(x, true).data)
            .await
            .unwrap();
        replicas.remove(verse_id);
        let (db_tx, db_rx) = crossbeam::channel::bounded(1);
        let db = Some(db_tx);
        db.as_ref().unwrap().try_send(DbCommand::LoadHierarchy).unwrap();
        handle_inbound_row_change(
            fabrics.get_mut(verse_id).unwrap(),
            verse_id,
            &role_row(true, 3),
            "did:key:local",
            &evt_tx,
            &db,
        );
        assert_eq!(fabrics[verse_id].pending_rejudges(), vec![x.to_string()]);
        assert!(drain_rejudge_queues(&mut fabrics, &replicas, &db).await, "kept revoke: retry");
        db_rx.try_recv().unwrap();
        assert!(!drain_rejudge_queues(&mut fabrics, &replicas, &db).await, "closed: wait for reopen");
        assert!(matches!(
            db_rx.try_recv().unwrap(),
            DbCommand::ApplyReplicatedRow { ref table, .. } if table == "role"
        ));
        assert_eq!(fabrics[verse_id].pending_rejudges(), vec![x.to_string()]);
        replicas.insert(verse_id.to_string(), Box::new(x_replica));
        db.as_ref().unwrap().try_send(DbCommand::LoadHierarchy).unwrap();
        assert!(drain_rejudge_queues(&mut fabrics, &replicas, &db).await, "full channel: retry");
        assert_eq!(fabrics[verse_id].pending_rejudges(), vec![x.to_string()]);
        db_rx.try_recv().unwrap();
        assert!(!drain_rejudge_queues(&mut fabrics, &replicas, &db).await);
        assert!(fabrics[verse_id].pending_rejudges().is_empty());
        assert!(matches!(
            db_rx.try_recv().unwrap(),
            DbCommand::RejudgeReplicatedNodes { ref nodes, restore_only: false, .. }
                if nodes.len() == 1 && nodes[0].0 == "n1" && nodes[0].1[0].0 == x
        ));

        // QA-0131: revoke, write, re-grant — the re-read never replays X's scene
        // rows through the new role; it asks the DB for a restore-only re-judge,
        // which admits only versions X wrote while holding Editor.
        fabrics.get_mut(verse_id).unwrap().note_entry(verse_id, &role_row(false, 5));
        assert!(fabrics[verse_id].pending_rereads().contains(&x.to_string()));
        // QA-0118: X's asset row skipped its prefetch while X was revoked; the
        // re-grant fetches it (no transport here, so the attempt reports failure).
        let (scene_tx, scene_rx) = crossbeam::channel::bounded(8);
        let (publication_tx, _) = tokio::sync::mpsc::channel(1);
        let mut jobs = crate::scene_assets::SceneAssetJobs::new(
            Arc::new(MockBlobStore::new()),
            None,
            Default::default(),
            None,
            crate::event_outbox::EventOutbox::new(scene_tx),
            publication_tx,
        );
        jobs.opened(verse_id, &[]);
        assert!(!drain_reread_queues(&mut fabrics, &replicas, "did:key:local", &evt_tx, &db, &None, Some(&mut jobs)).await);
        assert!(
            matches!(scene_rx.try_recv(), Ok(SyncEvent::SceneAssetFetchFailed { hash, .. }) if hash == blob),
            "an empowered author's unfetched asset row is prefetched on re-read"
        );
        let sent: Vec<DbCommand> = db_rx.try_iter().collect();
        assert!(
            !sent.iter().any(|cmd| matches!(cmd, DbCommand::ApplyReplicatedRow { table, .. } if table == "node")),
            "scene rows are never replayed through the current role"
        );
        assert!(sent.iter().any(|cmd| matches!(cmd, DbCommand::RejudgeReplicatedNodes { restore_only: true, .. })));
        assert!(fabrics[verse_id].pending_rereads().is_empty());
    }

    /// T12: a flood of strangers' `__shards` rows can evict a legitimately
    /// quarantined Editor's row from the fabric's IN-MEMORY quarantine, but
    /// that row is not lost — it is durably in the replica (as any inbound
    /// row would be), and the re-read on empowerment recovers it once the
    /// owner grants Editor, even though the fabric's own in-memory
    /// quarantine can no longer find it.
    #[tokio::test]
    async fn a_quarantine_flood_cannot_lose_a_legit_editors_shards_row() {
        let (evt_tx, _evt_rx) = crossbeam::channel::bounded(32);
        let mut fabrics: HashMap<String, VerseFabric> = HashMap::new();
        let mut replicas: HashMap<String, Box<dyn VerseReplicator>> = HashMap::new();

        let owner = "did:key:owner";
        let editor = "did:key:editor";
        let verse_id = "v-t12";
        let petal_scope = fe_policy::build_scope(verse_id, Some("f1"), Some("p1"));

        // The editor's `__shards` row is durably in the replica (as an
        // inbound row would be), authored by the editor.
        let replicator = MockVerseReplicator::new(editor);
        replicator
            .write_row(
                SHARD_TABLE,
                "p1/a1/1",
                br#"{"shard":"p1/a1/1","hosts":["did:key:editor"],"mode":"sharded","replication_factor":1,"bucket_width_ms":1000,"range_start_ms":0,"range_end_ms":1000,"row_count":1,"size_bytes":1}"#,
            )
            .await
            .unwrap();
        replicas.insert(verse_id.to_string(), Box::new(replicator));

        let fabric = fabrics.entry(verse_id.to_string()).or_default();
        fabric.note_entry(
            verse_id,
            &RowChange {
                table: "verse".into(),
                record_id: verse_id.into(),
                content_hash: [0; 32],
                author_id: owner.into(),
                timestamp: 0,
                is_tombstone: false,
                data: serde_json::to_vec(&serde_json::json!({ "created_by": owner })).unwrap(),
            },
        );
        fabric.note_entry(
            verse_id,
            &RowChange {
                table: "petal".into(),
                record_id: "p1".into(),
                content_hash: [0; 32],
                author_id: owner.into(),
                timestamp: 0,
                is_tombstone: false,
                data: serde_json::to_vec(
                    &serde_json::json!({ "petal_id": "p1", "fractal_id": "f1" }),
                )
                .unwrap(),
            },
        );

        // The legit row arrives inbound (unknown role yet) — quarantined.
        fabric.note_entry(
            verse_id,
            &RowChange {
                table: SHARD_TABLE.into(),
                record_id: "p1/a1/1".into(),
                content_hash: [0; 32],
                author_id: editor.into(),
                timestamp: 0,
                is_tombstone: false,
                data: br#"{"shard":"p1/a1/1","hosts":["did:key:editor"],"mode":"sharded","replication_factor":1,"bucket_width_ms":1000,"range_start_ms":0,"range_end_ms":1000,"row_count":1,"size_bytes":1}"#.to_vec(),
            },
        );
        assert_eq!(fabric.quarantine_len(), 1);

        // Flood past the cap with strangers' rows on other shards — the
        // legit row is evicted (drop-oldest).
        for i in 0..1100 {
            let shard_key = format!("p1/a{i}/1");
            fabric.note_entry(
                verse_id,
                &RowChange {
                    table: SHARD_TABLE.into(),
                    record_id: shard_key.clone(),
                    content_hash: [0; 32],
                    author_id: "did:key:flooder".into(),
                    timestamp: 0,
                    is_tombstone: false,
                    data: serde_json::to_vec(&serde_json::json!({
                        "shard": shard_key, "hosts": ["did:key:flooder"],
                        "row_count": 1, "size_bytes": 1,
                    }))
                    .unwrap(),
                },
            );
        }
        assert!(
            fabric.quarantine_evicted() > 0,
            "the flood must have evicted something"
        );
        assert!(
            !fabric.shards.contains_key("p1/a1/1"),
            "the legit row must not have survived in the in-memory quarantine alone"
        );

        // The owner grants Editor — the re-read recovers the row straight
        // from the replica, which the in-memory eviction never touched.
        fabric.note_entry(
            verse_id,
            &RowChange {
                table: "role".into(),
                record_id: format!("{petal_scope}/{editor}/{owner}"),
                content_hash: [0; 32],
                author_id: owner.into(),
                timestamp: 0,
                is_tombstone: false,
                data: serde_json::to_vec(&serde_json::json!({
                    "scope": petal_scope, "peer_did": editor, "grantor_did": owner,
                    "role": "editor", "revoked": false, "hlc": 1,
                }))
                .unwrap(),
            },
        );

        drain_reread_queues(
            &mut fabrics,
            &replicas,
            "did:key:local",
            &evt_tx,
            &None,
            &None,
            None,
        )
        .await;

        let fabric = fabrics.get(verse_id).expect("fabric still present");
        assert!(
            fabric.shards.contains_key("p1/a1/1"),
            "the re-read must recover the legit row from the replica despite the in-memory eviction"
        );
    }

    /// OF-9 + T12: an Editor's `petal` row is gated now, so it can be held in
    /// the quarantine and EVICTED by a flood before the Editor's grant lands.
    /// It is not lost: `petal/` is in the re-read-on-empowerment set, so the
    /// grant pulls it back from the replica and the petal's scope chain
    /// converges.
    #[tokio::test]
    async fn an_evicted_editors_petal_row_is_reread_from_the_replica_on_empowerment() {
        let (evt_tx, _evt_rx) = crossbeam::channel::bounded(32);
        let mut fabrics: HashMap<String, VerseFabric> = HashMap::new();
        let mut replicas: HashMap<String, Box<dyn VerseReplicator>> = HashMap::new();
        let (owner, editor, verse_id) = ("did:key:owner", "did:key:editor", "v-petal");
        let petal_row =
            serde_json::to_vec(&serde_json::json!({ "petal_id": "p2", "fractal_id": "f1" }))
                .unwrap();

        let replicator = MockVerseReplicator::new(editor);
        replicator
            .write_row("petal", "p2", &petal_row)
            .await
            .unwrap();
        replicas.insert(verse_id.to_string(), Box::new(replicator));

        let fabric = fabrics.entry(verse_id.to_string()).or_default();
        let change = |table: &str, record_id: String, author: &str, data: Vec<u8>| RowChange {
            table: table.into(),
            record_id,
            content_hash: [0; 32],
            author_id: author.into(),
            timestamp: 0,
            is_tombstone: false,
            data,
        };
        fabric.note_entry(
            verse_id,
            &change(
                "verse",
                verse_id.into(),
                owner,
                serde_json::to_vec(&serde_json::json!({ "created_by": owner })).unwrap(),
            ),
        );
        fabric.note_entry(
            verse_id,
            &change("petal", "p2".into(), editor, petal_row.clone()),
        );
        assert_eq!(
            fabric.petal_scope(verse_id, "p2"),
            None,
            "held until the editor is empowered"
        );
        for i in 0..1100 {
            let key = format!("p9/a{i}/1");
            let data =
                serde_json::to_vec(&serde_json::json!({ "shard": key, "hosts": [] })).unwrap();
            fabric.note_entry(
                verse_id,
                &change(SHARD_TABLE, key.clone(), "did:key:flooder", data),
            );
        }
        assert!(fabric.quarantine_evicted() > 0);

        let fractal_scope = fe_policy::build_scope(verse_id, Some("f1"), None);
        let grant = serde_json::to_vec(&serde_json::json!({
            "scope": fractal_scope, "peer_did": editor, "grantor_did": owner,
            "role": "editor", "revoked": false, "hlc": 1,
        }))
        .unwrap();
        fabric.note_entry(
            verse_id,
            &change(
                "role",
                format!("{fractal_scope}/{editor}/{owner}"),
                owner,
                grant,
            ),
        );
        drain_reread_queues(
            &mut fabrics,
            &replicas,
            "did:key:local",
            &evt_tx,
            &None,
            &None,
            None,
        )
        .await;

        assert_eq!(
            fabrics[verse_id].petal_scope(verse_id, "p2"),
            Some(fe_policy::build_scope(verse_id, Some("f1"), Some("p2"))),
            "the re-read must recover the evicted petal row from the replica"
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

        let local = "did:key:local";
        let persisted = FixedSnapshot(vec![
            change(
                "verse",
                "verse-1",
                local,
                br#"{"verse_id":"verse-1","created_by":"did:key:local","default_access":"viewer"}"#,
            ),
            change(
                SHARD_TABLE,
                "p1/a/0",
                local,
                &serde_json::to_vec(&serde_json::json!({
                    "shard":"p1/a/0", "hosts":[local], "mode":"mirror", "replication_factor":1,
                    "bucket_width_ms":60000, "range_start_ms":0, "range_end_ms":60000,
                    "row_count":1, "size_bytes":100,
                }))
                .unwrap(),
            ),
        ]);
        let mut restarted = VerseFabric::default();
        seed_reconciliation(&persisted, "verse-1", local, &evt_tx, &None, &mut restarted).await;
        assert!(!restarted.query_catalog_ready("verse-1", "p1"));
        assert!(
            restarted.shards.is_empty(),
            "missing persisted Petal keeps the shard gated"
        );
        assert_eq!(restarted.quarantine_len(), 1);
        handle_inbound_row_change(
            &mut restarted,
            "verse-1",
            &change(
                "petal",
                "p1",
                local,
                br#"{"petal_id":"p1","fractal_id":"f1"}"#,
            ),
            local,
            &evt_tx,
            &None,
        );
        assert!(restarted.query_catalog_ready("verse-1", "p1"));
        assert_eq!(restarted.quarantine_len(), 0);
        assert_eq!(
            restarted.shards["p1/a/0"].row_count, 1,
            "catalog restoration admits the persisted ledger without re-ingesting or counting readings"
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
        // An empty mock snapshot requests stored catalog repair before the role replay.
        assert!(matches!(db_rx.try_recv(),
            Ok(DbCommand::ReemitLocalVerseCatalog { verse_id }) if verse_id == "verse-offline"));
        match db_rx.try_recv() {
            Ok(DbCommand::ReemitLocalRoleStatements { verse_id }) => {
                assert_eq!(verse_id, "verse-offline")
            }
            other => panic!("expected the role replay after catalog recovery, got {other:?}"),
        }
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
        let addr_a = endpoint.node_addr().await;

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

        // QA-0120: the host leaves the verse (its replica closes) and a revoke
        // made from the verse list follows. The publication lease re-opens the
        // replica with the last open's parameters, publishes, then expires.
        let mut leases = PublicationLeases::default();
        leases.opened("v-race", &"0".repeat(64), &None, &[]);
        handle_close_verse_replica(&mut replicas, &mut inbound_pumps, "v-race").await;
        let revoke = br#"{"role":"editor","revoked":true}"#;
        let revoke_hash = blob_store.add_blob(revoke).expect("revoke blob");
        handle_write_row_entry(
            &replicas,
            &mut pending,
            &mut VerseFabric::default(),
            &blob_store,
            "did:key:local",
            "v-race",
            "role",
            "VERSE#v-race/did:key:editor/did:key:local",
            &revoke_hash,
        )
        .await;
        assert_eq!(pending.len(), 1, "closed replica: the revoke is retained");
        let now = std::time::Instant::now();
        let Some(SyncCommand::OpenVerseReplica {
            verse_id,
            namespace_id,
            namespace_secret,
            bootstrap_peers,
        }) = leases.write("v-race", replicas.contains_key("v-race"), now)
        else {
            panic!("a write for a closed, known verse opens a publication lease");
        };
        assert!(
            leases.write("v-race", false, now).is_none(),
            "one lease per verse; later writes renew it"
        );
        leases.opened(&verse_id, &namespace_id, &namespace_secret, &bootstrap_peers);
        handle_open_verse_replica(
            &mut replicas,
            &mut inbound_pumps,
            inbound_tx.clone(),
            &ReplicaTransport::Iroh(Arc::new(IrohDocsEngineHolder::new())),
            &verse_id,
            &namespace_id,
            namespace_secret,
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
        assert!(pending.is_empty(), "the lease open published the revoke");
        assert!(replicas
            .get("v-race")
            .unwrap()
            .snapshot()
            .await
            .expect("snapshot")
            .iter()
            .any(|c| c.table == "role" && c.data == revoke.to_vec()));
        assert!(leases.expired(now).is_empty(), "the lease outlives its write");
        assert_eq!(leases.expired(now + PUBLICATION_LEASE), vec!["v-race".to_string()]);
        // A host open during a lease takes the replica over: nothing left to expire.
        leases.write("v-race", false, now);
        leases.opened("v-race", &"0".repeat(64), &None, &[]);
        leases.opened("v-race", &"0".repeat(64), &None, &[]);
        assert!(leases.expired(now + PUBLICATION_LEASE * 2).is_empty());
        assert!(leases.write("v-race", true, now).is_none(), "an open replica needs no lease");
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
        let addr_b: iroh::EndpointAddr =
            serde_json::from_str(&addr_json).expect("creator EndpointAddr JSON");

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

        // Sequencing barrier: commands run in order, so the ExportJoinTicket
        // reply below proves the open (and its pending flush) completed.
        let (ticket_tx, ticket_rx) = crossbeam::channel::bounded(1);
        cmd_tx.send(SyncCommand::ExportJoinTicket {
            request: crate::join_ticket::ExportJoinTicket {
                request_id: "read-ticket".into(), verse_id: verse_id.clone(), namespace_id: ns_id.clone(),
                issuer_did: local_did_b.clone(), role: fe_database::join_invite::JoinRole::Viewer,
                deadline: std::time::Instant::now()+std::time::Duration::from_secs(10),
            }, reply: ticket_tx,
        }).unwrap();
        let exported = tokio::task::spawn_blocking(move || ticket_rx.recv_timeout(std::time::Duration::from_secs(10)))
            .await.unwrap().unwrap();
        assert_eq!(exported.request_id, "read-ticket");
        let read_ticket = exported.ticket.expect("actual read ticket from the owning sync service");
        assert!(read_ticket.peers().iter().any(|peer| peer.id == addr_b.id));
        assert!(!format!("{read_ticket:?}").contains(read_ticket.encoded()));
        assert!(crate::join_ticket::NativeJoinTicket::parse(read_ticket.encoded(), &ns_id,
            fe_database::join_invite::JoinRole::Editor, &local_did_b).is_err(), "read ticket cannot impersonate write transport");
        assert!(crate::join_ticket::NativeJoinTicket::parse(read_ticket.encoded(), &"ff".repeat(32),
            fe_database::join_invite::JoinRole::Viewer, &local_did_b).is_err(), "namespace is bound");

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
        let owner_key = fe_identity::NodeKeypair::from_bytes(&[71u8;32]).unwrap();
        let recipient_key = fe_identity::NodeKeypair::from_bytes(&[72u8;32]).unwrap();
        let recipient_did = recipient_key.to_did_key();
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
        let invitation = fe_database::join_invite::JoinInvitation::issue_owner(
            &owner_key, recipient_did.clone(), format!("VERSE#{verse_id}"), fe_database::join_invite::JoinRole::Viewer, 1,
            fe_database::join_invite::JoinManifest {
                verse_id: verse_id.clone(), name: "Open-Race Verse".into(), created_by: local_did_b.clone(),
                created_at: "2026-10-07T00:00:00Z".into(), namespace_id: ns_id.clone(), default_access: "viewer".into(),
                ts_mode: "mirror".into(), ts_replication_factor: 1, ts_bucket_width_ms: 86_400_000,
            }, read_ticket.encoded().into(), now, 3600,
        ).unwrap();
        let verified = fe_database::join_invite::JoinInvitation::parse_owner(&invitation.to_link().unwrap(), &recipient_did, now).unwrap();
        let request = crate::join_ticket::JoinReplicaRequest {
            request_id: "cold-read-import".into(), invitation: verified,
            deadline: std::time::Instant::now()+std::time::Duration::from_secs(10),
        };
        let mut current_replicas: HashMap<String,Box<dyn VerseReplicator>> = HashMap::new();
        let peers = prepare_join_replica(&current_replicas, &ReplicaTransport::Iroh(holder_c.clone()), &request, &recipient_did)
            .await.expect("cold recipient imports its actual read capability without a namespace write secret");
        let peers: Vec<iroh::EndpointAddr> = peers.iter().map(|peer| serde_json::from_str(peer).unwrap()).collect();
        let fresh = IrohDocsReplicator::new(
            verse_id.clone(), ns_id.clone(), String::new(), recipient_did.clone(), holder_c.clone(),
        );
        fresh
            .open_document()
            .await
            .expect("fresh peer imports the capability");
        let _fresh_subscription = fresh.subscribe().await.expect("subscribe before dialing the creator");
        fresh
            .start_sync(peers)
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

        assert!(fresh.write_row("node", "viewer-write-refused", b"{}").await.is_err(),
            "a cold Viewer has READ transport only");
        while !fresh.joined_peer_ready(&ns_id, &local_did_b) {
            assert!(std::time::Instant::now() < deadline, "actual issuer synchronization must complete");
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }

        current_replicas.insert(verse_id.clone(),Box::new(fresh));
        let mut repeat = request.clone();
        repeat.request_id="join-into-already-current-verse".into();
        repeat.deadline=std::time::Instant::now()+std::time::Duration::from_secs(10);
        assert_eq!(prepare_join_replica(&current_replicas,&ReplicaTransport::Iroh(holder_c),&repeat,&recipient_did).await.unwrap_err(),"This Verse replica is already open; leave it before accepting another invitation");
        let current=current_replicas.get(&verse_id).expect("The refusal preserves the same navigation replica");
        assert!(current.joined_peer_ready(&ns_id,&local_did_b),"Cancel/timeout after this definite refusal cannot lose the existing native pipeline");
        assert!(current.snapshot().await.unwrap().iter().any(|row|row.table=="verse" && row.record_id==verse_id && row.data==manifest_bytes),"The real cold-imported document and live pump remain usable without an admission replay");


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

    /// W1 residual (§event-outbox): a command burst whose replies overflow
    /// the production-sized bounded(256) event channel never stalls the
    /// loop — while the main thread drains NOTHING, all 600 commands and
    /// `Shutdown` are still accepted through a bounded(1) command channel
    /// (pre-fix the loop blocked on the 257th event and the sends timed out)
    /// — and every reply then arrives, in order, followed by `Stopped`.
    #[test]
    fn a_command_burst_past_the_event_capacity_never_stalls_the_loop() {
        let _env_guard = SYNC_THREAD_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let _p2p_dir = hermetic_p2p_env();
        const BURST: u32 = 600;
        let (cmd_tx, cmd_rx) = crossbeam::channel::bounded(1);
        let (evt_tx, evt_rx) = crossbeam::channel::bounded(256);
        let factory = Arc::new(FakeTransportFactory {
            replica: Arc::new(MockVerseReplicator::new("did:key:burst")),
            opened: std::sync::atomic::AtomicUsize::new(0),
        });
        let handle = spawn_sync_thread_with_transport(
            iroh::SecretKey::from_bytes(&[91u8; 32]),
            Arc::new(MockBlobStore::new()),
            cmd_rx,
            evt_tx,
            "did:key:burst".to_string(),
            None,
            None,
            Some(factory),
        );

        let send = |cmd: SyncCommand| {
            cmd_tx
                .send_timeout(cmd, std::time::Duration::from_secs(10))
                .expect("the sync loop kept accepting commands with the event channel full")
        };
        for chunk_seq in 0..BURST {
            send(SyncCommand::RequestChunk {
                peer_id: String::new(),
                tileset_id: "burst".to_string(),
                chunk_seq,
                provider: None,
            });
        }
        send(SyncCommand::Shutdown);
        assert!(evt_rx.is_full(), "the main thread has drained nothing yet");

        let mut failed_seqs = Vec::new();
        let mut stopped = false;
        while let Ok(event) = evt_rx.recv_timeout(std::time::Duration::from_secs(10)) {
            match event {
                SyncEvent::ChunkFailed { chunk_seq, .. } => failed_seqs.push(chunk_seq),
                SyncEvent::Stopped => {
                    stopped = true;
                    break;
                }
                _ => {}
            }
        }
        handle.join().expect("sync thread panicked");
        restore_p2p_env();
        assert_eq!(
            failed_seqs,
            (0..BURST).collect::<Vec<_>>(),
            "lossless, in order"
        );
        assert!(stopped, "Stopped arrives after every reply");
    }
}
