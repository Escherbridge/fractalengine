//! Object-type-aware left-click dispatch (FR-2): a pure truth table keyed on
//! `(active Tool, SelectionKind, HitTarget)` that says *what* left-click should
//! do. This does NOT replace the first-claim-wins `ClickArbiter` in `router.rs`
//! — it is the shared decision model the consumer systems (and the FR-3 path
//! gimbal drag) read so "more operations on left click" stay object-aware and
//! grow in one place. See `fe-ui/src/node_manager/AGENTS.md` §dispatch.

use bevy::prelude::Entity;

use super::selection::SelectionKind;
use crate::panels::toolbar::Tool;

/// Which bezier handle of a path anchor a pick refers to
/// (pen_curve_tool_20260722 FR-5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandleSide {
    /// The incoming handle (`handle_in`).
    In,
    /// The outgoing handle (`handle_out`).
    Out,
}

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
    /// A bezier handle marker of the edited track's anchor `idx` (FR-5).
    PathHandle { idx: usize, side: HandleSide },
    /// A ribbon segment `idx` of the edited track (`points[idx] → points[idx+1]`).
    PathSegment { idx: usize },
    /// A materialized path-asset stamp entity.
    Stamp(Entity),
    /// An existing proposed terrain-edit overlay.
    TerrainProposal { id: String },
    /// Bare terrain surface. Left-click treats it as empty ground; the
    /// right-click menu offers the same verbs as `Empty` (`dialogs::context_menu`).
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
    /// Drag anchor `idx`'s bezier handle (pen_curve_tool_20260722 FR-5).
    /// Position-free like `MoveVertex` — the consumer computes positions.
    MoveHandle { idx: usize, side: HandleSide },
}

/// The FR-2 truth table: map `(tool, current selection, hit)` to the object-aware
/// [`Operation`]. Pure and total. Ordering is by *hit* first (what you clicked),
/// modulated by tool/selection only where it changes intent:
///
/// - A gimbal-axis hit resolves against the current selection (drag a node /
///   stamp / whole-track via the entity gimbal, or a vertex/segment via FR-3).
/// - A node/vertex/segment/stamp/proposal hit selects that object (the Pen tool
///   keeps placing points instead of selecting a node — placement dominates).
/// - Brush owns its own gesture upstream (it claims the frame in
///   `brush_interaction`, ahead of every consumer of this table), so here it
///   resolves to `None`: it must never select and never deselect.
/// - An empty hit places a point in Pen, else clears the selection.
pub fn resolve_operation(tool: Tool, kind: &SelectionKind, hit: HitTarget) -> Operation {
    match hit {
        HitTarget::GimbalAxis => resolve_gimbal(tool, kind),
        HitTarget::Node(entity) => match tool {
            // Pen intent dominates: an empty-ground append still wins over
            // selecting the node the ray grazed (matches §pen-tool routing).
            Tool::Pen => Operation::PlacePathPoint,
            Tool::Brush => Operation::None,
            _ => Operation::SelectNode(entity),
        },
        // A concrete object hit selects that object regardless of tool; WHEN such
        // a hit can occur is the router's gate, not this table's concern.
        HitTarget::PathVertex { idx } => Operation::SelectVertex { idx },
        // A handle hit drags that handle wherever it's pickable (FR-5) — like
        // vertices, WHEN it's pickable is the claiming system's gate.
        HitTarget::PathHandle { idx, side } => Operation::MoveHandle { idx, side },
        HitTarget::PathSegment { idx } => Operation::SelectSegment { idx },
        HitTarget::Stamp(entity) => Operation::SelectStamp(entity),
        HitTarget::TerrainProposal { id } => Operation::SelectProposal { id },
        // Bare terrain reads as empty ground for left-click (its right-click
        // verb set matches `Empty` too — `dialogs::context_menu`).
        HitTarget::TerrainCell | HitTarget::Empty => match tool {
            Tool::Pen => Operation::PlacePathPoint,
            Tool::Brush => Operation::None,
            _ => Operation::Deselect,
        },
    }
}

/// Gimbal-axis resolution split out for readability. A path vertex/segment is
/// grabbable in EVERY tool (decision 2026-07-19 "grab it wherever it's shown"):
/// its gimbal is always drawn as a Move handle, and a lone vertex/segment only
/// supports Move, so any tool resolves to `MoveVertex`/`MoveSegment` (FR-3).
/// Entity-backed selections (node / stamp / whole track) use the entity gimbal,
/// which stays closed outside the transform tools (`handle_gimbal_interaction`).
fn resolve_gimbal(tool: Tool, kind: &SelectionKind) -> Operation {
    match kind {
        SelectionKind::PathVertex { idx, .. } => Operation::MoveVertex { idx: *idx },
        SelectionKind::PathSegment { idx, .. } => Operation::MoveSegment { idx: *idx },
        SelectionKind::Node(_) | SelectionKind::Stamp(_) | SelectionKind::PathTrack { .. } => {
            if matches!(tool, Tool::Move | Tool::Rotate | Tool::Scale) {
                Operation::BeginGimbalDrag
            } else {
                Operation::None
            }
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
                &SelectionKind::PathTrack {
                    track_id: "t".into()
                },
                HitTarget::GimbalAxis
            ),
            Operation::BeginGimbalDrag
        );
    }

    #[test]
    fn gimbal_on_vertex_moves_vertex_in_every_tool() {
        // "Grab it wherever it's shown": the vertex gimbal is a Move handle in
        // every tool, so a lone vertex moves regardless of the active tool.
        for tool in [
            Tool::Select,
            Tool::Pen,
            Tool::Move,
            Tool::Rotate,
            Tool::Scale,
        ] {
            assert_eq!(
                resolve_operation(tool, &track_vertex(3), HitTarget::GimbalAxis),
                Operation::MoveVertex { idx: 3 },
                "{tool:?}"
            );
        }
    }

    #[test]
    fn gimbal_on_segment_moves_segment_in_every_tool() {
        for tool in [
            Tool::Select,
            Tool::Pen,
            Tool::Move,
            Tool::Rotate,
            Tool::Scale,
        ] {
            assert_eq!(
                resolve_operation(tool, &track_segment(2), HitTarget::GimbalAxis),
                Operation::MoveSegment { idx: 2 },
                "{tool:?}"
            );
        }
    }

    #[test]
    fn entity_gimbal_is_inert_in_select_and_pen() {
        // Entity-backed selections still need a transform tool — the entity
        // gimbal handler stays closed in Select/Pen.
        assert_eq!(
            resolve_operation(
                Tool::Select,
                &SelectionKind::Node(entity(1)),
                HitTarget::GimbalAxis
            ),
            Operation::None
        );
        assert_eq!(
            resolve_operation(
                Tool::Pen,
                &SelectionKind::Stamp(entity(1)),
                HitTarget::GimbalAxis
            ),
            Operation::None
        );
        assert_eq!(
            resolve_operation(
                Tool::Select,
                &SelectionKind::PathTrack {
                    track_id: "t".into()
                },
                HitTarget::GimbalAxis
            ),
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
            resolve_operation(
                Tool::Select,
                &SelectionKind::Empty,
                HitTarget::Node(entity(9))
            ),
            Operation::SelectNode(entity(9))
        );
        assert_eq!(
            resolve_operation(
                Tool::Move,
                &SelectionKind::Empty,
                HitTarget::Node(entity(9))
            ),
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
                resolve_operation(
                    tool,
                    &SelectionKind::Empty,
                    HitTarget::PathVertex { idx: 4 }
                ),
                Operation::SelectVertex { idx: 4 },
                "{tool:?}"
            );
            assert_eq!(
                resolve_operation(
                    tool,
                    &SelectionKind::Empty,
                    HitTarget::PathSegment { idx: 1 }
                ),
                Operation::SelectSegment { idx: 1 },
                "{tool:?}"
            );
        }
    }

    #[test]
    fn handle_hit_moves_handle_in_every_tool() {
        // FR-5: a handle marker drags its handle wherever it's pickable — the
        // table is tool- and selection-independent (gating is the system's).
        for tool in [
            Tool::Select,
            Tool::Pen,
            Tool::Move,
            Tool::Rotate,
            Tool::Scale,
        ] {
            for side in [HandleSide::In, HandleSide::Out] {
                assert_eq!(
                    resolve_operation(
                        tool,
                        &SelectionKind::Empty,
                        HitTarget::PathHandle { idx: 2, side }
                    ),
                    Operation::MoveHandle { idx: 2, side },
                    "{tool:?} {side:?}"
                );
            }
        }
        // Selection-independent: an existing vertex selection doesn't change it.
        assert_eq!(
            resolve_operation(
                Tool::Select,
                &track_vertex(0),
                HitTarget::PathHandle {
                    idx: 5,
                    side: HandleSide::Out
                }
            ),
            Operation::MoveHandle {
                idx: 5,
                side: HandleSide::Out
            }
        );
    }

    #[test]
    fn stamp_and_proposal_hits_select_them() {
        assert_eq!(
            resolve_operation(
                Tool::Select,
                &SelectionKind::Empty,
                HitTarget::Stamp(entity(5))
            ),
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
    fn terrain_cell_hit_reads_as_empty_ground() {
        // The superseded `TerrainCellEdit` verb is gone (D15): bare terrain is
        // just ground. Brush never reaches this table (it claims upstream).
        assert_eq!(
            resolve_operation(
                Tool::Select,
                &SelectionKind::Node(entity(1)),
                HitTarget::TerrainCell
            ),
            Operation::Deselect
        );
        assert_eq!(
            resolve_operation(Tool::Pen, &SelectionKind::Empty, HitTarget::TerrainCell),
            Operation::PlacePathPoint
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

    #[test]
    fn brush_never_selects_and_never_deselects() {
        // Brush owns its gesture upstream; if a frame ever reaches this table
        // with Brush active, the answer must be "do nothing" — NOT the empty
        // click's `Deselect`, which would silently drop the user's selection.
        for hit in [
            HitTarget::Empty,
            HitTarget::TerrainCell,
            HitTarget::Node(entity(9)),
        ] {
            assert_eq!(
                resolve_operation(Tool::Brush, &SelectionKind::Node(entity(1)), hit.clone()),
                Operation::None,
                "{hit:?}"
            );
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
            Operation::MoveHandle {
                idx: 0,
                side: HandleSide::In,
            },
        ];
    }
}
