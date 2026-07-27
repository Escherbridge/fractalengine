//! Left-click arbitration for the viewport. See `node_manager/AGENTS.md` §input-router.

use bevy::prelude::*;
use bevy_egui::EguiContexts;

use super::dispatch::HitTarget;
use crate::plugin::ViewportRect;

/// A viewport left-click consumer, ordered highest-priority first.
///
/// Consumers run in this order via the `.chain()` in `mod.rs`; the first to
/// `claim` a frame owns it and lower-priority consumers yield.
#[derive(Copy, Clone, PartialEq, Eq, Debug, PartialOrd, Ord)]
pub(super) enum ClickPriority {
    /// Drag a bezier handle marker of the edited track (pen_curve_tool_20260722
    /// FR-5, ratified Q7): outranks the gimbal arm AND the vertex marker, so an
    /// overlapping handle + vertex resolves to the handle.
    PathHandle,
    /// Gimbal axis pick / drag (transform tools).
    Gimbal,
    /// Drag / annotate an existing path-point marker.
    PathMarker,
    /// Pen-tool append of a new path point.
    PathPlace,
    /// Select a ribbon SEGMENT of the edited track (path_interaction_20260716,
    /// FR-3). Outranks `NodePick` so while editing a click on the ribbon selects
    /// the segment instead of re-picking the whole track as a node.
    PathSegment,
    /// First-class terrain brush; outranks node selection.
    Brush,
    /// glTF / node selection (lowest priority).
    NodePick,
}

/// Pointer lifecycle phase resolved once per frame from the mouse button.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub(super) enum PointerPhase {
    /// Left button just went down this frame (fresh press).
    Press,
    /// Left button held down (drag hold).
    Hold,
    /// Left button just released this frame.
    Release,
    /// No button activity — cursor is merely hovering.
    #[default]
    Hover,
}

/// Per-frame left-click decision shared by the consumer systems.
///
/// `resolve_pointer_frame` recomputes this before consumers run each frame:
/// egui/viewport gating (`available`), the resolved `cursor`/`ray`, the pointer
/// `phase`, and the claimed `owner` (reset to `None` per frame).
#[derive(Resource, Default)]
pub(super) struct ClickArbiter {
    /// `true` when the left-click is up for grabs this frame (passed egui +
    /// viewport gating). Consumers must not claim when this is `false`.
    available: bool,
    /// Cursor position in physical window pixels, when inside the viewport.
    cursor: Option<Vec2>,
    /// Camera ray through `cursor`, computed once per frame.
    ray: Option<Ray3d>,
    /// Resolved pointer phase for this frame.
    phase: PointerPhase,
    /// The consumer that has claimed this frame's click, if any.
    owner: Option<ClickPriority>,
}

impl ClickArbiter {
    /// Claim this frame's click for `who`. Succeeds only if unclaimed and the
    /// click is available; since consumers run highest-priority-first, the
    /// first claim wins.
    pub(super) fn claim(&mut self, who: ClickPriority) -> bool {
        if !self.available || self.owner.is_some() {
            return false;
        }
        self.owner = Some(who);
        true
    }

    /// Whether the left-click is available (passed egui + viewport gating).
    pub(super) fn is_available(&self) -> bool {
        self.available
    }

    /// Whether any consumer has already claimed this frame's click. Lets a
    /// consumer distinguish "nobody wanted it" (a genuine empty click) from
    /// "someone above me took it" without knowing which priority (FR-3 uses
    /// this to clear the path selection only on a true empty click).
    pub(super) fn is_claimed(&self) -> bool {
        self.owner.is_some()
    }

    /// Camera ray through the cursor for this frame.
    pub(super) fn ray(&self) -> Option<Ray3d> {
        self.ray
    }

    /// Whether this frame's phase is a fresh left-press.
    pub(super) fn is_fresh_press(&self) -> bool {
        self.phase == PointerPhase::Press
    }

    // FR-1 API surface for the next router consumer (`glb_mesh_picking_20260713`)
    // and the unit tests; not yet read by an in-tree consumer system.
    #[allow(dead_code)]
    /// Whether `who` owns this frame's click.
    pub(super) fn is_owner(&self, who: ClickPriority) -> bool {
        self.owner == Some(who)
    }

    #[allow(dead_code)]
    /// Resolved cursor position (physical pixels), when inside the viewport.
    pub(super) fn cursor(&self) -> Option<Vec2> {
        self.cursor
    }

    /// Resolved pointer phase for this frame (consumed by the pen gesture's
    /// Press/Hold/Release machine — pen_curve_tool_20260722 FR-4).
    pub(super) fn phase(&self) -> PointerPhase {
        self.phase
    }
}

// ---------------------------------------------------------------------------
// Claim-priority table (FR-3): the object-level pick preference over HitTarget
// ---------------------------------------------------------------------------

/// Effective claim priority of a resolved [`HitTarget`] — LOWER rank wins.
/// Pure + total over every `HitTarget` variant: `handle > vertex > segment >
/// gimbal-axis > node > stamp > proposal > terrain-cell > empty`. This is the
/// object-level pick preference (which target owns a click that could resolve
/// to several), distinct from the per-system [`ClickPriority`] chain that
/// enforces it at runtime. See `node_manager/AGENTS.md` §pointer-manager.
#[allow(dead_code)] // FR-3 claim-priority surface; asserted by tests, no in-tree consumer yet.
pub(crate) fn hit_target_rank(hit: &HitTarget) -> u8 {
    match hit {
        HitTarget::PathHandle { .. } => 0,
        HitTarget::PathVertex { .. } => 1,
        HitTarget::PathSegment { .. } => 2,
        HitTarget::GimbalAxis => 3,
        HitTarget::Node(_) => 4,
        HitTarget::Stamp(_) => 5,
        HitTarget::TerrainProposal { .. } => 6,
        HitTarget::TerrainCell => 7,
        HitTarget::Empty => 8,
    }
}

// ---------------------------------------------------------------------------
// System: resolve the per-frame pointer decision (runs before consumers)
// ---------------------------------------------------------------------------

/// Pure per-frame phase resolution. A press+release coalesced into ONE frame
/// resolves as Release (drop-the-click) — see AGENTS.md §input-router.
fn resolve_phase(just_pressed: bool, just_released: bool, held: bool) -> PointerPhase {
    if just_released {
        PointerPhase::Release
    } else if just_pressed {
        PointerPhase::Press
    } else if held {
        PointerPhase::Hold
    } else {
        PointerPhase::Hover
    }
}

/// Stranded-gesture sweep: a Hover frame means the button is up, so any
/// surviving drag missed its Release — drop it (never replay it later).
pub(super) fn sweep_stranded_drag<T>(phase: PointerPhase, active: &mut Option<T>) {
    if phase == PointerPhase::Hover {
        *active = None;
    }
}

/// First system in the selection chain: recomputes [`ClickArbiter`] for the
/// frame. Centralizes egui pointer-capture + `ViewportRect` gating so consumer
/// systems no longer hold `EguiContexts` or re-derive the cursor/ray.
pub(super) fn resolve_pointer_frame(
    mouse_button: Res<ButtonInput<MouseButton>>,
    windows: Query<&Window>,
    cameras: Query<(&Camera, &GlobalTransform), With<fe_renderer::camera::OrbitCameraController>>,
    viewport_rect: Res<ViewportRect>,
    mut egui_ctx: EguiContexts,
    mut arbiter: ResMut<ClickArbiter>,
) {
    // Fresh frame: no owner yet.
    arbiter.owner = None;

    arbiter.phase = resolve_phase(
        mouse_button.just_pressed(MouseButton::Left),
        mouse_button.just_released(MouseButton::Left),
        mouse_button.pressed(MouseButton::Left),
    );

    let egui_using = egui_ctx
        .ctx_mut()
        .map(|ctx| ctx.is_using_pointer())
        .unwrap_or(false);

    let cursor = windows.single().ok().and_then(|w| w.cursor_position());
    let in_viewport = cursor
        .map(|c| viewport_rect.0.contains(bevy_egui::egui::pos2(c.x, c.y)))
        .unwrap_or(false);

    // Click is up for grabs only when egui isn't capturing and the cursor is
    // over the 3-D viewport.
    arbiter.available = !egui_using && in_viewport;

    arbiter.cursor = if in_viewport { cursor } else { None };
    arbiter.ray = match (arbiter.cursor, cameras.single().ok()) {
        (Some(cursor), Some((camera, cam_tx))) => camera.viewport_to_world(cam_tx, cursor).ok(),
        _ => None,
    };
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh arbiter with the click available (as `resolve_pointer_frame`
    /// would leave it inside the viewport with no egui capture).
    fn available_arbiter() -> ClickArbiter {
        ClickArbiter {
            available: true,
            phase: PointerPhase::Press,
            ..Default::default()
        }
    }

    #[test]
    fn hit_target_rank_covers_every_variant_exactly_once_in_priority_order() {
        use super::super::dispatch::HandleSide;
        // One value per HitTarget variant. The exhaustive match in
        // `hit_target_rank` (no wildcard) forces a rank for any new variant;
        // this array must then gain it too, or the permutation assert trips.
        let all = [
            HitTarget::PathHandle {
                idx: 0,
                side: HandleSide::In,
            },
            HitTarget::PathVertex { idx: 0 },
            HitTarget::PathSegment { idx: 0 },
            HitTarget::GimbalAxis,
            HitTarget::Node(Entity::from_bits(1)),
            HitTarget::Stamp(Entity::from_bits(2)),
            HitTarget::TerrainProposal {
                id: "p".to_string(),
            },
            HitTarget::TerrainCell,
            HitTarget::Empty,
        ];
        // Ranks are a permutation of 0..N — each variant a distinct rank, no
        // ties, no gaps — asserted independently of any enum/derive ordering.
        let mut ranks: Vec<u8> = all.iter().map(hit_target_rank).collect();
        ranks.sort_unstable();
        assert_eq!(ranks, (0..all.len() as u8).collect::<Vec<_>>());

        // The ratified backbone holds: handle > vertex > segment > gimbal-axis
        // > node-pick > empty (lower rank = higher priority).
        let backbone = [
            HitTarget::PathHandle {
                idx: 9,
                side: HandleSide::Out,
            },
            HitTarget::PathVertex { idx: 9 },
            HitTarget::PathSegment { idx: 9 },
            HitTarget::GimbalAxis,
            HitTarget::Node(Entity::from_bits(3)),
            HitTarget::Empty,
        ];
        for pair in backbone.windows(2) {
            assert!(
                hit_target_rank(&pair[0]) < hit_target_rank(&pair[1]),
                "{:?} must outrank {:?}",
                pair[0],
                pair[1]
            );
        }
    }

    #[test]
    fn first_claim_wins() {
        let mut arb = available_arbiter();
        assert!(arb.claim(ClickPriority::Gimbal));
        assert!(!arb.claim(ClickPriority::NodePick));
        assert!(arb.is_owner(ClickPriority::Gimbal));
        assert!(!arb.is_owner(ClickPriority::NodePick));
    }

    #[test]
    fn higher_priority_claims_before_lower_in_chain_order() {
        // Consumers run Gimbal → PathMarker → PathPlace → NodePick. Simulate
        // that order: Gimbal claims first, so NodePick is denied.
        let mut arb = available_arbiter();
        for who in [
            ClickPriority::Gimbal,
            ClickPriority::PathMarker,
            ClickPriority::PathPlace,
            ClickPriority::NodePick,
        ] {
            let claimed = arb.claim(who);
            assert_eq!(claimed, who == ClickPriority::Gimbal);
        }
        assert!(arb.is_owner(ClickPriority::Gimbal));
    }

    #[test]
    fn handle_claim_outranks_vertex_and_gimbal_in_chain_order() {
        // pen_curve_tool_20260722 Q7: the handle system runs FIRST in the
        // chain, so its claim wins over the gimbal arm and the vertex marker.
        let mut arb = available_arbiter();
        for who in [
            ClickPriority::PathHandle,
            ClickPriority::Gimbal,
            ClickPriority::PathMarker,
            ClickPriority::PathPlace,
            ClickPriority::NodePick,
        ] {
            let claimed = arb.claim(who);
            assert_eq!(claimed, who == ClickPriority::PathHandle, "{who:?}");
        }
        assert!(arb.is_owner(ClickPriority::PathHandle));
    }

    #[test]
    fn node_pick_wins_when_higher_priorities_dont_claim() {
        // Select-mode empty click: gimbal + path-point decline to claim, so the
        // lowest-priority NodePick gets the frame (reproduces the ab9c53c fix).
        let mut arb = available_arbiter();
        assert!(arb.claim(ClickPriority::NodePick));
        assert!(arb.is_owner(ClickPriority::NodePick));
    }

    #[test]
    fn claim_fails_when_unavailable() {
        let mut arb = ClickArbiter {
            available: false,
            phase: PointerPhase::Press,
            ..Default::default()
        };
        assert!(!arb.claim(ClickPriority::Gimbal));
        assert!(arb.owner.is_none());
    }

    #[test]
    fn owner_reset_clears_claim() {
        let mut arb = available_arbiter();
        assert!(arb.claim(ClickPriority::Gimbal));
        // A new frame resets the owner (mirrors resolve_pointer_frame).
        arb.owner = None;
        assert!(!arb.is_owner(ClickPriority::Gimbal));
        assert!(arb.claim(ClickPriority::NodePick));
        assert!(arb.is_owner(ClickPriority::NodePick));
    }

    #[test]
    fn is_fresh_press_reflects_phase() {
        let arb = available_arbiter();
        assert!(arb.is_fresh_press());
        let hover = ClickArbiter::default();
        assert!(!hover.is_fresh_press());
    }

    #[test]
    fn default_phase_is_hover() {
        assert_eq!(ClickArbiter::default().phase(), PointerPhase::Hover);
    }

    #[test]
    fn same_frame_press_and_release_resolves_as_release() {
        // A click coalesced into one frame must not report Press — a Press
        // with no later Release strands the pen/handle drag machines. Release
        // wins (drop-the-click), whatever the held bit says.
        assert_eq!(resolve_phase(true, true, false), PointerPhase::Release);
        assert_eq!(resolve_phase(true, true, true), PointerPhase::Release);
    }

    #[test]
    fn resolve_phase_maps_the_plain_cases() {
        assert_eq!(resolve_phase(true, false, true), PointerPhase::Press);
        assert_eq!(resolve_phase(false, true, false), PointerPhase::Release);
        assert_eq!(resolve_phase(false, false, true), PointerPhase::Hold);
        assert_eq!(resolve_phase(false, false, false), PointerPhase::Hover);
    }

    #[test]
    fn hover_sweeps_a_stranded_drag_other_phases_keep_it() {
        let mut active = Some(42);
        sweep_stranded_drag(PointerPhase::Press, &mut active);
        sweep_stranded_drag(PointerPhase::Hold, &mut active);
        sweep_stranded_drag(PointerPhase::Release, &mut active);
        assert_eq!(active, Some(42), "live gesture phases never sweep");
        sweep_stranded_drag(PointerPhase::Hover, &mut active);
        assert_eq!(active, None, "button-up hover clears the stranded drag");
    }

    #[test]
    fn phase_reports_hold_and_release_and_neither_is_fresh_press() {
        // FR-4: the pen gesture keys its Hold/Release branches on phase().
        let hold = ClickArbiter {
            phase: PointerPhase::Hold,
            ..Default::default()
        };
        assert_eq!(hold.phase(), PointerPhase::Hold);
        assert!(!hold.is_fresh_press());
        let release = ClickArbiter {
            phase: PointerPhase::Release,
            ..Default::default()
        };
        assert_eq!(release.phase(), PointerPhase::Release);
        assert!(!release.is_fresh_press());
    }
}
