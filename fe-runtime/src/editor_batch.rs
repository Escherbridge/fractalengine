//! Checked Petal-local editor batches; see src/AGENTS.md section editor-batches.

use serde::{Deserialize, Serialize};
use serde_json::Value;
mod affine;

use crate::{
    messages::{StampOverrideEdit, VisibilityGroupDto},
    node_history::{NodeRowSnapshot, NodeTransformValue},
};

pub const MAX_EDITOR_BATCH_TARGETS: usize = 256;
pub const MAX_EDITOR_BATCH_BYTES: usize = 4 * 1024 * 1024;

/// Stored path context for a promoted stamp, never an authority grant.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditorStampPath {
    pub state: NodeRowSnapshot,
    pub history_version: Option<i64>,
}

/// One ordered stored object and its exact capture version.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditorBatchMember {
    pub node_id: String,
    pub history_version: Option<i64>,
    pub state: NodeRowSnapshot,
    pub stamp_path: Option<EditorStampPath>,
}

/// Stored target authority; Group members are expanded by the DB writer.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum EditorBatchTarget {
    #[default]
    Nodes,
    Group {
        group_id: String,
    },
    DeletedGroup {
        delete_request_id: String,
    },
}

/// Server-derived exact inverse context, backed by a durable checked delete.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditorDeletedGroup {
    pub group_id: String,
    pub registry_before: Vec<VisibilityGroupDto>,
    pub memberships: Vec<EditorMembershipValue>,
}

/// Scope-checked selection read; writes revalidate every field at use.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditorBatchCapture {
    #[serde(default)]
    pub target: EditorBatchTarget,
    #[serde(default)]
    pub deleted_group: Option<EditorDeletedGroup>,
    pub scope: String,
    pub petal_id: String,
    pub members: Vec<EditorBatchMember>,
    pub groups: Vec<VisibilityGroupDto>,
    pub petal_snapshot: String,
    pub world_scale: f32,
}

/// Affine delta: pivot, per-axis scale, unit rotation, then translation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditorAffine {
    pub pivot: [f64; 3],
    pub translation: [f64; 3],
    pub rotation: [f32; 4],
    pub scale: [f64; 3],
}

/// Explicit property changes, including deletion distinct from JSON null.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum EditorPropertyEdit {
    Set { key: String, value: Value },
    Delete { key: String },
}

/// Exact membership representation retained for inverse edits.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum EditorMembershipValue {
    Absent,
    Null,
    Groups(Vec<String>),
}

/// Exact stored presence for one of the three named stamp override keys.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum EditorStampOverrideValue {
    Absent,
    Null,
    Value(Value),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditorStampOverrideSet {
    pub scale: EditorStampOverrideValue,
    pub rotation: EditorStampOverrideValue,
    pub arc_m: EditorStampOverrideValue,
}

/// Type-specific mutation; ordinary transforms cannot mutate path/stamp projections.
#[derive(Debug, Clone)]
pub enum EditorTargetMutation {
    Transform(NodeTransformValue),
    PathAffine(EditorAffine),
    PathRestore {
        points: Value,
    },
    StampOverrides(StampOverrideEdit),
    StampRestore {
        edit: StampOverrideEdit,
        overrides: EditorStampOverrideSet,
    },
    Properties(Vec<EditorPropertyEdit>),
}

#[derive(Debug, Clone)]
pub struct EditorTargetEdit {
    pub node_id: String,
    pub mutation: EditorTargetMutation,
}

/// Every edit applies to the complete captured target set or refuses.
#[derive(Debug, Clone)]
pub enum EditorBatchEdit {
    Targets(Vec<EditorTargetEdit>),
    CreateGroup {
        id: String,
        name: String,
    },
    DeleteGroup {
        group_id: String,
    },
    RestoreDeletedGroup {
        delete_request_id: String,
    },
    RedoCreatedGroup {
        undo_request_id: String,
    },
    UndoCreatedGroup {
        create_request_id: String,
        group_id: String,
        memberships: Vec<EditorMembershipValue>,
    },
    Membership {
        group_id: String,
        member: bool,
    },
    RestoreMemberships(Vec<EditorMembershipValue>),
}

#[derive(Debug, Clone)]
pub struct EditorBatchRequest {
    pub capture: EditorBatchCapture,
    pub edit: EditorBatchEdit,
}

/// Complete immutable outcome; no member receipt implies a partial batch success.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditorBatchReceipt {
    #[serde(default)]
    pub target: EditorBatchTarget,
    pub request_id: String,
    pub scope: String,
    pub petal_id: String,
    pub members: Vec<EditorBatchMember>,
    pub groups: Vec<VisibilityGroupDto>,
    pub created_group_id: Option<String>,
}

/// Exact prior outcome plus independently guarded current readback.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditorBatchResolution {
    pub request_id: String,
    pub receipt: Option<EditorBatchReceipt>,
    pub current: EditorBatchCapture,
}
