//! Integration bus broker (D-X6, spec R13-R15, docs/extensions/bus.md); rationale in bus/AGENTS.md.

mod broker;
mod query;
pub mod rate;
pub mod schema;
pub mod sources;

#[cfg(test)]
mod tests;

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub use broker::{
    Bus, BusCounters, BusHooks, ClientGrants, ClientHandle, CommandTicket, GuestWork, InstanceSpec,
};
pub use sources::{
    AdmissionEvent, AdmissionKind, AdmissionReason, AdmissionState, AuditAction, AuditEntry,
    AuditSink, BusPolicy, BusSources, PreparedQuery, QueryRequest, QueryRows, QuerySource,
    ReadingEvent, ReadingOrigin, ReadingRow, RoleSource, SceneSnapshotSource,
};

/// Subscriptions one instance may hold.
pub const MAX_SUBSCRIPTIONS_PER_INSTANCE: usize = 16;
/// Subscriptions one external connection may hold.
pub const MAX_SUBSCRIPTIONS_PER_CLIENT: usize = 16;
/// Host ceiling for an exposed stream's `max_event_bytes`.
pub const MAX_STREAM_EVENT_BYTES: usize = 64 * 1024;
/// Host ceiling for an exposed stream's `max_rate_hz`.
pub const MAX_STREAM_RATE_HZ: f64 = 50.0;
/// Largest command `args`.
pub const MAX_COMMAND_ARGS_BYTES: usize = 16 * 1024;
/// Largest command result.
pub const MAX_COMMAND_RESULT_BYTES: usize = 64 * 1024;
/// Host ceiling for a command's `rate_hz`.
pub const MAX_COMMAND_RATE_HZ: f64 = 10.0;
/// Command calls per second per caller key (remote DID, `ui`, local API, each package).
pub const CALLER_COMMAND_RATE_HZ: f64 = 10.0;
/// Requests per second per remote DID on every extension route (bus.md §4 check 0).
pub const REMOTE_REQUEST_RATE_HZ: f64 = 10.0;
/// A command reply later than this answers `unknown-outcome`.
pub const COMMAND_REPLY_TIMEOUT_MS: u64 = 5_000;
/// Commands queued per instance before callers get `rate-limited`.
pub const COMMAND_QUEUE_PER_INSTANCE: usize = 16;
/// Floor and default of a `host/query` subscription's `min_interval_ms`.
pub const QUERY_MIN_INTERVAL_FLOOR_MS: u64 = 1_000;
/// Events per `on-events` call.
pub const GUEST_BATCH_EVENTS: usize = 64;
/// Bytes per `on-events` call (a larger query snapshot arrives alone).
pub const GUEST_BATCH_BYTES: usize = 256 * 1024;
/// Largest `host/query` snapshot a guest receives.
pub const GUEST_QUERY_EVENT_BYTES: usize = 1024 * 1024;
/// Subscriber queue for a guest.
pub const GUEST_QUEUE: usize = 256;
/// Subscriber queue for an external client.
pub const EXTERNAL_QUEUE: usize = 1_024;
/// Subscriber queue for `host/admission`.
pub const ADMISSION_QUEUE: usize = 64;
/// Nodes per `host/node-changes` snapshot part.
pub const NODE_SNAPSHOT_PART: usize = 500;
/// Changes per `host/node-changes` delta.
pub const NODE_DELTA_CHANGES: usize = 256;
/// Readings per `host/readings` delta.
pub const READINGS_PER_EVENT: usize = 1_000;
/// Anchors a `host/readings` filter may name.
pub const MAX_ANCHOR_FILTER: usize = 256;

/// Why a host call was refused or did not complete (WIT `types.host-error`).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HostError {
    #[error("capability-denied: {0}")]
    CapabilityDenied(String),
    #[error("policy-denied: {0}")]
    PolicyDenied(String),
    #[error("invalid-input: {0}")]
    InvalidInput(String),
    #[error("budget-exceeded: {0}")]
    BudgetExceeded(String),
    #[error("rate-limited: {0}")]
    RateLimited(String),
    #[error("revoked: {0}")]
    Revoked(String),
    #[error("not-available: {0}")]
    NotAvailable(String),
    #[error("backend: {0}")]
    Backend(String),
    #[error("unknown-outcome: {0}")]
    UnknownOutcome(String),
}

impl HostError {
    /// The WIT case name, also the `commands.refused.<reason>` counter suffix.
    pub fn code(&self) -> &'static str {
        match self {
            Self::CapabilityDenied(_) => "capability-denied",
            Self::PolicyDenied(_) => "policy-denied",
            Self::InvalidInput(_) => "invalid-input",
            Self::BudgetExceeded(_) => "budget-exceeded",
            Self::RateLimited(_) => "rate-limited",
            Self::Revoked(_) => "revoked",
            Self::NotAvailable(_) => "not-available",
            Self::Backend(_) => "backend",
            Self::UnknownOutcome(_) => "unknown-outcome",
        }
    }

    /// Capability and policy refusals are audited; budget, rate and transport outcomes are counted only.
    pub fn is_denial(&self) -> bool {
        matches!(self, Self::CapabilityDenied(_) | Self::PolicyDenied(_))
    }
}

/// A refusal an external caller sees (REST, MCP, WS, portal IPC); remote callers only ever get `NotFound`
/// for anything that would reveal what is installed (fe-api §existence-oracle).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BusRefusal {
    /// The uniform not-found.
    NotFound,
    /// Local caller: no active instance or producer.
    NotAvailable(String),
    /// Local caller: not declared, below the floor, or not granted.
    CapabilityDenied(String),
    /// Rate or queue budget spent.
    RateLimited(String),
    /// Arguments failed validation.
    InvalidInput(String),
    /// A per-connection limit (subscriptions) is reached.
    LimitReached(String),
}

impl BusRefusal {
    /// Stable reason code.
    pub fn code(&self) -> &'static str {
        match self {
            Self::NotFound => "not-found",
            Self::NotAvailable(_) => "not-available",
            Self::CapabilityDenied(_) => "capability-denied",
            Self::RateLimited(_) => "rate-limited",
            Self::InvalidInput(_) => "invalid-input",
            Self::LimitReached(_) => "limit-reached",
        }
    }

    /// The HTTP status REST answers (bus.md §4).
    pub fn http_status(&self) -> u16 {
        match self {
            Self::NotFound | Self::NotAvailable(_) => 404,
            Self::CapabilityDenied(_) => 403,
            Self::RateLimited(_) | Self::LimitReached(_) => 429,
            Self::InvalidInput(_) => 400,
        }
    }

    /// The same refusal as a guest sees it (guests are always local).
    pub fn into_host_error(self) -> HostError {
        match self {
            Self::NotFound => HostError::NotAvailable("not found".into()),
            Self::NotAvailable(detail) => HostError::NotAvailable(detail),
            Self::CapabilityDenied(detail) => HostError::CapabilityDenied(detail),
            Self::RateLimited(detail) => HostError::RateLimited(detail),
            Self::InvalidInput(detail) => HostError::InvalidInput(detail),
            Self::LimitReached(detail) => HostError::BudgetExceeded(detail),
        }
    }
}

/// One instance: the package bound to a petal on this host (OD-E3).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct InstanceKey {
    pub petal_id: String,
    pub package_id: String,
}

impl InstanceKey {
    /// Key for `package_id` bound at `petal_id`.
    pub fn new(petal_id: impl Into<String>, package_id: impl Into<String>) -> Self {
        Self { petal_id: petal_id.into(), package_id: package_id.into() }
    }
}

/// A bus source address (bus.md §1).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Source {
    /// `host/query`.
    Query,
    /// `host/readings`.
    Readings,
    /// `host/node-changes`.
    NodeChanges,
    /// `host/admission`.
    Admission,
    /// `<package_id>/<stream_id>`.
    Stream { package_id: String, stream_id: String },
}

impl Source {
    /// Parse an address; `None` for anything outside the grammar.
    pub fn parse(address: &str) -> Option<Self> {
        let (head, tail) = address.split_once('/')?;
        if head == "host" {
            return match tail {
                "query" => Some(Self::Query),
                "readings" => Some(Self::Readings),
                "node-changes" => Some(Self::NodeChanges),
                "admission" => Some(Self::Admission),
                _ => None,
            };
        }
        (is_package_id(head) && is_local_id(tail))
            .then(|| Self::Stream { package_id: head.to_string(), stream_id: tail.to_string() })
    }

    /// The canonical address text.
    pub fn address(&self) -> String {
        match self {
            Self::Query => "host/query".into(),
            Self::Readings => "host/readings".into(),
            Self::NodeChanges => "host/node-changes".into(),
            Self::Admission => "host/admission".into(),
            Self::Stream { package_id, stream_id } => format!("{package_id}/{stream_id}"),
        }
    }
}

/// A manifest `role_floor` as the policy engine's ordered role.
pub fn role_level(floor: fe_sdk::manifest_v2::RoleFloor) -> fe_policy::RoleLevel {
    use fe_policy::RoleLevel;
    use fe_sdk::manifest_v2::RoleFloor;
    match floor {
        RoleFloor::Viewer => RoleLevel::Viewer,
        RoleFloor::Editor => RoleLevel::Editor,
        RoleFloor::Manager => RoleLevel::Manager,
        RoleFloor::Owner => RoleLevel::Owner,
    }
}

/// Split `<package_id>/<command_id>`.
pub fn parse_command_address(address: &str) -> Option<(&str, &str)> {
    let (package_id, command_id) = address.split_once('/')?;
    (is_package_id(package_id) && is_local_id(command_id)).then_some((package_id, command_id))
}

/// Reverse-DNS, lowercase, two or more labels (manifest-v2.md §Identity).
pub fn is_package_id(id: &str) -> bool {
    id.len() <= 128 && id.split('.').count() >= 2 && id.split('.').all(|label| is_label(label, 128))
}

/// `[a-z][a-z0-9-]{0,31}`.
pub fn is_local_id(id: &str) -> bool {
    is_label(id, 32)
}

fn is_label(label: &str, max: usize) -> bool {
    let mut chars = label.chars();
    label.len() <= max
        && chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// Where a command call came from (WIT `caller-kind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CallerKind {
    Ui,
    Rest,
    Mcp,
    Extension,
    Portal,
}

/// The authenticated caller of a command or subscription (WIT `caller`); `local` is always host-computed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Caller {
    pub did: String,
    pub kind: CallerKind,
    /// The calling package for kind `extension` or `portal`.
    pub package_id: Option<String>,
    /// True when `did` is the host's local DID.
    pub local: bool,
}

/// The rate-limit key of a caller (bus.md §4 check 6).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum CallerKey {
    Remote(String),
    Ui,
    LocalApi,
    Extension(String),
    Portal(String),
}

impl Caller {
    pub(crate) fn key(&self) -> CallerKey {
        if !self.local {
            return CallerKey::Remote(self.did.clone());
        }
        let package = self.package_id.clone().unwrap_or_default();
        match self.kind {
            CallerKind::Ui => CallerKey::Ui,
            CallerKind::Rest | CallerKind::Mcp => CallerKey::LocalApi,
            CallerKind::Extension => CallerKey::Extension(package),
            CallerKind::Portal => CallerKey::Portal(package),
        }
    }
}

/// WIT `delivery-kind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DeliveryKind {
    Snapshot,
    Delta,
    ResyncRequired,
    Ended,
}

impl DeliveryKind {
    /// The wire text (bus.md §5).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Snapshot => "snapshot",
            Self::Delta => "delta",
            Self::ResyncRequired => "resync-required",
            Self::Ended => "ended",
        }
    }
}

/// Why a subscription ended (bus.md §5 `ended` reasons).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndReason {
    Unsubscribed,
    ProducerInactive,
    ConsentRevoked,
    RoleLost,
    InstanceStopped,
    PolicyDenied,
    Revoked,
}

impl EndReason {
    /// The wire text.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unsubscribed => "unsubscribed",
            Self::ProducerInactive => "producer-inactive",
            Self::ConsentRevoked => "consent-revoked",
            Self::RoleLost => "role-lost",
            Self::InstanceStopped => "instance-stopped",
            Self::PolicyDenied => "policy-denied",
            Self::Revoked => "revoked",
        }
    }
}

/// One delivery (WIT `bus-event`); `data` is JSON text shared across fan-out.
#[derive(Debug, Clone, PartialEq)]
pub struct BusEvent {
    pub subscription: u64,
    pub source: Arc<str>,
    pub seq: u64,
    pub kind: DeliveryKind,
    pub dropped_before: u32,
    pub at_ms: u64,
    pub data: Arc<str>,
}

impl BusEvent {
    /// The JSON envelope external clients receive (bus.md §5), built without re-parsing `data`.
    pub fn envelope_text(&self) -> String {
        let source = serde_json::to_string(&*self.source).unwrap_or_else(|_| "\"\"".into());
        format!(
            "{{\"sub\":{},\"source\":{},\"seq\":{},\"kind\":\"{}\",\"dropped_before\":{},\"at_ms\":{},\"data\":{}}}",
            self.subscription,
            source,
            self.seq,
            self.kind.as_str(),
            self.dropped_before,
            self.at_ms,
            self.data
        )
    }
}

/// Guest-reported command failure (WIT `command-error`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandError {
    InvalidInput(String),
    Refused(String),
    Failed(String),
}

impl CommandError {
    /// Counter and audit text.
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidInput(_) => "invalid-input",
            Self::Refused(_) => "refused",
            Self::Failed(_) => "failed",
        }
    }

    /// REST status: invalid-input 400, refused 409, failed 502 (bus.md §4).
    pub fn http_status(&self) -> u16 {
        match self {
            Self::InvalidInput(_) => 400,
            Self::Refused(_) => 409,
            Self::Failed(_) => 502,
        }
    }

    /// How an extension caller sees it in `on-call-result` (contract gap noted in bus/AGENTS.md §calls).
    pub fn into_host_error(self) -> HostError {
        match self {
            Self::InvalidInput(detail) => HostError::InvalidInput(detail),
            Self::Refused(detail) => HostError::PolicyDenied(format!("refused: {detail}")),
            Self::Failed(detail) => HostError::Backend(format!("failed: {detail}")),
        }
    }
}

/// What an admitted command call finally answered.
#[derive(Debug, Clone, PartialEq)]
pub enum CommandOutcome {
    /// The guest's JSON result.
    Ok(String),
    /// The guest refused or failed.
    Guest(CommandError),
    /// The host ended the call: `unknown-outcome` (5 s), `not-available` (instance stopped) or an oversized result.
    Host(HostError),
}

/// Sorted-keys, no-whitespace JSON (the canonicalizer manifest signatures use).
pub fn canonical_json(value: &Value) -> String {
    match value {
        Value::Array(items) => {
            format!("[{}]", items.iter().map(canonical_json).collect::<Vec<_>>().join(","))
        }
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let entries: Vec<String> = keys
                .into_iter()
                .map(|key| format!("{}:{}", Value::String(key.clone()), canonical_json(&map[key])))
                .collect();
            format!("{{{}}}", entries.join(","))
        }
        other => other.to_string(),
    }
}

/// Lowercase-hex blake3 of the canonical JSON (the audit `args_digest`).
pub fn json_digest(value: &Value) -> String {
    blake3::hash(canonical_json(value).as_bytes()).to_hex().to_string()
}
