//! Typed selection read-model (FR-1): a per-frame **projection** over the two
//! selection authorities — `NodeManager.selected` and `PathEditorState.*` — so
//! any system can ask "what kind of thing is selected?" without merging their
//! storage (the split is codified in `conductor/code_styleguides/ui_ux.md §5`).
//! Feeds the gimbal (FR-3) and the object-aware left-click dispatch (FR-2). See
//! `fe-ui/src/node_manager/AGENTS.md` §selection-read-model.

use bevy::prelude::*;

use crate::gis::PathEditorState;
use crate::node_manager::NodeManager;

/// What kind of object the editor currently has selected. A read-only
/// projection (never stored authority) — see [`project_selection`]. `Stamp` and
/// `TerrainProposal` have no selection path yet (they gain one in FR-2 / the
/// terrain-proposal phases); they exist so dispatch can already match on them.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum SelectionKind {
    /// Nothing selected.
    #[default]
    Empty,
    /// A scene node (glTF / primitive) selected as an ECS entity.
    Node(Entity),
    /// A whole path track (its ribbon), keyed by track node id.
    PathTrack { track_id: String },
    /// A single path vertex — index into the edited track's `points`.
    PathVertex { track_id: String, idx: usize },
    /// A path segment `idx` spanning `points[idx] → points[idx + 1]`.
    PathSegment { track_id: String, idx: usize },
    /// A materialized path-asset stamp entity (future: FR-2 stamp ops).
    Stamp(Entity),
    /// A proposed terrain-edit overlay (future: FR-5 proposal editing).
    TerrainProposal { id: String },
}

/// The current selection projection, recomputed each frame by
/// [`update_selection_state`]. Read-only for every consumer.
#[derive(Resource, Default)]
pub struct SelectionState {
    pub kind: SelectionKind,
}

/// Project the two authorities into a single [`SelectionKind`] (FR-1). Pure so
/// the priority truth-table is unit-testable. Priority, most specific first:
/// path vertex → path segment → (a selected entity that IS the open track's
/// bridged ribbon reads as the whole track, else a plain node) → an open track
/// with no bridged entity → empty. Never merges storage (ui_ux.md §5).
pub fn project_selection(
    node_selected: Option<(Entity, &str)>,
    editing_track_id: Option<&str>,
    selected_point: Option<usize>,
    selected_segment: Option<usize>,
) -> SelectionKind {
    // Path sub-selections are the most specific and require an open track.
    if let Some(track_id) = editing_track_id {
        if let Some(idx) = selected_point {
            return SelectionKind::PathVertex {
                track_id: track_id.to_string(),
                idx,
            };
        }
        if let Some(idx) = selected_segment {
            return SelectionKind::PathSegment {
                track_id: track_id.to_string(),
                idx,
            };
        }
    }
    // A selected entity wins next — but if it is the open track's own bridged
    // ribbon (node_id == editing_track_id), that reads as a whole-track select
    // so ribbon-vs-node stays type-distinguishable for dispatch (FR-2).
    if let Some((entity, node_id)) = node_selected {
        if editing_track_id == Some(node_id) {
            return SelectionKind::PathTrack {
                track_id: node_id.to_string(),
            };
        }
        return SelectionKind::Node(entity);
    }
    // Track open in the Paths tab with no bridged entity (e.g. ribbon unspawned).
    if let Some(track_id) = editing_track_id {
        return SelectionKind::PathTrack {
            track_id: track_id.to_string(),
        };
    }
    SelectionKind::Empty
}

/// World-space gimbal target for a *path* selection (FR-3): the vertex position,
/// the segment midpoint, or the track centroid. `points` are the edited track's
/// vertices in the same petal-local world frame as the rendered ribbon/markers.
/// `None` for non-path selections (they use the entity-based `gimbal_center`) or
/// out-of-range indices / an empty track.
pub fn path_gimbal_target(kind: &SelectionKind, points: &[Vec3]) -> Option<Vec3> {
    match kind {
        SelectionKind::PathVertex { idx, .. } => points.get(*idx).copied(),
        SelectionKind::PathSegment { idx, .. } => {
            let a = points.get(*idx)?;
            let b = points.get(*idx + 1)?;
            Some((*a + *b) * 0.5)
        }
        SelectionKind::PathTrack { .. } => {
            if points.is_empty() {
                None
            } else {
                Some(points.iter().copied().sum::<Vec3>() / points.len() as f32)
            }
        }
        _ => None,
    }
}

/// `true` for the path selection kinds that draw a gimbal at a resolved world
/// target rather than at an entity (FR-3).
pub fn is_path_selection(kind: &SelectionKind) -> bool {
    matches!(
        kind,
        SelectionKind::PathVertex { .. }
            | SelectionKind::PathSegment { .. }
            | SelectionKind::PathTrack { .. }
    )
}

/// Recompute [`SelectionState`] from the two authorities each frame. Runs late
/// in the selection chain (after the pick systems, before `draw_gimbal_system`).
pub(super) fn update_selection_state(
    manager: Res<NodeManager>,
    path_state: Res<PathEditorState>,
    mut selection: ResMut<SelectionState>,
) {
    let node_selected = manager
        .selected
        .as_ref()
        .map(|s| (s.entity, s.node_id.as_str()));
    let kind = project_selection(
        node_selected,
        path_state.editing_track_id.as_deref(),
        path_state.selected_point,
        path_state.selected_segment,
    );
    if selection.kind != kind {
        selection.kind = kind;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entity(n: u64) -> Entity {
        Entity::from_bits(n)
    }

    // --- projection truth table (FR-1) ---

    #[test]
    fn empty_when_no_authority_set() {
        assert_eq!(
            project_selection(None, None, None, None),
            SelectionKind::Empty
        );
    }

    #[test]
    fn plain_node_selection() {
        assert_eq!(
            project_selection(Some((entity(7), "glb-1")), None, None, None),
            SelectionKind::Node(entity(7))
        );
    }

    #[test]
    fn node_wins_over_a_different_open_track() {
        // User clicked a glb while some other track is still open in Paths.
        assert_eq!(
            project_selection(Some((entity(7), "glb-1")), Some("track-9"), None, None),
            SelectionKind::Node(entity(7))
        );
    }

    #[test]
    fn bridged_ribbon_reads_as_whole_track() {
        // The selected entity IS the open track's ribbon (node_id == track_id).
        assert_eq!(
            project_selection(Some((entity(3), "track-1")), Some("track-1"), None, None),
            SelectionKind::PathTrack {
                track_id: "track-1".to_string()
            }
        );
    }

    #[test]
    fn open_track_without_bridged_entity_is_path_track() {
        assert_eq!(
            project_selection(None, Some("track-1"), None, None),
            SelectionKind::PathTrack {
                track_id: "track-1".to_string()
            }
        );
    }

    #[test]
    fn vertex_is_most_specific() {
        // Vertex beats both the bridged ribbon entity and a segment.
        assert_eq!(
            project_selection(Some((entity(3), "track-1")), Some("track-1"), Some(2), None),
            SelectionKind::PathVertex {
                track_id: "track-1".to_string(),
                idx: 2
            }
        );
    }

    #[test]
    fn segment_beats_track_but_not_vertex() {
        assert_eq!(
            project_selection(None, Some("track-1"), None, Some(4)),
            SelectionKind::PathSegment {
                track_id: "track-1".to_string(),
                idx: 4
            }
        );
        // A vertex present alongside a segment still wins (mutual exclusion is
        // enforced upstream, but the projection is deterministic regardless).
        assert_eq!(
            project_selection(None, Some("track-1"), Some(0), Some(4)),
            SelectionKind::PathVertex {
                track_id: "track-1".to_string(),
                idx: 0
            }
        );
    }

    #[test]
    fn sub_selection_ignored_without_open_track() {
        // Defensive: a stray point index with no open track never fabricates a
        // path selection.
        assert_eq!(
            project_selection(None, None, Some(2), None),
            SelectionKind::Empty
        );
    }

    // --- gimbal target resolution (FR-3) ---

    #[test]
    fn vertex_target_is_the_point() {
        let pts = [Vec3::new(1.0, 2.0, 3.0), Vec3::new(4.0, 5.0, 6.0)];
        let k = SelectionKind::PathVertex {
            track_id: "t".into(),
            idx: 1,
        };
        assert_eq!(path_gimbal_target(&k, &pts), Some(Vec3::new(4.0, 5.0, 6.0)));
    }

    #[test]
    fn segment_target_is_the_midpoint() {
        let pts = [Vec3::new(0.0, 0.0, 0.0), Vec3::new(10.0, 0.0, 4.0)];
        let k = SelectionKind::PathSegment {
            track_id: "t".into(),
            idx: 0,
        };
        assert_eq!(path_gimbal_target(&k, &pts), Some(Vec3::new(5.0, 0.0, 2.0)));
    }

    #[test]
    fn track_target_is_the_centroid() {
        let pts = [
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(6.0, 0.0, 0.0),
            Vec3::new(0.0, 0.0, 6.0),
        ];
        let k = SelectionKind::PathTrack { track_id: "t".into() };
        assert_eq!(path_gimbal_target(&k, &pts), Some(Vec3::new(2.0, 0.0, 2.0)));
    }

    #[test]
    fn out_of_range_and_empty_yield_none() {
        let k_v = SelectionKind::PathVertex {
            track_id: "t".into(),
            idx: 9,
        };
        assert_eq!(path_gimbal_target(&k_v, &[Vec3::ZERO]), None);
        // Segment needs idx+1 in range.
        let k_s = SelectionKind::PathSegment {
            track_id: "t".into(),
            idx: 0,
        };
        assert_eq!(path_gimbal_target(&k_s, &[Vec3::ZERO]), None);
        // Empty track has no centroid.
        let k_t = SelectionKind::PathTrack { track_id: "t".into() };
        assert_eq!(path_gimbal_target(&k_t, &[]), None);
        // Non-path selections have no path target.
        assert_eq!(path_gimbal_target(&SelectionKind::Node(entity(1)), &[]), None);
        assert_eq!(path_gimbal_target(&SelectionKind::Empty, &[]), None);
    }

    #[test]
    fn is_path_selection_matches_only_path_kinds() {
        assert!(is_path_selection(&SelectionKind::PathTrack {
            track_id: "t".into()
        }));
        assert!(is_path_selection(&SelectionKind::PathVertex {
            track_id: "t".into(),
            idx: 0
        }));
        assert!(is_path_selection(&SelectionKind::PathSegment {
            track_id: "t".into(),
            idx: 0
        }));
        assert!(!is_path_selection(&SelectionKind::Node(entity(1))));
        assert!(!is_path_selection(&SelectionKind::Empty));
        assert!(!is_path_selection(&SelectionKind::Stamp(entity(1))));
    }
}
