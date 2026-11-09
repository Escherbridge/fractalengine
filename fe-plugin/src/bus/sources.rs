//! Services and feeds the binary injects into the broker (the HostEnv idiom); see bus/AGENTS.md §sources.

use std::sync::Arc;

use crossbeam::channel::Sender;
use fe_policy::RoleLevel;
use serde::Serialize;
use serde_json::{Map, Value};

use super::{CallerKind, HostError};

/// One `host/query` registration: the SQL, its vars and the identity and scope it is judged under.
#[derive(Debug, Clone, PartialEq)]
pub struct QueryRequest {
    pub identity_did: String,
    pub verse_id: String,
    pub petal_id: String,
    pub sql: String,
    pub vars: Map<String, Value>,
}

/// What fe-api's guard made of a query: the executable scoped SQL and the FROM tables it reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedQuery {
    /// `fe_query::PreparedSql::tables` (any case; the broker compares lowercase).
    pub tables: Vec<String>,
    /// `fe_query::PreparedSql::sql`, re-run as `GuardedQuery { sql }`.
    pub guarded_sql: String,
}

/// One run's rows with REST's caps (10 000 rows, 8 MiB) already applied.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct QueryRows {
    pub rows: Vec<Value>,
    pub truncated: bool,
}

/// fe-api's guarded query core (`guard_and_prepare_query` + `run_guarded_query_via_state`), injected by the binary.
pub trait QuerySource: Send + Sync {
    /// Guard and scope `request`; refuse with `invalid-input` exactly where `POST /api/v1/query` would.
    fn prepare(&self, request: &QueryRequest) -> Result<PreparedQuery, HostError>;
    /// Run off the worker and answer on `reply` exactly once.
    fn run(&self, request: &QueryRequest, prepared: &PreparedQuery, reply: Sender<Result<QueryRows, HostError>>);
    /// Spend one unit of the identity's query rate (`QUERY_RATE_PER_SEC`, shared with REST).
    fn try_acquire_rate(&self, identity_did: &str) -> bool;
}

/// Reads a petal's live nodes for a `host/node-changes` snapshot.
pub trait SceneSnapshotSource: Send + Sync {
    /// Every live node of `petal_id` as fe-runtime `NodeDto` JSON plus a `properties` object; off the worker, one reply.
    fn snapshot(&self, petal_id: &str, reply: Sender<Result<Vec<Value>, HostError>>);
}

/// The role view; in-memory, never blocking and never calling back into the bus (it runs under the broker lock).
pub trait RoleSource: Send + Sync {
    /// `did`'s current role at `petal_id` (`RoleLevel::None` when it has none).
    fn role_at(&self, did: &str, petal_id: &str) -> RoleLevel;
}

/// The verse policy row's bus switch (governance.md §policy-row).
pub trait BusPolicy: Send + Sync {
    /// Whether remote DIDs may reach `external: true` streams and commands in this verse.
    fn external_access_allowed(&self, verse_id: &str) -> bool;
}

/// Governance's audit writer (R21); must not block the caller.
pub trait AuditSink: Send + Sync {
    /// Record one entry.
    fn record(&self, entry: AuditEntry);
}

/// One portal `set_property` (portal ipc-v1.md §6.1, A-13): exactly one op, judged as `identity_did` at write time.
#[derive(Debug, Clone, PartialEq)]
pub struct PortalWrite {
    /// The broker's id for this write (audit correlation).
    pub write: u64,
    /// The installing identity (the host's local DID); the DB re-checks its role at use.
    pub identity_did: String,
    /// Host-stamped provenance `{package_id, content_hash}` from the mount binding, never from page data.
    pub package_id: String,
    pub content_hash: String,
    /// The web-bundle installation (A-14), when governance has one.
    pub installation_id: Option<String>,
    pub verse_id: String,
    /// The repository node's petal: the admission is bound to exactly this petal.
    pub petal_id: String,
    /// Exactly one `PendingOp::SetProperty` on a key the bus already matched against the declared patterns.
    pub op: fe_sdk::plugin_host::PendingOp,
}

/// The extension write path for portal writes, injected by the binary (bus/AGENTS.md §portal-writes).
pub trait PortalWriteSink: Send + Sync {
    /// Commit `write.op` as ONE `DbCommand::PluginCommit` (no readings, no tick) under a host-minted admission
    /// carrying `with_package({package_id, content_hash})`; never block the caller, answer `reply` exactly once.
    fn commit(&self, write: &PortalWrite, reply: Sender<fe_runtime::plugin_host::PluginResult>);
}

/// What an audit entry records.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum AuditAction {
    /// An instance's declared bus surface was admitted (consent covered it).
    Grant,
    Subscribe,
    Unsubscribe,
    CommandCall,
    /// A capability or policy refusal.
    Denial,
    /// A portal `set_property` that reached the role check or the DB, with its outcome (ipc-v1.md §9).
    PropertyWrite,
}

/// One audit row (bus.md §7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AuditEntry {
    pub action: AuditAction,
    pub actor_did: String,
    pub caller_kind: Option<CallerKind>,
    /// The installation acted on, when one exists (never revealed to remote callers).
    pub installation_id: Option<String>,
    /// Source, command or endpoint address.
    pub target: String,
    /// blake3 of the canonical args, for command calls.
    pub args_digest: Option<String>,
    /// `ok`, a refusal code or an end reason.
    pub outcome: String,
    pub at_ms: u64,
}

/// The injected services; [`BusSources::fail_closed`] grants nothing and only logs audits.
#[derive(Clone)]
pub struct BusSources {
    pub query: Option<Arc<dyn QuerySource>>,
    pub scene: Option<Arc<dyn SceneSnapshotSource>>,
    pub roles: Arc<dyn RoleSource>,
    pub policy: Arc<dyn BusPolicy>,
    pub audit: Arc<dyn AuditSink>,
}

impl BusSources {
    /// No query or scene source, every DID role-less, no external access, audits to tracing.
    pub fn fail_closed() -> Self {
        Self {
            query: None,
            scene: None,
            roles: Arc::new(NoRoles),
            policy: Arc::new(NoExternalAccess),
            audit: Arc::new(TracingAudit),
        }
    }
}

struct NoRoles;

impl RoleSource for NoRoles {
    fn role_at(&self, _did: &str, _petal_id: &str) -> RoleLevel {
        RoleLevel::None
    }
}

struct NoExternalAccess;

impl BusPolicy for NoExternalAccess {
    fn external_access_allowed(&self, _verse_id: &str) -> bool {
        false
    }
}

/// Fallback sink until governance (E10) injects the op-log writer.
pub struct TracingAudit;

impl AuditSink for TracingAudit {
    fn record(&self, entry: AuditEntry) {
        tracing::info!(target: "fe_plugin::audit", action = ?entry.action, actor = %entry.actor_did, target_address = %entry.target, outcome = %entry.outcome, "extension audit");
    }
}

/// Whether a reading was accepted locally or applied from a peer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ReadingOrigin {
    Local,
    Replicated,
}

/// One accepted reading as `host/readings` carries it (bus.md §2.2).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ReadingRow {
    pub reading_id: String,
    pub node_id: String,
    pub metric: String,
    pub value: f64,
    pub units: String,
    pub recorded_at: String,
    pub recorded_at_ms: i64,
    pub source_did: String,
    pub origin: ReadingOrigin,
}

/// A reading accepted by the readings door in `petal_id`.
#[derive(Debug, Clone, PartialEq)]
pub struct ReadingEvent {
    pub petal_id: String,
    pub row: ReadingRow,
}

/// Admission event kind (bus.md §2.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum AdmissionKind {
    Extension,
    WebBundle,
}

/// Admission state (governance.md §admission-record).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum AdmissionState {
    Admitted,
    Disabled,
    PolicyDenied,
    Revoked,
    Removed,
}

/// The closed reason set; each reason belongs to exactly one state (bus.md §2.4 table).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum AdmissionReason {
    Consented,
    Restored,
    LocalDisable,
    DisableAll,
    ConsentRevoked,
    PolicyRemoved,
    Ceiling,
    RoleLost,
    RevocationList,
    Uninstalled,
}

impl AdmissionReason {
    /// The state this reason reports, so an event can never pair a reason with the wrong state.
    pub fn state(self) -> AdmissionState {
        match self {
            Self::Consented | Self::Restored => AdmissionState::Admitted,
            Self::LocalDisable | Self::DisableAll | Self::ConsentRevoked => AdmissionState::Disabled,
            Self::PolicyRemoved | Self::Ceiling | Self::RoleLost => AdmissionState::PolicyDenied,
            Self::RevocationList => AdmissionState::Revoked,
            Self::Uninstalled => AdmissionState::Removed,
        }
    }
}

/// One admission change on this host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdmissionEvent {
    pub package_id: String,
    pub content_hash: String,
    pub kind: AdmissionKind,
    pub reason: AdmissionReason,
    pub verse_id: Option<String>,
}

impl AdmissionEvent {
    /// The delta data (bus.md §2.4); `state` is derived from `reason`.
    pub fn to_json(&self) -> Value {
        let mut data = serde_json::json!({
            "package_id": self.package_id,
            "content_hash": self.content_hash,
            "kind": self.kind,
            "state": self.reason.state(),
            "reason": self.reason,
        });
        if let Some(verse_id) = &self.verse_id {
            data["verse_id"] = Value::String(verse_id.clone());
        }
        data
    }
}
