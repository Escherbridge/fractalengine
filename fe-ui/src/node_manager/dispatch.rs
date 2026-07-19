//! Object-type-aware left-click dispatch (FR-2): a pure truth table keyed on
//! `(active Tool, SelectionKind, HitTarget)` that says *what* left-click should
//! do. This does NOT replace the first-claim-wins `ClickArbiter` in `router.rs`
//! — it is the shared decision model the consumer systems (and the FR-3 path
//! gimbal drag) read so "more operations on left click" stay object-aware and
//! grow in one place. See `fe-ui/src/node_manager/AGENTS.md` §dispatch.

use bevy::prelude::Entity;

use super::selection::SelectionKind;
use crate::panels::toolbar::Tool;

/// What the ray hit in the viewport this frame — the raw pick result, before it
/// is interpreted against the current tool/selection. Consumers build this from
/// their own pick (marker pick, ribbon pick, AABB node pick, gimbal-axis pick),
/// then ask [`resolve_operation`] for the object-aware [`Operation`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HitTarget {
    /// Nothing pickable under the ray (e.g. the bare ground plane).
    Empty,
    /// A scene node entity (glTF / primitive).
    Node(Entity),
    /// An existing path vertex marker — index into the edited track's points.
    PathVertex { idx: usize },
    /// A ribbon segment `idx` of the edited track (`points[idx] → points[idx+1]`).
    PathSegment { idx: usize },
    /// A materialized path-asset stamp entity.
    Stamp(Entity),
    /// An existing proposed terrain-edit overlay.
    TerrainProposal { id: String },
    /// A terrain cell under a terrain-edit brush (FR-5 seam — see AGENTS.md).
    TerrainCell,
    /// A gimbal axis handle (transform tools).
    GimbalAxis,
}

/// The object-aware operation left-click resolves to. The set has deliberate
/// headroom (the "more operations on left click" ask): new verbs slot in here
/// and gain a `resolve_operation` arm without touching the router. Variants that
/// aren't produced by the current table (`PlaceNode`) are reserved seams.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Operation {
    /// No-op this frame (the triple has no meaningful action).
    None,
    /// Clear the current selection (empty click in a non-placing tool).
    Deselect,
    /// Select a scene node entity.
    SelectNode(Entity),
    /// Select a path vertex.
    SelectVertex { idx: usize },
    /// Select a path segment.
    SelectSegment { idx: usize },
    /// Select a path-asset stamp entity.
    SelectStamp(Entity),
    /// Select a terrain-proposal overlay.
    SelectProposal { id: String },
    /// Append a new path point at the hit (Pen tool).
    PlacePathPoint,
    /// Create a new scene node at the hit (reserved headroom seam).
    PlaceNode,
    /// Begin an entity-transform gimbal drag on the current selection
    /// (node / stamp / whole path track).
    BeginGimbalDrag,
    /// Drag the selected path vertex via the gimbal (FR-3, non-entity).
    MoveVertex { idx: usize },
    /// Drag the selected path segment via the gimbal (FR-3, moves both ends).
    MoveSegment { idx: usize },
    /// Emit a proposed terrain-cell edit (FR-5 seam — see AGENTS.md §dispatch).
    TerrainCellEdit,
}

/// Cities-Skylines-inspired terrain-edit brush an [`Operation::TerrainCellEdit`]
/// carries downstream (FR-5). Every brush emits a PROPOSAL overlay, never a
/// destructive terrain write (NFR-1). Reserved for the terrain-cell seam.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerrainBrush {
    Raise,
    Lower,
    Flatten,
    Ramp,
    Slope,
    Pad,
    Cut,
    Fill,
}

/// Payload for a proposed terrain-cell edit (FR-5). Mirrors the fields of the
/// forthcoming `crate::actions::UiAction::TerrainProposalAdd { op, footprint,
/// target_height, delta }` (owned by worker w4b). Kept as a local struct so this
/// crate compiles before that variant lands; the emitting system maps it 1:1.
///
/// TODO(ultrapilot): once `UiAction::TerrainProposalAdd` exists, the terrain-cell
/// consumer pushes it directly instead of returning this struct — see AGENTS.md
/// §dispatch (terrain seam).
#[derive(Debug, Clone, PartialEq)]
pub struct TerrainProposalEdit {
    /// Which brush produced this edit.
    pub op: TerrainBrush,
    /// World-space footprint polygon (ground XZ, Y carried) the edit covers.
    pub footprint: Vec<[f32; 3]>,
    /// Desired Bevy-Y height for level brushes (flatten / pad).
    pub target_height: f32,
    /// Raise / lower amount in world units for relative brushes.
    pub delta: f32,
}

/// Build the FR-5 terrain-proposal payload for a resolved
/// [`Operation::TerrainCellEdit`]. Pure so the object→payload mapping is
/// testable before the `UiAction::TerrainProposalAdd` variant exists.
pub fn terrain_cell_proposal(
    op: TerrainBrush,
    footprint: Vec<[f32; 3]>,
    target_height: f32,
    delta: f32,
) -> TerrainProposalEdit {
    TerrainProposalEdit {
        op,
        footprint,
        target_height,
        delta,
    }
}

/// The FR-2 truth table: map `(tool, current selection, hit)` to the object-aware
/// [`Operation`]. Pure and total. Ordering is by *hit* first (what you clicked),
/// modulated by tool/selection only where it changes intent:
///
/// - A gimbal-axis hit resolves against the current selection (drag a node /
///   stamp / whole-track via the entity gimbal, or a vertex/segment via FR-3).
/// - A node/vertex/segment/stamp/proposal hit selects that object (the Pen tool
///   keeps placing points instead of selecting a node — placement dominates).
/// - A terrain-cell hit proposes an edit (FR-5 seam).
/// - An empty hit places a point in Pen, else clears the selection.
pub fn resolve_operation(tool: Tool, kind: &SelectionKind, hit: HitTarget) -> Operation {
    match hit {
        HitTarget::GimbalAxis => resolve_gimbal(tool, kind),
        HitTarget::Node(entity) => match tool {
            // Pen intent dominates: an empty-ground append still wins over
            // selecting the node the ray grazed (matches §pen-tool routing).
            Tool::Pen => Operation::PlacePathPoint,
            _ => Operation::SelectNode(entity),
        },
        // A concrete object hit selects that object regardless of tool; WHEN such
        // a hit can occur is the router's gate, not this table's concern.
        HitTarget::PathVertex { idx } => Operation::SelectVertex { idx },
        HitTarget::PathSegment { idx } => Operation::SelectSegment { idx },
        HitTarget::Stamp(entity) => Operation::SelectStamp(entity),
        HitTarget::TerrainProposal { id } => Operation::SelectProposal { id },
        HitTarget::TerrainCell => Operation::TerrainCellEdit,
        HitTarget::Empty => match tool {
            Tool::Pen => Operation::PlacePathPoint,
            _ => Operation::Deselect,
        },
    }
}

/// Gimbal-axis resolution split out for readability. Only the transform tools
/// grab the gimbal; Select/Pen draw it as a visual handle but don't drag (the
/// axis-pick gate in `handle_gimbal_interaction` stays closed there). Under Move
/// a path vertex/segment becomes a non-entity `MoveVertex`/`MoveSegment` (FR-3);
/// Rotate/Scale of a single vertex/segment has no meaning yet.
fn resolve_gimbal(tool: Tool, kind: &SelectionKind) -> Operation {
    if !matches!(tool, Tool::Move | Tool::Rotate | Tool::Scale) {
        return Operation::None;
    }
    match kind {
        SelectionKind::PathVertex { idx, .. } if tool == Tool::Move => {
            Operation::MoveVertex { idx: *idx }
        }
        SelectionKind::PathSegment { idx, .. } if tool == Tool::Move => {
            Operation::MoveSegment { idx: *idx }
        }
        SelectionKind::PathVertex { .. } | SelectionKind::PathSegment { .. } => Operation::None,
        SelectionKind::Node(_) | SelectionKind::Stamp(_) | SelectionKind::PathTrack { .. } => {
            Operation::BeginGimbalDrag
        }
        SelectionKind::Empty | SelectionKind::TerrainProposal { .. } => Operation::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entity(n: u64) -> Entity {
        Entity::from_bits(n)
    }

    fn track_vertex(idx: usize) -> SelectionKind {
        SelectionKind::PathVertex {
            track_id: "t".into(),
            idx,
        }
    }

    fn track_segment(idx: usize) -> SelectionKind {
        SelectionKind::PathSegment {
            track_id: "t".into(),
            idx,
        }
    }

    // --- gimbal-axis hits (drag intent) ---

    #[test]
    fn gimbal_on_node_begins_entity_drag_in_transform_tools() {
        for tool in [Tool::Move, Tool::Rotate, Tool::Scale] {
            assert_eq!(
                resolve_operation(tool, &SelectionKind::Node(entity(1)), HitTarget::GimbalAxis),
                Operation::BeginGimbalDrag,
                "{tool:?}"
            );
        }
    }

    #[test]
    fn gimbal_on_stamp_and_track_begin_entity_drag() {
        assert_eq!(
            resolve_operation(
                Tool::Move,
                &SelectionKind::Stamp(entity(2)),
                HitTarget::GimbalAxis
            ),
            Operation::BeginGimbalDrag
        );
        assert_eq!(
            resolve_operation(
                Tool::Rotate,
                &SelectionKind::PathTrack { track_id: "t".into() },
                HitTarget::GimbalAxis
            ),
            Operation::BeginGimbalDrag
        );
    }

    #[test]
    fn gimbal_on_vertex_moves_vertex_only_under_move() {
        assert_eq!(
            resolve_operation(Tool::Move, &track_vertex(3), HitTarget::GimbalAxis),
            Operation::MoveVertex { idx: 3 }
        );
        // Rotate/Scale of a lone vertex is a no-op for now.
        assert_eq!(
            resolve_operation(Tool::Rotate, &track_vertex(3), HitTarget::GimbalAxis),
            Operation::None
        );
        assert_eq!(
            resolve_operation(Tool::Scale, &track_vertex(3), HitTarget::GimbalAxis),
            Operation::None
        );
    }

    #[test]
    fn gimbal_on_segment_moves_segment_only_under_move() {
        assert_eq!(
            resolve_operation(Tool::Move, &track_segment(2), HitTarget::GimbalAxis),
            Operation::MoveSegment { idx: 2 }
        );
        assert_eq!(
            resolve_operation(Tool::Scale, &track_segment(2), HitTarget::GimbalAxis),
            Operation::None
        );
    }

    #[test]
    fn gimbal_is_inert_in_select_and_pen() {
        // Select/Pen show the path gimbal as a visual only — no drag (Pen safety).
        assert_eq!(
            resolve_operation(Tool::Select, &track_vertex(0), HitTarget::GimbalAxis),
            Operation::None
        );
        assert_eq!(
            resolve_operation(Tool::Pen, &track_segment(0), HitTarget::GimbalAxis),
            Operation::None
        );
        assert_eq!(
            resolve_operation(Tool::Select, &SelectionKind::Node(entity(1)), HitTarget::GimbalAxis),
            Operation::None
        );
    }

    #[test]
    fn gimbal_on_empty_or_proposal_is_noop() {
        assert_eq!(
            resolve_operation(Tool::Move, &SelectionKind::Empty, HitTarget::GimbalAxis),
            Operation::None
        );
        assert_eq!(
            resolve_operation(
                Tool::Move,
                &SelectionKind::TerrainProposal { id: "p1".into() },
                HitTarget::GimbalAxis
            ),
            Operation::None
        );
    }

    // --- object hits (select intent) ---

    #[test]
    fn node_hit_selects_node_except_in_pen() {
        assert_eq!(
            resolve_operation(Tool::Select, &SelectionKind::Empty, HitTarget::Node(entity(9))),
            Operation::SelectNode(entity(9))
        );
        assert_eq!(
            resolve_operation(Tool::Move, &SelectionKind::Empty, HitTarget::Node(entity(9))),
            Operation::SelectNode(entity(9))
        );
        // Pen keeps placing points even when the ray grazes a node.
        assert_eq!(
            resolve_operation(Tool::Pen, &SelectionKind::Empty, HitTarget::Node(entity(9))),
            Operation::PlacePathPoint
        );
    }

    #[test]
    fn vertex_and_segment_hits_select_regardless_of_tool() {
        for tool in [Tool::Select, Tool::Pen, Tool::Move] {
            assert_eq!(
                resolve_operation(tool, &SelectionKind::Empty, HitTarget::PathVertex { idx: 4 }),
                Operation::SelectVertex { idx: 4 },
                "{tool:?}"
            );
            assert_eq!(
                resolve_operation(tool, &SelectionKind::Empty, HitTarget::PathSegment { idx: 1 }),
                Operation::SelectSegment { idx: 1 },
                "{tool:?}"
            );
        }
    }

    #[test]
    fn stamp_and_proposal_hits_select_them() {
        assert_eq!(
            resolve_operation(Tool::Select, &SelectionKind::Empty, HitTarget::Stamp(entity(5))),
            Operation::SelectStamp(entity(5))
        );
        assert_eq!(
            resolve_operation(
                Tool::Select,
                &SelectionKind::Empty,
                HitTarget::TerrainProposal { id: "p7".into() }
            ),
            Operation::SelectProposal { id: "p7".into() }
        );
    }

    #[test]
    fn terrain_cell_hit_proposes_an_edit() {
        assert_eq!(
            resolve_operation(Tool::Select, &SelectionKind::Empty, HitTarget::TerrainCell),
            Operation::TerrainCellEdit
        );
    }

    // --- empty hits (place / deselect) ---

    #[test]
    fn empty_hit_places_in_pen_else_deselects() {
        assert_eq!(
            resolve_operation(Tool::Pen, &SelectionKind::Empty, HitTarget::Empty),
            Operation::PlacePathPoint
        );
        for tool in [Tool::Select, Tool::Move, Tool::Rotate, Tool::Scale] {
            assert_eq!(
                resolve_operation(tool, &SelectionKind::Node(entity(1)), HitTarget::Empty),
                Operation::Deselect,
                "{tool:?}"
            );
        }
    }

    // --- terrain proposal payload seam (FR-5) ---

    #[test]
    fn terrain_cell_proposal_carries_all_fields() {
        let footprint = vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 0.0, 1.0]];
        let edit = terrain_cell_proposal(TerrainBrush::Flatten, footprint.clone(), 12.5, -3.0);
        assert_eq!(edit.op, TerrainBrush::Flatten);
        assert_eq!(edit.footprint, footprint);
        assert_eq!(edit.target_height, 12.5);
        assert_eq!(edit.delta, -3.0);
    }

    #[test]
    fn every_brush_is_distinct() {
        let all = [
            TerrainBrush::Raise,
            TerrainBrush::Lower,
            TerrainBrush::Flatten,
            TerrainBrush::Ramp,
            TerrainBrush::Slope,
            TerrainBrush::Pad,
            TerrainBrush::Cut,
            TerrainBrush::Fill,
        ];
        for (i, a) in all.iter().enumerate() {
            for (j, b) in all.iter().enumerate() {
                assert_eq!(i == j, a == b);
            }
        }
    }

    /// Guards against a variant being silently dropped from the public op set —
    /// also keeps the reserved `PlaceNode`/`None` seams constructed.
    #[test]
    fn operation_variants_are_constructible() {
        let _ = [
            Operation::None,
            Operation::Deselect,
            Operation::SelectNode(entity(1)),
            Operation::SelectVertex { idx: 0 },
            Operation::SelectSegment { idx: 0 },
            Operation::SelectStamp(entity(1)),
            Operation::SelectProposal { id: "x".into() },
            Operation::PlacePathPoint,
            Operation::PlaceNode,
            Operation::BeginGimbalDrag,
            Operation::MoveVertex { idx: 0 },
            Operation::MoveSegment { idx: 0 },
            Operation::TerrainCellEdit,
        ];
    }
}
