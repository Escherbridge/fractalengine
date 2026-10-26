//! terrain_proposal_delta_viz_20261025 S3: publishes the ARMED tool's draft as
//! a `ProposalPreview` (fe-renderer seam) before any commit (FR-3), and
//! mirrors the resulting `ProposalPreviewReport` for the Terrain tools
//! section's live readout. fe-ui never depends on fe-terrain (NFR-5) — every
//! cross-crate type here is fe-renderer's seam. See `AGENTS.md`
//! §terrain-proposal-editor.

use bevy::prelude::*;

use fe_renderer::terrain_overlay::{PreviewSource, ProposalPreview, ProposalPreviewReport};

use crate::actions::terrain_proposal::{SculptOpKind, SculptShapeMode, SculptToolState};
use crate::geometry::world_to_meters;
use crate::navigation_manager::NavigationManager;
use crate::node_manager::brush_interaction::BrushGesture;
use crate::panels::terrain_tools_panel::{circle_footprint, rect_footprint};
use crate::panels::tool_panel::ToolPanelState;
use crate::panels::toolbar::Tool;
use crate::plugin::ToolState;
use crate::terrain_map::PetalMapState;
use crate::terrain_proposal_state::{ProposalEditState, ProposalOp};
use crate::ui_shell::right_sidebar::{RightSidebarSection, RightSidebarState};

/// World-unit snapshot [`BrushGesture::preview_snapshot`] hands back: the
/// corridor footprint is already the same polygon a release would commit. No
/// `petal_id` here — `handle_brush_interaction` already cancels the gesture
/// the moment the active petal diverges from the stroke's own, so the merge
/// boundary's own (already-current) petal id is authoritative.
/// `target_height_world`/`delta_world` are WORLD units (mirrors
/// `BrushSnapshot`'s own convention) — [`publish_proposal_preview`] converts
/// to real meters at the merge boundary, since `ProposalPreview::target_m`/
/// `delta_m` are meters (fe-terrain's cut/fill math wants real units, NFR-4).
#[derive(Debug, Clone)]
pub(crate) struct BrushPreviewSnapshot {
    pub(crate) footprint: Vec<[f32; 2]>,
    pub(crate) op: String,
    pub(crate) target_height_world: Option<f32>,
    pub(crate) delta_world: Option<f32>,
    pub(crate) strength: f32,
}

/// Latest [`ProposalPreviewReport`] for the Terrain tools section's live
/// readout line (`panels::terrain_tools_panel::render_preview_readout_overlay`).
/// `None` whenever no preview is showing — a reader must never mistake a
/// stale report for a live one.
#[derive(Resource, Default, Clone)]
pub struct PreviewReadout(pub Option<ProposalPreviewReport>);

/// Snake-case op tag for a [`ProposalOp`], matching fe-terrain's
/// `TerrainOp::as_snake` tags exactly (the two enums are independently
/// defined — fe-ui must not depend on fe-terrain — so this mapping is the
/// seam that keeps their tags in sync).
fn proposal_op_snake(op: ProposalOp) -> &'static str {
    match op {
        ProposalOp::Raise => "raise",
        ProposalOp::Lower => "lower",
        ProposalOp::Flatten => "flatten",
        ProposalOp::Ramp => "ramp",
        ProposalOp::Slope => "slope",
        ProposalOp::Pad => "pad",
        ProposalOp::Cut => "cut",
        ProposalOp::Fill => "fill",
    }
}

/// Source (b): a DEFINED sculpt footprint (Circle/Rect/Polygon) armed while
/// Brush is the active viewport tool. The freeform `Brush` shape mode is NOT
/// this source — an in-progress stroke is source (c) below, and an idle
/// Brush mode with no stroke has nothing to preview yet.
fn sculpt_shape_draft(
    sculpt: &SculptToolState,
    tool: &ToolState,
    petal_id: &str,
) -> Option<ProposalPreview> {
    if tool.active_tool != Tool::Brush {
        return None;
    }
    let footprint = match sculpt.shape_mode {
        SculptShapeMode::Brush => return None,
        SculptShapeMode::Circle => circle_footprint(sculpt.radius),
        SculptShapeMode::Rect => rect_footprint(sculpt.radius),
        SculptShapeMode::Polygon => {
            if sculpt.region_draft.len() < 3 {
                return None;
            }
            sculpt.region_draft.clone()
        }
    };
    // `SculptToolState`'s fields are already petal-local METERS (N-1) — no
    // `world_to_meters` conversion here, unlike the palette/brush sources
    // below. Smooth has no target/delta of its own (it relaxes toward the
    // region mean, which only S2's real height-field read can compute); its
    // `strength` alone carries the pull weight.
    let (target_m, delta_m) = match sculpt.op {
        SculptOpKind::Level => (sculpt.target_height, 0.0),
        SculptOpKind::Raise | SculptOpKind::Lower => (0.0, sculpt.delta),
        SculptOpKind::Smooth => (0.0, 0.0),
    };
    Some(ProposalPreview {
        petal_id: petal_id.to_string(),
        op: sculpt.op.to_snake().to_string(),
        footprint,
        target_m,
        delta_m,
        strength: sculpt.strength,
        source: PreviewSource::SculptShape,
    })
}

/// Source (a): the armed palette op/radius/delta — a circle centered at the
/// origin, the SAME "append at origin, reposition later" convention
/// `terrain_tools_panel::render_controls_and_emit` uses when it emits
/// `TerrainProposalAdd`, so a shown preview matches what Add would commit.
fn palette_draft(tool_panel: &ToolPanelState, petal_id: &str, world_scale: f64) -> ProposalPreview {
    let radius = if tool_panel.terrain_footprint_radius.is_finite()
        && tool_panel.terrain_footprint_radius > 0.0
    {
        tool_panel.terrain_footprint_radius
    } else {
        5.0
    };
    ProposalPreview {
        petal_id: petal_id.to_string(),
        op: proposal_op_snake(tool_panel.terrain_tool_mode.to_proposal_op()).to_string(),
        footprint: circle_footprint(radius),
        target_m: world_to_meters(tool_panel.terrain_target_height, world_scale),
        delta_m: world_to_meters(tool_panel.terrain_delta, world_scale),
        strength: 1.0,
        source: PreviewSource::PaletteOp,
    }
}

/// Source (d): the selected committed proposal, for the readout only — shown
/// when nothing is actively armed, so the Proposals list's selection still
/// drives a live cut/fill line instead of going blank.
fn selected_proposal_draft(
    state: &ProposalEditState,
    petal_id: &str,
    world_scale: f64,
) -> Option<ProposalPreview> {
    let id = state.selected.as_deref()?;
    let record = state.proposals.iter().find(|r| r.id == id)?;
    Some(ProposalPreview {
        petal_id: petal_id.to_string(),
        op: proposal_op_snake(record.op).to_string(),
        footprint: record.footprint.clone(),
        target_m: world_to_meters(record.target_height.unwrap_or(0.0), world_scale),
        delta_m: world_to_meters(record.delta.unwrap_or(0.0), world_scale),
        strength: 1.0,
        source: PreviewSource::PaletteOp,
    })
}

/// FR-3: publish the armed tool's draft as `ProposalPreview` before any
/// commit. Precedence, most-live wins: an in-progress brush stroke (c) > a
/// defined sculpt shape (b) > the armed palette op (a) > the selected
/// committed proposal, readout-only (d). With none of those, the resource is
/// REMOVED — "no preview" is the resource's absence, never a zeroed value, so
/// a reader (S2's render system) can't mistake a stale draft for a live one.
///
/// Registered in `PostUpdate` (plugin.rs) so it always observes THIS frame's
/// `handle_brush_interaction`/`process_ui_actions` writes (both run earlier,
/// in `Update`) rather than lagging a frame behind them.
#[allow(clippy::too_many_arguments)]
pub fn publish_proposal_preview(
    mut commands: Commands,
    nav: Res<NavigationManager>,
    right_sidebar: Res<RightSidebarState>,
    tool_state: Res<ToolState>,
    tool_panel: Res<ToolPanelState>,
    sculpt: Res<SculptToolState>,
    proposal_state: Res<ProposalEditState>,
    petal_map: Res<PetalMapState>,
    brush: Res<BrushGesture>,
    mut readout: ResMut<PreviewReadout>,
    mut last_petal: Local<Option<String>>,
    mut last_proposal_count: Local<usize>,
) {
    let active_petal = nav.active_petal_id.clone();

    // Petal switch: the outgoing petal's draft must never bleed into the next
    // one (mirrors `ProposalEditState::reset_for_petal_switch`'s own rule).
    // Reset the commit-detection baseline too, so the new petal's first real
    // frame below isn't mistaken for "a proposal just committed".
    if *last_petal != active_petal {
        *last_petal = active_petal.clone();
        *last_proposal_count = proposal_state.proposals.len();
        commands.remove_resource::<ProposalPreview>();
        readout.0 = None;
        return;
    }

    // `TerrainProposalAdd` leaves the palette's fields armed for the next Add
    // by design (see `render_controls_and_emit`'s doc comment — it never
    // resets `ToolPanelState.terrain_*`), so a grown proposal count is the
    // only signal that a commit just landed. Suppress the palette source for
    // exactly that one frame so the just-committed draft doesn't linger as if
    // still pending.
    let just_committed = proposal_state.proposals.len() > *last_proposal_count;
    *last_proposal_count = proposal_state.proposals.len();

    let Some(petal_id) = active_petal else {
        commands.remove_resource::<ProposalPreview>();
        readout.0 = None;
        return;
    };
    let world_scale = petal_map.world_scale;

    let draft = brush
        .preview_snapshot()
        .map(|snap| ProposalPreview {
            petal_id: petal_id.clone(),
            op: snap.op,
            footprint: snap.footprint,
            target_m: world_to_meters(snap.target_height_world.unwrap_or(0.0), world_scale),
            delta_m: world_to_meters(snap.delta_world.unwrap_or(0.0), world_scale),
            strength: snap.strength,
            source: PreviewSource::BrushStroke,
        })
        .or_else(|| sculpt_shape_draft(&sculpt, &tool_state, &petal_id))
        .or_else(|| {
            (!just_committed && right_sidebar.is_active(RightSidebarSection::TerrainTools))
                .then(|| palette_draft(&tool_panel, &petal_id, world_scale))
        })
        .or_else(|| selected_proposal_draft(&proposal_state, &petal_id, world_scale));

    match draft {
        Some(preview) => {
            commands.insert_resource(preview);
        }
        None => {
            commands.remove_resource::<ProposalPreview>();
            readout.0 = None;
        }
    }
}

/// Mirrors the latest `ProposalPreviewReport` into [`PreviewReadout`],
/// dropping a report for any petal other than the active one — a stale
/// cross-petal report must never surface on the readout line.
pub fn drain_preview_reports(
    mut reports: MessageReader<ProposalPreviewReport>,
    nav: Res<NavigationManager>,
    mut readout: ResMut<PreviewReadout>,
) {
    for report in reports.read() {
        if nav.active_petal_id.as_deref() == Some(report.petal_id.as_str()) {
            readout.0 = Some(report.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_app() -> App {
        let mut app = App::new();
        app.init_resource::<NavigationManager>();
        app.init_resource::<RightSidebarState>();
        app.init_resource::<ToolState>();
        app.init_resource::<ToolPanelState>();
        app.init_resource::<SculptToolState>();
        app.init_resource::<ProposalEditState>();
        app.init_resource::<PetalMapState>();
        app.init_resource::<BrushGesture>();
        app.init_resource::<PreviewReadout>();
        app.add_systems(Update, publish_proposal_preview);
        app
    }

    fn arm_petal(app: &mut App, petal_id: &str) {
        app.world_mut()
            .resource_mut::<NavigationManager>()
            .active_petal_id = Some(petal_id.to_string());
    }

    #[test]
    fn armed_palette_publishes_preview_before_add() {
        let mut app = test_app();
        arm_petal(&mut app, "p1");
        app.world_mut()
            .resource_mut::<RightSidebarState>()
            .reveal(RightSidebarSection::TerrainTools);
        {
            let mut tool_panel = app.world_mut().resource_mut::<ToolPanelState>();
            tool_panel.terrain_footprint_radius = 3.0;
            tool_panel.terrain_target_height = 5.0;
            tool_panel.terrain_delta = 2.0;
        }
        // Frame 1 settles the petal-entry baseline (the same
        // `Local<Option<String>>` "first frame initializes, doesn't act"
        // idiom `navigation_manager`'s own tests use); frame 2 is the one
        // that actually computes and publishes the armed draft.
        app.update();
        app.update();

        let preview = app
            .world()
            .get_resource::<ProposalPreview>()
            .expect("armed palette publishes a preview before any TerrainProposalAdd");
        assert_eq!(preview.op, "raise");
        assert_eq!(preview.source, PreviewSource::PaletteOp);
        assert!(!preview.footprint.is_empty());
        assert_eq!(preview.target_m, world_to_meters(5.0, 1.0));
        assert_eq!(preview.delta_m, world_to_meters(2.0, 1.0));
        // Still only a draft — nothing has been committed.
        assert!(app
            .world()
            .resource::<ProposalEditState>()
            .proposals
            .is_empty());
    }

    #[test]
    fn brush_stroke_in_progress_publishes_corridor_preview() {
        let mut app = test_app();
        arm_petal(&mut app, "p1");
        let samples = vec![[0.0, 0.0], [4.0, 0.0], [4.0, 4.0]];
        *app.world_mut().resource_mut::<BrushGesture>() =
            BrushGesture::test_with_active_stroke("p1", samples.clone(), 1.0);
        app.update();
        app.update();

        let preview = app
            .world()
            .get_resource::<ProposalPreview>()
            .expect("an in-progress stroke publishes a preview");
        assert_eq!(preview.source, PreviewSource::BrushStroke);
        let expected = crate::actions::terrain_proposal::stroke_corridor_footprint(&samples, 1.0)
            .expect("three non-collinear samples form a valid corridor");
        assert_eq!(
            preview.footprint, expected,
            "preview footprint must equal what release would commit"
        );
    }

    #[test]
    fn commit_or_disarm_clears_preview() {
        let mut app = test_app();
        arm_petal(&mut app, "p1");
        app.world_mut()
            .resource_mut::<RightSidebarState>()
            .reveal(RightSidebarSection::TerrainTools);
        app.update();
        app.update();
        assert!(
            app.world().get_resource::<ProposalPreview>().is_some(),
            "armed palette shows a preview"
        );

        // Disarm: close the Terrain tools section (same petal, no settling
        // frame needed — the clear takes effect the next update).
        app.world_mut().resource_mut::<RightSidebarState>().requested = None;
        app.update();
        assert!(
            app.world().get_resource::<ProposalPreview>().is_none(),
            "closing the section disarms the preview"
        );

        // Re-arm, then simulate a commit landing (`TerrainProposalAdd`'s
        // `process_ui_actions` handler growing `ProposalEditState.proposals`
        // — that handler itself is exercised elsewhere, not here).
        app.world_mut()
            .resource_mut::<RightSidebarState>()
            .reveal(RightSidebarSection::TerrainTools);
        app.update();
        assert!(app.world().get_resource::<ProposalPreview>().is_some());
        app.world_mut()
            .resource_mut::<ProposalEditState>()
            .push_new(ProposalOp::Raise, vec![[0.0, 0.0]], Some(0.0), Some(1.0));
        app.update();
        assert!(
            app.world().get_resource::<ProposalPreview>().is_none(),
            "the frame a proposal commits, the stale armed preview is suppressed"
        );
    }

    #[test]
    fn petal_switch_clears_preview() {
        let mut app = test_app();
        arm_petal(&mut app, "p1");
        app.world_mut()
            .resource_mut::<RightSidebarState>()
            .reveal(RightSidebarSection::TerrainTools);
        app.update();
        app.update();
        assert!(app.world().get_resource::<ProposalPreview>().is_some());

        arm_petal(&mut app, "p2");
        app.update();
        assert!(
            app.world().get_resource::<ProposalPreview>().is_none(),
            "switching the active petal clears the old petal's preview on the switch frame"
        );
    }
}
