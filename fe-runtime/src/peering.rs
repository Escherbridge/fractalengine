//! API <-> Sync bridge envelopes for P2P identity, addressed invite ticketing,
//! replica join lifecycle, and live sync/clock diagnostics.
//!
//! `fe-api` depends on `fe-runtime` (never on `fe-sync`), so these bridge
//! types live here and are served by `fe-sync` (`SyncCommand::Peering`).

use serde::Serialize;
use tokio::sync::oneshot;

/// Crossbeam sender handed to `fe-api` for sync-thread peering & state calls.
pub type PeeringCallSender = crossbeam::channel::Sender<PeeringCall>;
/// Crossbeam receiver drained by `fe-sync`'s peering bridge.
pub type PeeringCallReceiver = crossbeam::channel::Receiver<PeeringCall>;

/// Identity and iroh transport snapshot returned by `GET /api/v1/identity`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NodePeeringIdentity {
    pub did: String,
    pub endpoint_id: Option<String>,
    pub direct_addrs: Vec<String>,
    pub home_relay: Option<String>,
    pub relay_mode: String,
    pub relay_health: String,
}

/// Live per-verse replica summary inside [`SyncRuntimeSnapshot`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OpenReplicaInfo {
    pub verse_id: String,
    pub namespace_id: String,
    pub peer_count: usize,
    pub peer_ids: Vec<String>,
    pub quarantine_len: usize,
    pub quarantine_evicted: u64,
    pub byte_gate_in_flight: u64,
    pub served_by_relay: bool,
}

/// Sync-thread runtime telemetry for `GET /api/v1/debug/sync-state`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SyncRuntimeSnapshot {
    pub local_did: String,
    pub endpoint_id: Option<String>,
    pub relay_mode: String,
    pub relay_health: String,
    pub home_relay: Option<String>,
    pub open_replicas: Vec<OpenReplicaInfo>,
    pub deferred_admission_drops: u64,
    pub db_cmd_bytes_in_flight: u64,
}

/// Request sent from `fe-api` to `fe-sync` across [`PeeringCallSender`].
pub enum PeeringCall {
    GetIdentity {
        detail: bool,
        reply: oneshot::Sender<NodePeeringIdentity>,
    },
    ExportJoinTicket {
        request_id: String,
        verse_id: String,
        namespace_id: String,
        issuer_did: String,
        role: String,
        deadline_secs: u64,
        reply: oneshot::Sender<Result<String, String>>,
    },
    JoinVerseReplica {
        request_id: String,
        credential: String,
        deadline_secs: u64,
        reply: oneshot::Sender<Result<(), String>>,
    },
    InspectJoinedReplica {
        request_id: String,
        credential: String,
        deadline_secs: u64,
        reply: oneshot::Sender<Result<bool, String>>,
    },
    GetSyncState {
        verse_id: Option<String>,
        reply: oneshot::Sender<SyncRuntimeSnapshot>,
    },
}
