//! Left-click arbitration for the viewport. See `node_manager/AGENTS.md` §input-router.

use bevy::prelude::*;
use bevy_egui::EguiContexts;

use crate::plugin::ViewportRect;

/// A viewport left-click consumer, ordered highest-priority first.
///
/// Consumers run in this order via the `.chain()` in `mod.rs`; the first to
/// `claim` a frame owns it and lower-priority consumers yield.
#[derive(Copy, Clone, PartialEq, Eq, Debug, PartialOrd, Ord)]
pub(super) enum ClickPriority {
    /// Gimbal axis pick / drag (transform tools).
    Gimbal,
    /// Drag / annotate an existing path-point marker.
    PathMarker,
    /// Pen-tool append of a new path point.
    PathPlace,
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

    #[allow(dead_code)]
    /// Resolved pointer phase for this frame.
    pub(super) fn phase(&self) -> PointerPhase {
        self.phase
    }
}

// ---------------------------------------------------------------------------
// System: resolve the per-frame pointer decision (runs before consumers)
// ---------------------------------------------------------------------------

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

    arbiter.phase = if mouse_button.just_pressed(MouseButton::Left) {
        PointerPhase::Press
    } else if mouse_button.just_released(MouseButton::Left) {
        PointerPhase::Release
    } else if mouse_button.pressed(MouseButton::Left) {
        PointerPhase::Hold
    } else {
        PointerPhase::Hover
    };

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
}
