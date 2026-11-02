//! Typed ordinary object commit/read boundaries; see src/AGENTS.md §ordinary-object-history.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Persisted node-table snapshot with XYZ position and stored rotation components.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeRowSnapshot {
    pub node_id: String,
    pub petal_id: String,
    pub display_name: Option<String>,
    pub asset_id: Option<String>,
    pub position: [f64; 3],
    pub elevation: f64,
    /// Legacy initial rows may have quaternion4; ordinary full transforms store Euler3.
    pub rotation: Vec<f64>,
    pub scale: [f64; 3],
    pub interactive: bool,
    pub created_at: String,
    pub edit_seq: i64,
    pub properties: serde_json::Map<String, Value>,
    pub tombstone: Option<Value>,
}

/// Confirmed current materialization; historical reads use a different result type.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeMutationReceipt {
    pub node_id: String,
    pub history_version: i64,
    pub state: NodeRowSnapshot,
    pub persist: PersistStatus,
}

/// A receipt always confirms persistence.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PersistStatus {
    Committed,
}

/// Scope-safe journal/replay metadata; commit versions order one object only.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeHistoryCoverage {
    pub node_id: String,
    pub journal_version: u64,
    pub current_version: i64,
    pub coverage_start_version: Option<i64>,
    pub coverage_started_at: Option<String>,
    pub supported_operations: Vec<String>,
    pub order: String,
    pub event_time_ordering: bool,
    pub current_state_verified: bool,
}

/// One retained immutable journal fact, including partial legacy facts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeHistoryEntry {
    pub log_id: String,
    pub node_id: String,
    pub hlc_timestamp: i64,
    pub op: String,
    pub source_did: String,
    pub payload: Value,
    pub row_version: i64,
    pub created_at: String,
}

/// Forward cursor page; historical facts never apply themselves to the live scene.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeHistoryPage {
    #[serde(flatten)]
    pub coverage: NodeHistoryCoverage,
    pub entries: Vec<NodeHistoryEntry>,
    pub next_after_version: Option<i64>,
}

/// Why an exact committed version can or cannot be reconstructed.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HistoryAvailability {
    Available,
    BeforeCoverage,
    IncompleteInterval,
    UnsupportedOperation,
    VersionUnavailable,
}

/// Read-only historical preview, distinct from a current-state commit receipt.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeStateAtVersion {
    #[serde(flatten)]
    pub coverage: NodeHistoryCoverage,
    pub at_version: i64,
    pub availability: HistoryAvailability,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub baseline_version: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<NodeRowSnapshot>,
}
