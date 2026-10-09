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
}
