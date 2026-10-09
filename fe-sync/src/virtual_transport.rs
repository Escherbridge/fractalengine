//! The sim-lab transport seam (F8/A19): a **virtual** replica transport
//! implements the same [`VerseReplicator`] contract as the real iroh-docs
//! path, so prod and sim cannot drift by construction (decision D3).
//!
//! The sync thread's open sequence drives whatever it is handed through the
//! SAME open/inbound/apply lifecycle: `open_document` → `start_sync` →
//! `subscribe` (per-replica inbound pump) → `snapshot` (startup
//! reconciliation) → `write_row` (outbound) → `close`. Only the transport
//! under the trait object differs. See `fe-sync/src/AGENTS.md`
//! §virtual-transport and fe-sim `net.rs`/`transport.rs` for the
//! deterministic in-process network that implements this trait.

use crate::replicator::{ReplicatorFuture, VerseReplicator};

/// A sim-lab virtual replica: the per-verse transport a
/// [`VirtualTransportFactory`] builds at every
/// [`crate::SyncCommand::OpenVerseReplica`].
///
/// The method set deliberately mirrors the `IrohDocsReplicator` open-phase
/// surface (the names are iroh vocabulary, but the contract is transport-
/// neutral: "open the replica's backing store, join its swarm, report
/// liveness"). Implementations must also implement [`VerseReplicator`] —
/// the write/subscribe/snapshot/close contract the sync thread's command
/// loop and inbound pump ride.
pub trait VirtualReplica: VerseReplicator {
    /// Open the replica's backing document/namespace (import the capability
    /// or join). Offline/unavailable stacks return `Ok(())` with no backing
    /// doc (the sanctioned mock fallback); a failure while the transport
    /// reports available is recorded by the caller as a loud non-replicating
    /// replica state.
    fn open_document(&self) -> ReplicatorFuture<'_, anyhow::Result<()>>;

    /// Whether the replica is backed by a live transport (doc-backed).
    fn is_doc_backed(&self) -> bool;

    /// Join the live sync swarm for this replica, dialing `peers`. No-op for
    /// virtual transports whose membership is scripted in the hub rather
    /// than dialed.
    fn start_sync(&self, peers: Vec<iroh::NodeAddr>) -> ReplicatorFuture<'_, anyhow::Result<()>>;

    /// Record a failed open on an AVAILABLE transport (F20 finding 4
    /// parity: the replica stays registered but loudly non-replicating).
    fn mark_open_failed(&self, reason: String);

    /// The recorded open failure, if any.
    fn open_error(&self) -> Option<String>;
}

/// Builds the per-verse [`VirtualReplica`] for a sim sync thread.
///
/// Installed through
/// [`crate::spawn_sync_thread_with_transport`]; a sync thread with a factory
/// binds **no iroh endpoint** (no real network at all — A19) and sources
/// every verse replica from the factory instead of the iroh-docs stack.
/// One factory per simulated peer, carrying that peer's identity.
pub trait VirtualTransportFactory: Send + Sync {
    /// Build the replica for one verse open.
    fn open_replica(
        &self,
        verse_id: &str,
        namespace_id: &str,
        namespace_secret: Option<String>,
        local_did: &str,
    ) -> Box<dyn VirtualReplica>;

    /// Whether the virtual transport is available (the analogue of
    /// `IrohDocsEngineHolder::is_available`). An in-process hub is always
    /// available; an open failure while available leaves the replica loudly
    /// non-replicating, never a mock install (F20 finding 4 parity).
    fn is_available(&self) -> bool;

    /// Banner label for the open log line (e.g. `"virtual (sim)"`).
    fn describe(&self) -> &'static str;

    /// Join a verse compute topic on the virtual gossip plane (F9/A21):
    /// the distributed-query fan-out (`SubmitComputeTask`) rides per-verse
    /// gossip topics on the real path, so the sim hub virtualizes the same
    /// seam — one topic handle per subscription, carrying the scripted
    /// latency/partition/churn semantics of the hub it belongs to.
    ///
    /// `local_did` / `local_node` are the sync thread's identity (the same
    /// ed25519 key backs both — the F20 endpoint-identity == fe-DID
    /// alignment), so a delivered message's `from` authenticates its author
    /// exactly like a direct iroh-gossip delivery.
    ///
    /// Default: `None` — the verse honestly has no compute topic (the same
    /// "no gossip sender" answer a virtual transport gave before F9). A
    /// factory without a gossip plane compiles and behaves unchanged.
    fn join_gossip_topic(
        &self,
        _topic_key: &str,
        _local_did: &str,
        _local_node: iroh::NodeId,
    ) -> Option<std::sync::Arc<dyn VirtualGossipTopic>> {
        None
    }
}

/// One inbound message on a virtual gossip topic — the hub's analogue of
/// iroh-gossip's `GossipEvent::Received`. `direct` mirrors
/// `DeliveryScope::is_direct()`: hub deliveries are 0-hop from their
/// publisher, so they are always direct and `from` always authenticates the
/// envelope author (the F23 forged-attribution gate applies verbatim).
#[derive(Debug, Clone)]
pub struct VirtualGossipMessage {
    /// The publisher's node identity (its DID's ed25519 key).
    pub from: iroh::NodeId,
    /// Always `true` from the hub (0-hop delivery) — see above.
    pub direct: bool,
    /// The raw frame (`ComputeEnvelope` bytes for the compute plane).
    pub content: bytes::Bytes,
}

/// A virtual gossip topic subscription (F9/A21): the send/receive halves
/// the sync thread's compute plane drives, mirroring the split
/// `GossipTopic` shape the real path uses.
///
/// Delivery semantics are the hub's: a broadcast schedules one delivery per
/// current subscriber OTHER than the publisher (iroh-gossip 0.35 parity — a
/// sender never receives its own broadcast; the F23 SELF_ECHO admission
/// gate stays as defense for the real path's topology edge cases) under
/// the hub's scripted latency/partition/churn, and a subscriber that was offline or
/// partitioned away MISSES the message (gossip has no history — the
/// degradation scenario depends on this).
pub trait VirtualGossipTopic: Send + Sync {
    /// Broadcast one frame to the topic. Synchronous schedule into the
    /// hub's delivery heap; failures are returned as an honest reason
    /// (e.g. topic left).
    fn broadcast(&self, content: bytes::Bytes) -> Result<(), String>;

    /// Take the inbound stream for this subscription (once — the sync
    /// thread takes it at subscribe time and pumps it). `None` after the
    /// first take.
    fn take_inbound(&self) -> Option<tokio::sync::mpsc::Receiver<VirtualGossipMessage>>;
}
