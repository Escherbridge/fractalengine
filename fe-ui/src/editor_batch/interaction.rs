//! Shared transform routing and cached geometry; see editor_batch/AGENTS.md.

use super::{math::member_transform, EditorBatchState};
use crate::{
    gis::PathEditorState,
    navigation_manager::NavigationManager,
    node_manager::{NodeManager, SelectionKind},
    panels::toolbar::Tool,
};
use bevy::prelude::*;
use fe_runtime::editor_batch::EditorBatchCapture;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TransformRoute {
    Waiting,
    OrdinarySingle,
    CheckedWholeObject,
    PathSubSelection,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MemberKind {
    Ordinary,
    Path,
    Stamp,
}

#[derive(Debug)]
pub(crate) struct InteractionMember {
    pub kind: MemberKind,
    pub transform: Transform,
}

#[derive(Debug)]
pub(crate) struct SelectionInteraction {
    pub members: Result<Vec<InteractionMember>, String>,
    pub center: Result<Vec3, String>,
    derived: bool,
    pub source_limitation: Option<&'static str>,
    capabilities: [Result<(), String>; 4],
}

impl SelectionInteraction {
    pub(super) fn capture(capture: &EditorBatchCapture) -> Self {
        let members = capture
            .members
            .iter()
            .map(|member| {
                Ok(InteractionMember {
                    kind: if member.stamp_path.is_some() {
                        MemberKind::Stamp
                    } else if member.state.properties.contains_key("gpx_points") {
                        MemberKind::Path
                    } else {
                        MemberKind::Ordinary
                    },
                    transform: member_transform(member, capture.world_scale)?,
                })
            })
            .collect::<Result<Vec<_>, String>>();
        let center = members.as_ref().map_err(Clone::clone).and_then(|members| {
            if members.is_empty() {
                return Err("Select at least one object".into());
            }
            let center = (members
                .iter()
                .map(|member| member.transform.translation.as_dvec3())
                .sum::<bevy::math::DVec3>()
                / members.len() as f64)
                .as_vec3();
            if !center.is_finite() {
                return Err("Selection center is outside the supported numeric range".into());
            }
            Ok(center)
        });
        let capability = |tool: Tool, uniform_scale: bool| -> Result<(), String> {
            if capture.members.iter().any(super::source::closed_path) {
                return Err(super::source::CLOSED_PATH_REASON.into());
            }
            let pivot = *center.as_ref().map_err(Clone::clone)?;
            for (source, member) in capture
                .members
                .iter()
                .zip(members.as_ref().map_err(Clone::clone)?)
            {
                if let Some(path) = &source.stamp_path {
                    if capture
                        .members
                        .iter()
                        .any(|member| member.node_id == path.state.node_id)
                    {
                        return Err("A path and its stamp cannot be transformed together".into());
                    }
                    if tool == Tool::Move
                        || !member.transform.translation.abs_diff_eq(pivot, 0.0001)
                    {
                        return Err("The selection includes a path-locked stamp; this center-based tool would move it off its path".into());
                    }
                }
                if tool == Tool::Scale
                    && capture.members.len() > 1
                    && !uniform_scale
                    && member.kind == MemberKind::Ordinary
                    && member.transform.rotation.dot(Quat::IDENTITY).abs() < 1.0 - 0.000001
                {
                    return Err("Axis scaling of a rotated selected object would require an unsupported shear; uniform numeric scaling remains available".into());
                }
            }
            Ok(())
        };
        let capabilities = [
            capability(Tool::Move, false),
            capability(Tool::Rotate, false),
            capability(Tool::Scale, false),
            capability(Tool::Scale, true),
        ];
        Self {
            members,
            center,
            derived: capture.members.iter().any(|member| {
                member.stamp_path.is_some() || member.state.properties.contains_key("gpx_points")
            }),
            source_limitation: capture
                .members
                .iter()
                .any(super::source::closed_path)
                .then_some(super::source::CLOSED_PATH_REASON),
            capabilities,
        }
    }

    pub(crate) fn capability(&self, tool: Tool, uniform_scale: bool) -> Result<(), &str> {
        let index = match tool {
            Tool::Move => 0,
            Tool::Rotate => 1,
            Tool::Scale if uniform_scale => 3,
            Tool::Scale => 2,
            _ => return Ok(()),
        };
        self.capabilities[index]
            .as_ref()
            .map(|_| ())
            .map_err(String::as_str)
    }
}

impl EditorBatchState {
    pub(crate) fn transform_route(
        &self,
        nav: &NavigationManager,
        nodes: &NodeManager,
        path: Option<&PathEditorState>,
    ) -> TransformRoute {
        if nodes.selection_count() <= 1
            && path.is_some_and(|path| {
                path.editing_track_id.is_some()
                    && nodes.primary().is_none_or(|selected| {
                        path.editing_track_id.as_deref() == Some(selected.node_id.as_str())
                    })
                    && (path
                        .selected_point
                        .is_some_and(|idx| idx < path.points.len())
                        || path
                            .selected_segment
                            .is_some_and(|idx| idx + 1 < path.points.len()))
            })
        {
            return TransformRoute::PathSubSelection;
        }
        let Some(interaction) = self.interaction(nav, nodes) else {
            return TransformRoute::Waiting;
        };
        if nodes.selection_count() > 1 || interaction.derived {
            TransformRoute::CheckedWholeObject
        } else {
            TransformRoute::OrdinarySingle
        }
    }

    pub(crate) fn selection_kind(
        &self,
        nav: &NavigationManager,
        nodes: &NodeManager,
        path: &PathEditorState,
    ) -> SelectionKind {
        if nodes.selection_count() > 1 {
            return SelectionKind::Multiple {
                count: nodes.selection_count(),
            };
        }
        let sub_selection =
            self.transform_route(nav, nodes, Some(path)) == TransformRoute::PathSubSelection;
        let kind = crate::node_manager::project_selection(
            nodes
                .primary()
                .map(|selected| (selected.entity, selected.node_id.as_str())),
            path.editing_track_id.as_deref(),
            path.selected_point.filter(|_| sub_selection),
            path.selected_segment.filter(|_| sub_selection),
        );
        if let SelectionKind::Node(entity) = kind {
            match self
                .interaction(nav, nodes)
                .and_then(|interaction| interaction.members.as_ref().ok())
                .and_then(|members| members.first())
                .map(|member| member.kind)
            {
                Some(MemberKind::Stamp) => SelectionKind::Stamp(entity),
                Some(MemberKind::Path) => SelectionKind::PathTrack {
                    track_id: nodes.primary().unwrap().node_id.clone(),
                },
                _ => kind,
            }
        } else {
            kind
        }
    }
}
