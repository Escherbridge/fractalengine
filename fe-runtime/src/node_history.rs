//! Typed ordinary object commit/read boundaries; see src/AGENTS.md §ordinary-object-history.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Decode legacy quaternion or ordinary Euler XYZ storage into a unit quaternion.
pub fn stored_rotation_quaternion(rotation: &[f64]) -> Option<[f32; 4]> {
    use bevy::math::{EulerRot, Quat};
    if rotation
        .iter()
        .any(|value| !value.is_finite() || !(*value as f32).is_finite())
    {
        return None;
    }
    let quaternion = match rotation {
        [x, y, z] => Quat::from_euler(EulerRot::XYZ, *x as f32, *y as f32, *z as f32),
        [x, y, z, w] => Quat::from_array([*x as f32, *y as f32, *z as f32, *w as f32]),
        _ => return None,
    };
    let length = quaternion.length_squared();
    (length.is_finite() && length > f32::EPSILON).then(|| quaternion.normalize().to_array())
}

/// Trusted admission context, constructed by API auth or the local desktop.
#[derive(Debug, Clone)]
pub enum NodeAccessGrant {
    /// Internal desktop command; DB resolves local identity and checks stored roles.
    Local,
    /// Validated token grant, rechecked against stored ancestry at DB use.
    Scoped(String),
}

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
    UnavailableScope,
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

/// Typed full transform used by editor compare-and-swap commands.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeTransformValue {
    /// Petal-local XYZ position.
    pub position: [f64; 3],
    /// Quaternion orientation, compared with sign equivalence.
    pub rotation: [f32; 4],
    /// Per-axis scale.
    pub scale: [f64; 3],
}

impl NodeTransformValue {
    /// Compare finite transforms with sign-equivalent quaternion orientation.
    pub fn matches(&self, other: &Self) -> bool {
        use bevy::math::Quat;
        let finite = |value: &Self| {
            value
                .position
                .iter()
                .chain(&value.scale)
                .all(|v| v.is_finite())
                && value.rotation.iter().all(|v| v.is_finite())
                && Quat::from_array(value.rotation).length_squared() > f32::EPSILON
        };
        finite(self)
            && finite(other)
            && self
                .position
                .iter()
                .zip(other.position)
                .chain(self.scale.iter().zip(other.scale))
                .all(|(left, right)| (*left - right).abs() <= 0.0001)
            && Quat::from_array(self.rotation)
                .normalize()
                .dot(Quat::from_array(other.rotation).normalize())
                .abs()
                >= 1.0 - 0.000001
    }
    /// Decode the shared persisted transform representation.
    pub fn from_row(row: &NodeRowSnapshot) -> Option<Self> {
        Some(Self {
            position: row.position,
            rotation: stored_rotation_quaternion(&row.rotation)?,
            scale: row.scale,
        })
    }
}

/// Relevant state required before an editor operation may commit.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum NodeEditBefore {
    /// Full transform at the start of the edit.
    Transform(NodeTransformValue),
    /// Complete stored point rows before one position changes.
    PathPoints(Value),
}

/// Editor-only compare-and-swap precondition, checked again inside the transaction.
#[derive(Debug, Clone)]
pub struct NodeEditPrecondition {
    /// Full captured ancestry, never an authorization grant.
    pub scope: String,
    /// Known committed version; initial hydrated edits may rely on state CAS.
    pub history_version: Option<i64>,
    /// Relevant state the transaction must still contain.
    pub before: NodeEditBefore,
}
