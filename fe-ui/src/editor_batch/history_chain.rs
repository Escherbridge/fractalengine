//! Exact owned inverse links shared by both histories; see AGENTS.md.

use fe_runtime::{
    editor_batch::*,
    node_history::{NodeEditBefore, NodeRowSnapshot},
};
use serde::Serialize;
use std::collections::VecDeque;

#[derive(Debug, Clone, Serialize)]
enum Restored {
    Batch(EditorBatchCapture),
    Ordinary {
        node_id: String,
        version: i64,
        before: NodeEditBefore,
    },
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct OwnedRestore {
    scope: String,
    petal_id: String,
    pub(crate) redo: bool,
    restored: Restored,
    saved: Vec<EditorBatchMember>,
}

fn equal(left: &impl Serialize, right: &impl Serialize) -> bool {
    serde_json::to_value(left)
        .ok()
        .zip(serde_json::to_value(right).ok())
        .is_some_and(|(a, b)| a == b)
}

pub(crate) fn matches(before: &NodeEditBefore, state: &NodeRowSnapshot) -> bool {
    match before {
        NodeEditBefore::Transform(value) => {
            fe_runtime::node_history::NodeTransformValue::from_row(state)
                .is_some_and(|now| value.matches(&now))
        }
        NodeEditBefore::PathPoints(points) => state.properties.get("gpx_points") == Some(points),
    }
}

impl OwnedRestore {
    pub(crate) fn batch(
        restored: &EditorBatchCapture,
        saved: &EditorBatchCapture,
        redo: bool,
    ) -> Self {
        Self {
            scope: restored.scope.clone(),
            petal_id: restored.petal_id.clone(),
            redo,
            restored: Restored::Batch(restored.clone()),
            saved: saved.members.clone(),
        }
    }

    pub(crate) fn ordinary(
        scope: String,
        petal_id: String,
        node_id: String,
        version: Option<i64>,
        before: NodeEditBefore,
        saved: EditorBatchMember,
        redo: bool,
    ) -> Option<Self> {
        let version = version.filter(|v| *v > 0)?;
        Some(Self {
            scope,
            petal_id,
            redo,
            restored: Restored::Ordinary {
                node_id,
                version,
                before,
            },
            saved: vec![saved],
        })
    }

    fn row(
        &self,
        id: &str,
        version: Option<i64>,
        state: &NodeRowSnapshot,
    ) -> Option<&EditorBatchMember> {
        let saved = self.saved.iter().find(|member| member.node_id == id)?;
        match &self.restored {
            Restored::Batch(capture) => {
                let original = capture.members.iter().find(|member| member.node_id == id)?;
                if version.is_none()
                    || version != original.history_version
                    || !equal(state, &original.state)
                {
                    return None;
                }
            }
            Restored::Ordinary {
                node_id,
                version: original_version,
                before,
            } => {
                if id != node_id || version != Some(*original_version) || !matches(before, state) {
                    return None;
                }
                let mut unchanged = state.clone();
                match before {
                    NodeEditBefore::Transform(_) => {
                        unchanged.position = saved.state.position;
                        unchanged.elevation = saved.state.elevation;
                        unchanged.rotation = saved.state.rotation.clone();
                        unchanged.scale = saved.state.scale;
                        unchanged.edit_seq = saved.state.edit_seq;
                    }
                    NodeEditBefore::PathPoints(_) => {
                        unchanged.properties.insert(
                            "gpx_points".into(),
                            saved.state.properties.get("gpx_points")?.clone(),
                        );
                    }
                }
                if !equal(&unchanged, &saved.state) {
                    return None;
                }
            }
        }
        Some(saved)
    }

    pub(crate) fn rebase_capture(&self, expected: &mut EditorBatchCapture) {
        if expected.scope != self.scope || expected.petal_id != self.petal_id {
            return;
        }
        if let Restored::Batch(original) = &self.restored {
            if original.world_scale != expected.world_scale
                || original.terrain_source_fingerprint != expected.terrain_source_fingerprint
                || !equal(&original.groups, &expected.groups)
            {
                return;
            }
        }
        for member in &mut expected.members {
            let same_context = match &self.restored {
                Restored::Batch(original) => original
                    .members
                    .iter()
                    .find(|old| old.node_id == member.node_id)
                    .is_some_and(|old| equal(&old.stamp_path, &member.stamp_path)),
                Restored::Ordinary { .. } => member.stamp_path.is_none(),
            };
            if same_context {
                if let Some(saved) =
                    self.row(&member.node_id, member.history_version, &member.state)
                {
                    member.state = saved.state.clone();
                    member.history_version = saved.history_version;
                    if matches!(&self.restored, Restored::Batch(_)) {
                        member.stamp_path = saved.stamp_path.clone();
                    }
                }
            }
            if let Some(path) = &mut member.stamp_path {
                if let Some(saved) =
                    self.row(&path.state.node_id, path.history_version, &path.state)
                {
                    path.state = saved.state.clone();
                    path.history_version = saved.history_version;
                }
            }
        }
    }

    pub(crate) fn ordinary_version(
        &self,
        scope: &str,
        petal: &str,
        id: &str,
        version: Option<i64>,
        before: &NodeEditBefore,
    ) -> Option<i64> {
        if scope != self.scope || petal != self.petal_id {
            return None;
        }
        let saved = self.saved.iter().find(|member| member.node_id == id)?;
        let linked = match &self.restored {
            Restored::Batch(capture) => capture
                .members
                .iter()
                .find(|member| member.node_id == id)
                .is_some_and(|old| {
                    version.is_some()
                        && version == old.history_version
                        && matches(before, &old.state)
                }),
            Restored::Ordinary {
                node_id,
                version: old,
                before: original,
            } => {
                id == node_id
                    && version == Some(*old)
                    && match (before, original) {
                        (NodeEditBefore::Transform(a), NodeEditBefore::Transform(b)) => {
                            a.matches(b)
                        }
                        (NodeEditBefore::PathPoints(a), NodeEditBefore::PathPoints(b)) => a == b,
                        _ => false,
                    }
            }
        };
        (linked && matches(before, &saved.state))
            .then_some(saved.history_version)
            .flatten()
    }
}

pub(crate) fn queue(queue: &mut VecDeque<OwnedRestore>, link: OwnedRestore) {
    queue.push_back(link);
    while queue.len() > 50
        || serde_json::to_vec(queue).map_or(true, |bytes| bytes.len() > MAX_EDITOR_BATCH_BYTES)
    {
        queue.pop_front();
    }
}
