//! terrain_proposal_delta_viz_20261025 S3: publishes the ARMED tool's draft as
//! a `ProposalPreview` (fe-renderer seam) before any commit (FR-3), and
//! mirrors the resulting `ProposalPreviewReport` for the Terrain tools
//! section's live readout. fe-ui never depends on fe-terrain (NFR-5) — every
//! cross-crate type here is fe-renderer's seam. See `AGENTS.md`
//! §terrain-proposal-editor.

use bevy::prelude::*;

use fe_renderer::terrain_overlay::{PreviewSource, ProposalPreview, ProposalPreviewReport};

use crate::actions::terrain_proposal::{brush_region_fields, SculptShapeMode, SculptToolState};
use crate::geometry::{meters_to_world, world_to_meters};
use crate::navigation_manager::NavigationManager;
use crate::node_manager::brush_interaction::BrushGesture;
use crate::panels::terrain_tools_panel::circle_footprint;
use crate::panels::tool_options::{shape_apply_footprint, shape_region_op_fields};
use crate::panels::tool_panel::ToolPanelState;
use crate::panels::toolbar::Tool;
use crate::plugin::ToolState;
use crate::terrain_map::PetalMapState;
use crate::terrain_proposal_state::{ProposalEditState, ProposalOp};
#[cfg(test)]
use crate::ui_shell::right_sidebar::RightSidebarSection;
use crate::ui_shell::right_sidebar::RightSidebarState;

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

/// Preview `(target, delta, strength)` from a persisted region record's
/// `(target_height, delta)` slots — mirrors fe-terrain's `EarthworkRegion::
/// effective_delta`/`effective_strength` (Level/Smooth keep blend strength in
/// the `delta` slot), so the preview integrates exactly what the commit bakes.
fn region_record_preview_params(
    op: &str,
    target: Option<f32>,
    delta_slot: Option<f32>,
) -> (f32, f32, f32) {
    let target = target.unwrap_or(0.0);
    if matches!(op, "level" | "smooth") {
        (target, 0.0, delta_slot.unwrap_or(1.0).clamp(0.0, 1.0))
    } else {
        (target, delta_slot.unwrap_or(0.0), 1.0)
    }
}

/// The WORLD-unit footprint a shape-mode Apply would commit: the same
/// `shape_apply_footprint` the Apply button submits (petal-local meters),
/// converted exactly as `handle_shape_region` converts it (`meters_to_world`).
pub(crate) fn shape_commit_footprint_world(
    sculpt: &SculptToolState,
    world_scale: f64,
) -> Vec<[f32; 2]> {
    shape_apply_footprint(sculpt)
        .into_iter()
        .map(|[x, z]| {
            [
                meters_to_world(x, world_scale),
                meters_to_world(z, world_scale),
            ]
        })
        .collect()
}

/// Source (b): a DEFINED sculpt footprint (Circle/Rect/Polygon) armed while
/// Brush is the active viewport tool. The freeform `Brush` shape mode is NOT
/// this source — an in-progress stroke is source (c) below, and an idle
/// Brush mode with no stroke has nothing to preview yet. Footprint and params
/// mirror what `SculptShapeRegion` commits: world-unit footprint, meters
/// target/delta (the seam's units), and NO strength (the committed shape
/// region carries none, so it bakes at full strength).
fn sculpt_shape_draft(
    sculpt: &SculptToolState,
    tool: &ToolState,
    petal_id: &str,
    world_scale: f64,
    nav: &NavigationManager,
    map: &PetalMapState,
) -> Option<ProposalPreview> {
    if tool.active_tool != Tool::Brush || matches!(sculpt.shape_mode, SculptShapeMode::Brush) {
        return None;
    }
    if let Some(draft) = tool.terrain.shape_preview(nav, map) {
        let (target_m, delta_m, strength) =
            region_record_preview_params(&draft.op, draft.target_m, draft.delta_m);
        return Some(ProposalPreview {
            petal_id: petal_id.into(),
            op: draft.op,
            footprint: draft.footprint_world,
            target_m,
            delta_m,
            strength,
            source: PreviewSource::SculptShape,
        });
    }
    if !tool.terrain.polygon_matches(nav, map) {
        return None;
    }
    let footprint = shape_commit_footprint_world(sculpt, world_scale);
    if footprint.len() < 3 {
        return None;
    }
    let op = sculpt.op.to_snake();
    let (target, delta_slot) = shape_region_op_fields(sculpt);
    let (target_m, delta_m, strength) = region_record_preview_params(op, target, delta_slot);
    Some(ProposalPreview {
        petal_id: petal_id.to_string(),
        op: op.to_string(),
        footprint,
        target_m,
        delta_m,
        strength,
        source: PreviewSource::SculptShape,
    })
}

/// Source (c): the in-progress brush corridor, with the region-record params
/// the release commit would persist (`brush_region_fields`); `None` when the
/// commit would reject the stroke. World target/delta convert to meters here.
fn brush_stroke_draft(
    brush: &BrushGesture,
    petal_id: &str,
    world_scale: f64,
) -> Option<ProposalPreview> {
    let snap = brush.preview_snapshot()?;
    let (target, delta_slot) = brush_region_fields(
        &snap.op,
        snap.target_height_world,
        snap.delta_world,
        snap.strength,
    )
    .ok()?;
    let (target_world, delta_world, strength) =
        region_record_preview_params(&snap.op, target, delta_slot);
    Some(ProposalPreview {
        petal_id: petal_id.to_string(),
        op: snap.op,
        footprint: snap.footprint,
        target_m: world_to_meters(target_world, world_scale),
        delta_m: world_to_meters(delta_world, world_scale),
        strength,
        source: PreviewSource::BrushStroke,
    })
}

/// Preview the captured proposal at its hovered viewport position.
fn palette_draft(
    tool: &ToolState,
    petal_id: &str,
    world_scale: f64,
    nav: &NavigationManager,
    map: &PetalMapState,
) -> Option<ProposalPreview> {
    let draft = tool.terrain.proposal_preview(nav, map, tool.active_tool)?;
    Some(ProposalPreview {
        petal_id: petal_id.into(),
        op: proposal_op_snake(draft.op).into(),
        footprint: draft.footprint_world,
        target_m: world_to_meters(draft.target_world, world_scale),
        delta_m: world_to_meters(draft.delta_world, world_scale),
        strength: 1.0,
        source: PreviewSource::PaletteOp,
    })
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
        source: PreviewSource::SelectedProposal,
    })
}

/// FR-3: publish the armed tool's draft as `ProposalPreview` before any
/// commit. Precedence, most-live wins: an in-progress brush stroke (c) > a
/// defined sculpt shape (b) > the armed palette op (a) > the selected
/// committed proposal, readout-only (d). With none of those, the resource is
/// REMOVED — "no preview" is the resource's absence, never a zeroed value, so
/// a reader (S2's render system) can't mistake a stale draft for a live one.
/// An unchanged draft is never re-inserted, so the resource only reads as
/// changed when the draft really moved (fe-terrain's NFR-3 idle path).
///
/// Registered in `PostUpdate` (plugin.rs) so it always observes THIS frame's
/// `handle_brush_interaction`/`process_ui_actions` writes (both run earlier,
/// in `Update`) rather than lagging a frame behind them.
#[allow(clippy::too_many_arguments)]
pub(crate) fn publish_proposal_preview(
    mut commands: Commands,
    current: Option<Res<ProposalPreview>>,
    nav: Res<NavigationManager>,
    _right_sidebar: Res<RightSidebarState>,
    tool_state: Res<ToolState>,
    _tool_panel: Res<ToolPanelState>,
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

    // Suppress a consumed proposal preview on its commit frame.
    let just_committed = proposal_state.proposals.len() > *last_proposal_count;
    *last_proposal_count = proposal_state.proposals.len();

    let Some(petal_id) = active_petal else {
        commands.remove_resource::<ProposalPreview>();
        readout.0 = None;
        return;
    };
    let world_scale = petal_map.world_scale;

    let draft = brush_stroke_draft(&brush, &petal_id, world_scale)
        .or_else(|| {
            sculpt_shape_draft(
                &sculpt,
                &tool_state,
                &petal_id,
                world_scale,
                &nav,
                &petal_map,
            )
        })
        .or_else(|| {
            (!just_committed)
                .then(|| palette_draft(&tool_state, &petal_id, world_scale, &nav, &petal_map))
                .flatten()
        })
        .or_else(|| selected_proposal_draft(&proposal_state, &petal_id, world_scale));

    match draft {
        Some(preview) => {
            if current.as_deref() != Some(&preview) {
                commands.insert_resource(preview);
            }
        }
        None => {
            if current.is_some() {
                commands.remove_resource::<ProposalPreview>();
            }
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
        {
            let mut nav = app.world_mut().resource_mut::<NavigationManager>();
            nav.active_verse_id = Some("v".into());
            nav.active_fractal_id = Some("f".into());
            nav.active_petal_id = Some(petal_id.into());
        }
        let mut map = app.world_mut().resource_mut::<PetalMapState>();
        map.petal_id = Some(petal_id.into());
        map.loaded = true;
        map.terrain_json = Some(serde_json::json!({"enabled":true}));
    }

    fn arm_preview(app: &mut App, shape: bool) {
        app.world_mut()
            .resource_scope(|world, mut tool: Mut<ToolState>| {
                let current = world.resource::<NavigationManager>();
                let nav = NavigationManager {
                    active_verse_id: current.active_verse_id.clone(),
                    active_fractal_id: current.active_fractal_id.clone(),
                    active_petal_id: current.active_petal_id.clone(),
                    ..Default::default()
                };
                let map = world.resource::<PetalMapState>().clone();
                let role = crate::plugin::LocalUserRole {
                    role: Some(fe_database::RoleLevel::Editor),
                };
                let active = tool.active_tool;
                if shape {
                    assert!(tool.terrain.arm_shape(
                        &nav,
                        &map,
                        &role,
                        world.resource::<SculptToolState>(),
                        active
                    ));
                } else {
                    let panel = world.resource::<ToolPanelState>();
                    assert!(tool.terrain.arm_proposal(
                        &nav,
                        &map,
                        &role,
                        panel.terrain_tool_mode.to_proposal_op(),
                        if panel.terrain_footprint_radius > 0.0 {
                            panel.terrain_footprint_radius
                        } else {
                            5.0
                        },
                        panel.terrain_target_height,
                        panel.terrain_delta,
                        active
                    ));
                }
                tool.terrain.hover(
                    Some([0.0, 0.0]),
                    &mut world.resource_mut::<SculptToolState>(),
                    &map,
                );
            });
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
        arm_preview(&mut app, false);
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
        // Params too: release persists Raise as `|delta| × strength` (1.0 ×
        // 0.5) at full strength — previewing raw delta 1.0 would double it.
        let (target, delta_slot) = brush_region_fields("raise", None, Some(1.0), 0.5).unwrap();
        assert_eq!((target, delta_slot), (None, Some(0.5)));
        assert_eq!(
            preview.delta_m, 0.5,
            "preview delta = the committed delta slot"
        );
        assert_eq!(preview.strength, 1.0, "Raise records bake at full strength");
    }

    #[test]
    fn sculpt_shape_preview_matches_what_apply_commits_on_a_scaled_petal() {
        // `SculptToolState` is petal-local METERS; `SculptShapeRegion` commits
        // a WORLD footprint (`handle_shape_region`'s meters_to_world) and no
        // strength. The preview must draw/integrate that same region, not a
        // meters-sized circle at full-vs-partial strength.
        let mut app = test_app();
        arm_petal(&mut app, "p1");
        app.world_mut().resource_mut::<PetalMapState>().world_scale = 2.0;
        app.world_mut().resource_mut::<ToolState>().active_tool = Tool::Brush;
        {
            let mut sculpt = app.world_mut().resource_mut::<SculptToolState>();
            sculpt.shape_mode = SculptShapeMode::Circle;
            sculpt.radius = 3.0;
            sculpt.op = crate::actions::terrain_proposal::SculptOpKind::Level;
            sculpt.target_height = 4.0;
            sculpt.strength = 0.5;
        }
        arm_preview(&mut app, true);
        app.update();
        app.update();

        let preview = app
            .world()
            .get_resource::<ProposalPreview>()
            .expect("an armed shape publishes a preview")
            .clone();
        assert_eq!(preview.source, PreviewSource::SculptShape);
        let sculpt = app.world().resource::<SculptToolState>();
        let committed_world: Vec<[f32; 2]> = shape_apply_footprint(sculpt)
            .into_iter()
            .map(|[x, z]| [x * 2.0, z * 2.0])
            .collect();
        assert_eq!(
            preview.footprint, committed_world,
            "world-unit footprint, as committed"
        );
        assert_eq!(preview.target_m, 4.0, "seam target stays meters");
        assert_eq!(
            preview.strength, 1.0,
            "a committed shape region bakes at full strength"
        );
    }

    #[test]
    fn commit_or_disarm_clears_preview() {
        let mut app = test_app();
        arm_petal(&mut app, "p1");
        app.world_mut()
            .resource_mut::<RightSidebarState>()
            .reveal(RightSidebarSection::TerrainTools);
        arm_preview(&mut app, false);
        app.update();
        app.update();
        assert!(
            app.world().get_resource::<ProposalPreview>().is_some(),
            "armed palette shows a preview"
        );

        // Disarm: close the Terrain tools section (same petal, no settling
        // frame needed — the clear takes effect the next update).
        app.world_mut().resource_mut::<ToolState>().terrain.cancel();
        app.update();
        assert!(
            app.world().get_resource::<ProposalPreview>().is_none(),
            "explicit Cancel disarms the preview"
        );

        // Re-arm, then simulate a commit landing (`TerrainProposalAdd`'s
        // `process_ui_actions` handler growing `ProposalEditState.proposals`
        // — that handler itself is exercised elsewhere, not here).
        app.world_mut()
            .resource_mut::<RightSidebarState>()
            .reveal(RightSidebarSection::TerrainTools);
        arm_preview(&mut app, false);
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
        arm_preview(&mut app, false);
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
