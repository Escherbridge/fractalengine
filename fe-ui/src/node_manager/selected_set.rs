//! Ordered Petal-local selection reducer; see node_manager/AGENTS.md.

use super::{DragRestore, NodeManager, NodeSelection};
use bevy::prelude::*;

pub const MAX_SELECTED_NODES: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SelectionMode {
    #[default]
    Replace,
    Toggle,
    Range,
    ExtendRange,
    Extend,
}

pub(super) fn reconcile_selection(
    nav: Res<crate::navigation_manager::NavigationManager>,
    visibility: Res<crate::visibility::VisibilityState>,
    live: Query<(
        Entity,
        &crate::plugin::SpawnedNodeMarker,
        Option<&Visibility>,
    )>,
    stamps: Option<Res<crate::actions::asset::StampInteractionState>>,
    mut manager: ResMut<NodeManager>,
    mut transforms: Query<&mut Transform>,
) {
    let canonical = |member: &NodeSelection| {
        crate::verse_manager::parse_stamp_marker_id(&member.node_id).and_then(|(track, index)| {
            stamps
                .as_ref()
                .and_then(|stamps| stamps.promoted_node_id_parts(track, index))
        })
    };
    let matches = |member: &NodeSelection,
                   marker: &crate::plugin::SpawnedNodeMarker,
                   drawn: Option<&Visibility>| {
        let semantic = marker.node_id == member.node_id
            || stamps
                .as_ref()
                .and_then(|stamps| stamps.promoted_stamp_for_node(&member.node_id))
                .is_some_and(|stamp| {
                    crate::verse_manager::parse_stamp_marker_id(&marker.node_id)
                        == Some((stamp.track_node_id.as_str(), stamp.stamp_index))
                });
        semantic
            && nav.active_petal_id.as_deref() == Some(marker.petal_id.as_str())
            && crate::visibility::pickable(&member.node_id, &nav, &visibility)
            && crate::visibility::pickable(&marker.node_id, &nav, &visibility)
            && crate::visibility::rendered(drawn)
    };
    if manager
        .selected_petal()
        .is_some_and(|petal| nav.active_petal_id.as_deref() != Some(petal))
    {
        manager.deselect();
    } else if (manager.selection_petal.is_none()
        && manager.selection_count() > 0
        && nav.active_petal_id.is_some())
        || manager
            .members()
            .any(|member| canonical(member).is_some_and(|id| id != member.node_id))
        || !manager.members().all(|member| {
            live.get(member.entity)
                .is_ok_and(|(_, marker, drawn)| matches(member, marker, drawn))
        })
    {
        let candidates = manager
            .members()
            .filter_map(|member| {
                live.get(member.entity)
                    .ok()
                    .filter(|(_, marker, drawn)| matches(member, marker, *drawn))
                    .or_else(|| {
                        live.iter()
                            .find(|(_, marker, drawn)| matches(member, marker, *drawn))
                    })
                    .map(|(entity, _, _)| SelectionTarget {
                        entity,
                        node_id: canonical(member).unwrap_or(&member.node_id).to_owned(),
                    })
            })
            .collect::<Vec<_>>();
        let mut targets: Vec<SelectionTarget> = Vec::with_capacity(candidates.len());
        for candidate in candidates {
            if !targets.iter().any(|target| {
                target.node_id == candidate.node_id || target.entity == candidate.entity
            }) {
                targets.push(candidate);
            }
        }
        let primary = manager
            .primary()
            .and_then(|primary| {
                targets
                    .iter()
                    .find(|target| target.entity == primary.entity)
            })
            .map(|target| target.node_id.clone())
            .or_else(|| targets.last().map(|target| target.node_id.clone()));
        let petal = nav.active_petal_id.clone();
        manager.replace_members(targets, primary.as_deref(), petal.as_deref());
    }
    for (entity, restore) in manager.take_cancelled_restores() {
        if let Ok(mut transform) = transforms.get_mut(entity) {
            restore.apply(&mut transform);
        }
    }
}

impl SelectionMode {
    pub fn from_modifiers(command: bool, shift: bool) -> Self {
        match (command, shift) {
            (false, false) => Self::Replace,
            (true, false) => Self::Toggle,
            (false, true) => Self::Range,
            (true, true) => Self::ExtendRange,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectionTarget {
    pub entity: Entity,
    pub node_id: String,
}

#[derive(Debug, Clone)]
pub(crate) struct SelectionRequest {
    pub petal_id: String,
    pub node_ids: Vec<String>,
    pub mode: SelectionMode,
    pub visible_order: Vec<String>,
}

impl NodeManager {
    pub(crate) fn selection_is_settled(&self) -> bool {
        self.pending_selection.is_none()
            && self.pending_sidebar_select.is_none()
            && self.pending_navigation_select.is_none()
    }

    pub fn primary(&self) -> Option<&NodeSelection> {
        self.selected.as_ref()
    }

    pub fn selection_count(&self) -> usize {
        self.additional.len() + usize::from(self.selected.is_some())
    }

    pub fn members(&self) -> impl Iterator<Item = &NodeSelection> {
        let index = self.primary_order.min(self.additional.len());
        self.additional[..index]
            .iter()
            .chain(self.selected.iter())
            .chain(self.additional[index..].iter())
    }

    pub fn contains_node(&self, node_id: &str) -> bool {
        self.members().any(|member| member.node_id == node_id)
    }

    pub fn generation(&self) -> u64 {
        self.selection_generation
    }

    pub fn selected_petal(&self) -> Option<&str> {
        self.selection_petal.as_deref()
    }

    pub(crate) fn request_selection(
        &mut self,
        petal_id: &str,
        node_ids: Vec<String>,
        mode: SelectionMode,
        visible_order: Vec<String>,
    ) {
        self.pending_sidebar_select = None;
        self.pending_navigation_select = None;
        self.pending_selection = Some(SelectionRequest {
            petal_id: petal_id.into(),
            node_ids,
            mode,
            visible_order,
        });
    }

    pub(super) fn requested_ids(&self, request: &SelectionRequest) -> Vec<String> {
        if !matches!(
            request.mode,
            SelectionMode::Range | SelectionMode::ExtendRange
        ) {
            return request.node_ids.clone();
        }
        let Some(target) = request.node_ids.last() else {
            return Vec::new();
        };
        let mut visible = Vec::new();
        for id in &request.visible_order {
            if !visible.contains(id) {
                visible.push(id.clone());
            }
        }
        let anchor = self.selection_anchor.as_ref().unwrap_or(target);
        let range = visible
            .iter()
            .position(|id| id == anchor)
            .zip(visible.iter().position(|id| id == target));
        range.map_or_else(
            || vec![target.clone()],
            |(a, b)| visible[a.min(b)..=a.max(b)].to_vec(),
        )
    }

    /// Apply one complete validated input, preserving insertion order and uniqueness.
    pub fn reduce_selection(
        &mut self,
        petal_id: Option<&str>,
        targets: Vec<SelectionTarget>,
        mode: SelectionMode,
    ) -> Result<bool, &'static str> {
        self.reduce_selection_with_primary(petal_id, targets, mode, None)
    }

    pub(super) fn reduce_requested_selection(
        &mut self,
        request: &SelectionRequest,
        targets: Vec<SelectionTarget>,
    ) -> Result<bool, &'static str> {
        self.reduce_selection_with_primary(
            Some(&request.petal_id),
            targets,
            request.mode,
            request.node_ids.last().map(String::as_str),
        )
    }

    fn reduce_selection_with_primary(
        &mut self,
        petal_id: Option<&str>,
        targets: Vec<SelectionTarget>,
        mode: SelectionMode,
        primary_hint: Option<&str>,
    ) -> Result<bool, &'static str> {
        if targets.len() > MAX_SELECTED_NODES {
            return Err("Select at most 256 objects at a time");
        }
        if targets.iter().enumerate().any(|(index, target)| {
            target.node_id.is_empty()
                || targets[..index].iter().any(|previous| {
                    previous.node_id == target.node_id || previous.entity == target.entity
                })
        }) {
            return Err("Selection contains a duplicate or unavailable object");
        }
        let same_petal =
            self.selection_petal.as_deref() == petal_id || self.selection_petal.is_none();
        let mut next: Vec<SelectionTarget> = if same_petal
            && matches!(
                mode,
                SelectionMode::Toggle | SelectionMode::Extend | SelectionMode::ExtendRange
            ) {
            self.members()
                .map(|member| SelectionTarget {
                    entity: member.entity,
                    node_id: member.node_id.clone(),
                })
                .collect()
        } else {
            Vec::new()
        };
        let previous_primary = self.primary().map(|member| member.node_id.clone());
        let clicked = primary_hint
            .map(str::to_owned)
            .or_else(|| targets.last().map(|target| target.node_id.clone()));
        let remove = mode == SelectionMode::Toggle
            && !targets.is_empty()
            && targets
                .iter()
                .all(|target| next.iter().any(|member| member.node_id == target.node_id));
        if remove {
            next.retain(|member| {
                !targets
                    .iter()
                    .any(|target| target.node_id == member.node_id)
            });
        } else {
            for target in targets {
                if !next.iter().any(|member| member.node_id == target.node_id) {
                    next.push(target);
                }
            }
        }
        if next.iter().enumerate().any(|(index, member)| {
            next[..index].iter().any(|previous| {
                previous.node_id == member.node_id || previous.entity == member.entity
            })
        }) {
            return Err("The complete selection contains a semantic or rendered-entity alias; no member was changed");
        }
        if next.len() > MAX_SELECTED_NODES {
            return Err("Select at most 256 objects at a time");
        }
        let primary = clicked
            .as_ref()
            .filter(|id| next.iter().any(|member| &member.node_id == *id))
            .or_else(|| {
                previous_primary
                    .as_ref()
                    .filter(|id| next.iter().any(|member| &member.node_id == *id))
            })
            .cloned()
            .or_else(|| next.last().map(|member| member.node_id.clone()));
        let changed = self.replace_members(next, primary.as_deref(), petal_id);
        if !matches!(mode, SelectionMode::Range | SelectionMode::ExtendRange) {
            self.selection_anchor = clicked;
        }
        self.pending_navigation_select = None;
        self.pending_sidebar_select = None;
        self.pending_selection = None;
        Ok(changed)
    }

    pub(super) fn replace_members(
        &mut self,
        targets: Vec<SelectionTarget>,
        primary: Option<&str>,
        petal_id: Option<&str>,
    ) -> bool {
        let unchanged = self.selection_petal.as_deref() == petal_id
            && self.primary().map(|member| member.node_id.as_str()) == primary
            && self.selection_count() == targets.len()
            && self
                .members()
                .zip(&targets)
                .all(|(old, new)| old.entity == new.entity && old.node_id == new.node_id);
        if unchanged {
            return false;
        }
        self.cancel_group_drag();
        let previous_id = self.primary().map(|primary| primary.node_id.clone());
        if let Some((entity, transform)) = self.cancel_axis_drag() {
            self.cancelled_restores.push(super::CancelledRestore {
                entity,
                node_id: previous_id.unwrap(),
                transform,
            });
        }
        let index = primary.and_then(|id| targets.iter().position(|target| target.node_id == id));
        let mut members: Vec<NodeSelection> = targets
            .into_iter()
            .map(|target| NodeSelection {
                entity: target.entity,
                node_id: target.node_id,
                drag: None,
                drag_committed: false,
            })
            .collect();
        self.selected = index.map(|index| members.remove(index));
        self.primary_order = index.unwrap_or(0);
        self.additional = members;
        self.selection_petal = petal_id.map(str::to_owned);
        self.selection_generation = self.selection_generation.wrapping_add(1);
        self.pending_transform_before = None;
        true
    }

    pub(crate) fn retain_members(&mut self, mut keep: impl FnMut(&NodeSelection) -> bool) {
        let members: Vec<_> = self
            .members()
            .filter(|member| keep(member))
            .map(|member| SelectionTarget {
                entity: member.entity,
                node_id: member.node_id.clone(),
            })
            .collect();
        let primary = self
            .primary()
            .filter(|member| members.iter().any(|kept| kept.node_id == member.node_id))
            .map(|member| member.node_id.clone())
            .or_else(|| members.last().map(|member| member.node_id.clone()));
        let petal = self.selection_petal.clone();
        self.replace_members(members, primary.as_deref(), petal.as_deref());
    }

    pub(super) fn take_cancelled_restores(&mut self) -> Vec<(Entity, DragRestore)> {
        std::mem::take(&mut self.cancelled_restores)
            .into_iter()
            .map(|restore| (restore.entity, restore.transform))
            .collect()
    }
}
