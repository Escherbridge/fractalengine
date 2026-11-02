//! First-class viewport brush gesture. Escape is handled centrally by the
//! staged ladder in `shortcuts.rs`, not here; see `AGENTS.md` §brush-tool.

use bevy::prelude::*;

use super::router::{ClickArbiter, ClickPriority, PointerPhase};
use crate::actions::terrain_proposal::{
    petal_map_enabled, stroke_corridor_footprint, SculptOpKind, SculptToolState,
    MAX_BRUSH_DABS_PER_STROKE,
};
use crate::actions::{UiAction, UiManager};
use crate::geometry::meters_to_world;
use crate::navigation_manager::NavigationManager;
use crate::panels::toolbar::Tool;
use crate::plugin::ToolState;
use crate::terrain_map::PetalMapState;

const DAB_SPACING_RADIUS_FACTOR: f32 = 0.5;

#[derive(Debug, Clone)]
struct BrushSnapshot {
    radius: f32,
    strength: f32,
    op: String,
    target_height: Option<f32>,
    delta: Option<f32>,
    material: String,
}

impl BrushSnapshot {
    fn from_state(state: &SculptToolState, world_scale: f64) -> Self {
        let (target_height, delta) = match state.op {
            SculptOpKind::Level => (
                Some(meters_to_world(state.target_height, world_scale)),
                None,
            ),
            SculptOpKind::Raise | SculptOpKind::Lower => {
                (None, Some(meters_to_world(state.delta, world_scale)))
            }
            SculptOpKind::Smooth => (None, None),
        };
        Self {
            radius: meters_to_world(state.radius, world_scale),
            strength: state.strength,
            op: state.op.to_snake().to_string(),
            target_height,
            delta,
            material: if state.material.trim().is_empty() {
                "earth".to_string()
            } else {
                state.material.trim().to_string()
            },
        }
    }
}

#[derive(Debug)]
struct ActiveStroke {
    petal_id: String,
    samples: Vec<[f32; 2]>,
    settings: BrushSnapshot,
}

// `pub(crate)`, not `pub(super)`: terrain_proposal_delta_viz S3's live-preview
// producer (`terrain_preview::publish_proposal_preview`) reads this resource
// through the `preview_snapshot` accessor below. `active` itself stays
// private — the accessor is the only crate-visible window onto it.
#[derive(Resource, Default)]
pub(crate) struct BrushGesture {
    active: Option<ActiveStroke>,
    scope: Option<super::terrain_authoring::TerrainScope>,
}

impl BrushGesture {
    /// `true` while a stroke is being sampled. Read by `GestureParams`.
    pub(super) fn is_active(&self) -> bool {
        self.active.is_some()
    }

    /// Drop the in-flight stroke without emitting its dabs (staged-Escape rung 0).
    pub(super) fn cancel(&mut self) {
        self.active = None;
        self.scope = None;
    }

    /// Read-only snapshot of the in-progress stroke for the live preview
    /// producer (terrain_proposal_delta_viz S3): the corridor footprint comes
    /// from the SAME `stroke_corridor_footprint` fn the release commit uses
    /// (`stroke_action` above), so a shown preview always matches what
    /// release would commit (plan.md §5 tripwire). Values are WORLD units —
    /// `terrain_preview::publish_proposal_preview` converts to real meters at
    /// the merge boundary. `None` while no stroke is active, or while the
    /// in-progress centerline can't yet form a valid corridor (e.g. a single
    /// degenerate sample) — same "nothing to preview yet" gate `stroke_action`
    /// uses via `stroke_corridor_footprint`'s own `None` case.
    pub(crate) fn preview_snapshot(&self) -> Option<crate::terrain_preview::BrushPreviewSnapshot> {
        let stroke = self.active.as_ref()?;
        let footprint = stroke_corridor_footprint(&stroke.samples, stroke.settings.radius)?;
        Some(crate::terrain_preview::BrushPreviewSnapshot {
            footprint,
            op: stroke.settings.op.clone(),
            target_height_world: stroke.settings.target_height,
            delta_world: stroke.settings.delta,
            strength: stroke.settings.strength,
        })
    }
}

/// Ray hit on the y = 0 ground plane as `[x, z]` (shared with the Measure tool).
pub(super) fn ground_hit(ray: Ray3d) -> Option<[f32; 2]> {
    let direction = *ray.direction;
    if direction.y.abs() <= f32::EPSILON {
        return None;
    }
    let t = -ray.origin.y / direction.y;
    if !(t.is_finite() && t >= 0.0) {
        return None;
    }
    let hit = ray.origin + direction * t;
    (hit.x.is_finite() && hit.z.is_finite()).then_some([hit.x, hit.z])
}

fn append_spaced_samples(samples: &mut Vec<[f32; 2]>, point: [f32; 2], spacing: f32) {
    if samples.is_empty() {
        samples.push(point);
        return;
    }
    if !(spacing.is_finite() && spacing > 0.0) {
        return;
    }
    while samples.len() < MAX_BRUSH_DABS_PER_STROKE {
        let last = *samples.last().expect("checked non-empty");
        let dx = point[0] - last[0];
        let dz = point[1] - last[1];
        let distance = dx.hypot(dz);
        if !(distance.is_finite() && distance >= spacing) {
            break;
        }
        samples.push([
            last[0] + dx / distance * spacing,
            last[1] + dz / distance * spacing,
        ]);
    }
}

fn stroke_action(stroke: ActiveStroke) -> Option<UiAction> {
    (!stroke.samples.is_empty()).then_some(UiAction::SculptBrushStroke {
        petal_id: stroke.petal_id,
        centers: stroke.samples,
        radius: stroke.settings.radius,
        strength: stroke.settings.strength,
        op: stroke.settings.op,
        target_height: stroke.settings.target_height,
        delta: stroke.settings.delta,
        material: stroke.settings.material,
    })
}

fn petal_map_is_loading(petal_map: &PetalMapState, petal_id: &str) -> bool {
    petal_map.petal_id.as_deref() != Some(petal_id) || !petal_map.loaded
}

#[allow(clippy::too_many_arguments)]
pub(super) fn handle_brush_interaction(
    mouse: Res<ButtonInput<MouseButton>>,
    mut tool: ResMut<ToolState>,
    nav: Res<NavigationManager>,
    petal_map: Res<PetalMapState>,
    time: Res<crate::notice::NoticeClock>,
    mut sculpt: ResMut<SculptToolState>,
    mut gesture: ResMut<BrushGesture>,
    mut ui_mgr: ResMut<UiManager>,
    mut arbiter: ResMut<ClickArbiter>,
    role: Res<crate::plugin::LocalUserRole>,
) {
    // Escape is NO LONGER read here: it is the central staged ladder's rung 0
    // (`shortcuts::handle_tool_shortcuts`, which runs first in the chain and
    // clears this resource through `GestureParams`). What stays is the
    // gesture-shaped cancel set: right-click (D5's cancel half), a tool change
    // away from Brush, and the petal-change guard below.
    let active_tool = tool.active_tool;
    tool.terrain
        .synchronize(&nav, &petal_map, &role, active_tool, &mut sculpt);
    let brush_active = active_tool == Tool::Brush;
    if mouse.just_pressed(MouseButton::Right) {
        gesture.cancel();
        tool.terrain.cancel();
        sculpt.region_draft.clear();
        return;
    }

    let hit = arbiter.ray().and_then(ground_hit);
    tool.terrain.hover(hit, &mut sculpt, &petal_map);
    if let Some(commit) = tool.terrain.finish_polygon(&sculpt) {
        ui_mgr.push_action(UiAction::CommitTerrainAuthoring(Box::new(commit)));
        return;
    }
    if arbiter.is_fresh_press()
        && (tool.terrain.is_active()
            || (brush_active
                && sculpt.shape_mode == crate::actions::terrain_proposal::SculptShapeMode::Polygon))
    {
        let Some(point) = hit else { return };
        if !arbiter.claim(ClickPriority::Brush) {
            return;
        }
        if let Some(commit) = tool.terrain.place(
            point,
            &mut sculpt,
            &nav,
            &petal_map,
            &role,
            active_tool,
            time.elapsed_secs_f64(),
        ) {
            ui_mgr.push_action(UiAction::CommitTerrainAuthoring(Box::new(commit)));
        }
        return;
    }
    if !brush_active
        || sculpt.shape_mode != crate::actions::terrain_proposal::SculptShapeMode::Brush
    {
        gesture.cancel();
        return;
    }
    if !role.can_edit()
        || gesture
            .scope
            .as_ref()
            .is_some_and(|scope| !scope.matches(&nav, &petal_map))
    {
        gesture.cancel();
        return;
    }

    if gesture
        .active
        .as_ref()
        .is_some_and(|stroke| nav.active_petal_id.as_deref() != Some(stroke.petal_id.as_str()))
    {
        gesture.cancel();
        return;
    }

    if arbiter.phase() == PointerPhase::Hover {
        gesture.cancel();
        return;
    }

    match arbiter.phase() {
        PointerPhase::Press => {
            let Some(center) = hit else { return };
            if !arbiter.claim(ClickPriority::Brush) {
                return;
            }
            let Some(petal_id) = nav.active_petal_id.as_deref() else {
                bevy::log::warn!("Brush ignored — no active petal");
                ui_mgr.show_toast(
                    "Navigate to a petal before using Brush",
                    time.elapsed_secs_f64(),
                );
                return;
            };
            if petal_map_is_loading(&petal_map, petal_id) {
                bevy::log::warn!("Brush ignored: active petal map is still loading");
                ui_mgr.show_toast(
                    "The petal map is still loading; try Brush again in a moment",
                    time.elapsed_secs_f64(),
                );
                return;
            }
            if !petal_map_enabled(&petal_map, petal_id) {
                bevy::log::warn!("Brush ignored — active petal has no enabled map");
                ui_mgr.show_toast(
                    "Assign an enabled map to this petal before using Brush",
                    time.elapsed_secs_f64(),
                );
                return;
            }
            sculpt.sanitize_numeric_state();
            let Some(scope) =
                super::terrain_authoring::TerrainScope::capture(&nav, &petal_map, &role)
            else {
                return;
            };
            gesture.scope = Some(scope);
            gesture.active = Some(ActiveStroke {
                petal_id: petal_id.to_string(),
                samples: vec![center],
                settings: BrushSnapshot::from_state(&sculpt, petal_map.world_scale),
            });
        }
        PointerPhase::Hold => {
            if let (Some(stroke), Some(center)) = (gesture.active.as_mut(), hit) {
                append_spaced_samples(
                    &mut stroke.samples,
                    center,
                    stroke.settings.radius * DAB_SPACING_RADIUS_FACTOR,
                );
            }
        }
        PointerPhase::Release => {
            let Some(mut stroke) = gesture.active.take() else {
                return;
            };
            if let Some(center) = hit {
                append_spaced_samples(
                    &mut stroke.samples,
                    center,
                    stroke.settings.radius * DAB_SPACING_RADIUS_FACTOR,
                );
            }
            if let (Some(action), Some(scope)) = (stroke_action(stroke), gesture.scope.take()) {
                ui_mgr.push_action(UiAction::CommitTerrainAuthoring(Box::new(
                    super::TerrainAuthoringCommit::new(scope, action),
                )));
            }
        }
        PointerPhase::Hover => {}
    }
}

#[cfg(test)]
impl BrushGesture {
    /// Test-only constructor: an active stroke with the given petal/samples/
    /// radius. Lets `terrain_preview`'s cross-module flow tests exercise
    /// [`BrushGesture::preview_snapshot`] without reaching into the real
    /// pointer/arbiter pipeline — `active` has no production setter by
    /// design (encapsulated; only the accessor crosses the module boundary).
    pub(crate) fn test_with_active_stroke(
        petal_id: &str,
        samples: Vec<[f32; 2]>,
        radius: f32,
    ) -> Self {
        Self {
            scope: None,
            active: Some(ActiveStroke {
                petal_id: petal_id.to_string(),
                samples,
                settings: BrushSnapshot {
                    radius,
                    strength: 0.5,
                    op: "raise".to_string(),
                    target_height: None,
                    delta: Some(1.0),
                    material: "earth".to_string(),
                },
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(op: &str) -> BrushSnapshot {
        BrushSnapshot {
            radius: 2.0,
            strength: 0.25,
            op: op.to_string(),
            target_height: Some(7.0),
            delta: Some(3.0),
            material: "earth".to_string(),
        }
    }

    #[test]
    fn sampling_is_distance_based_not_frame_based() {
        let mut samples = vec![[0.0, 0.0]];
        append_spaced_samples(&mut samples, [0.4, 0.0], 1.0);
        append_spaced_samples(&mut samples, [3.2, 0.0], 1.0);
        assert_eq!(
            samples,
            vec![[0.0, 0.0], [1.0, 0.0], [2.0, 0.0], [3.0, 0.0]]
        );
    }

    #[test]
    fn release_payload_batches_all_samples_once() {
        let action = stroke_action(ActiveStroke {
            petal_id: "petal-1".to_string(),
            samples: vec![[1.0, 2.0], [2.0, 2.0]],
            settings: snapshot("level"),
        })
        .expect("non-empty stroke");
        match action {
            UiAction::SculptBrushStroke {
                petal_id,
                centers,
                radius,
                strength,
                target_height,
                ..
            } => {
                assert_eq!(petal_id, "petal-1");
                assert_eq!(centers, vec![[1.0, 2.0], [2.0, 2.0]]);
                assert_eq!(radius, 2.0);
                assert_eq!(strength, 0.25);
                assert_eq!(target_height, Some(7.0));
            }
            other => panic!("unexpected brush payload: {other:?}"),
        }
    }

    #[test]
    fn snapshot_converts_meter_controls_to_world_units() {
        let state = SculptToolState {
            radius: 2.0,
            target_height: 7.0,
            delta: 3.0,
            op: SculptOpKind::Level,
            ..Default::default()
        };
        let level = BrushSnapshot::from_state(&state, 0.001);
        assert!((level.radius - 0.002).abs() < 1e-7);
        assert!((level.target_height.unwrap() - 0.007).abs() < 1e-7);

        let raise = BrushSnapshot::from_state(
            &SculptToolState {
                op: SculptOpKind::Raise,
                ..state
            },
            0.001,
        );
        assert!((raise.delta.unwrap() - 0.003).abs() < 1e-7);
    }

    fn live_gesture() -> BrushGesture {
        BrushGesture {
            scope: None,
            active: Some(ActiveStroke {
                petal_id: "p".to_string(),
                samples: vec![[1.0, 1.0]],
                settings: snapshot("raise"),
            }),
        }
    }

    #[test]
    fn canceled_gesture_drops_samples_without_actions() {
        // `cancel` never routes through `stroke_action`, so the sampled dabs
        // are dropped rather than committed.
        let mut gesture = live_gesture();
        gesture.cancel();
        assert!(gesture.active.is_none());
    }

    #[test]
    fn is_active_tracks_the_stroke_for_the_gesture_aggregate() {
        // The bit `GestureParams::any_active` (staged-Escape rung 0 + the one
        // right-click rule) reads.
        let mut gesture = live_gesture();
        assert!(gesture.is_active());
        gesture.cancel();
        assert!(!gesture.is_active());
        assert!(!BrushGesture::default().is_active());
    }

    #[test]
    fn cancel_is_idempotent() {
        let mut gesture = BrushGesture::default();
        gesture.cancel();
        gesture.cancel();
        assert!(!gesture.is_active());
    }

    #[test]
    fn preview_snapshot_is_none_without_an_active_stroke() {
        assert!(BrushGesture::default().preview_snapshot().is_none());
    }

    #[test]
    fn preview_snapshot_footprint_matches_what_release_would_commit() {
        // The plan.md §5 tripwire: brush preview footprint must come from the
        // SAME `stroke_corridor_footprint` the release commit uses. Mutation
        // check: swapping the accessor to build its own polygon (instead of
        // delegating) would desync this from `stroke_action`'s footprint and
        // this assertion would go red.
        let samples = vec![[0.0, 0.0], [4.0, 0.0], [4.0, 4.0]];
        let gesture = BrushGesture {
            scope: None,
            active: Some(ActiveStroke {
                petal_id: "p1".to_string(),
                samples: samples.clone(),
                settings: snapshot("raise"),
            }),
        };
        let snap = gesture.preview_snapshot().expect("active stroke previews");
        let expected = stroke_corridor_footprint(&samples, snap_radius());
        assert_eq!(snap.op, "raise");
        assert_eq!(Some(snap.footprint), expected);
    }

    /// `snapshot("raise")`'s radius, kept in one place so the two call sites
    /// above can't drift.
    fn snap_radius() -> f32 {
        snapshot("raise").radius
    }

    #[test]
    fn map_loading_check_is_scoped_to_active_petal() {
        let mut map = PetalMapState {
            petal_id: Some("p".into()),
            loaded: false,
            ..Default::default()
        };
        assert!(petal_map_is_loading(&map, "p"));
        map.loaded = true;
        assert!(!petal_map_is_loading(&map, "p"));
        assert!(petal_map_is_loading(&map, "other"));

        let mut app = App::new();
        app.insert_resource(NavigationManager {
            active_verse_id: Some("v".into()),
            active_fractal_id: Some("f".into()),
            active_petal_id: Some("p".into()),
            ..Default::default()
        })
        .insert_resource(PetalMapState {
            petal_id: Some("p".into()),
            loaded: true,
            world_scale: 2.0,
            terrain_json: Some(
                serde_json::json!({"enabled":true,"origin":{"lat":0,"lon":0,"ele":0}}),
            ),
            ..Default::default()
        })
        .insert_resource(crate::plugin::LocalUserRole {
            role: Some(fe_database::RoleLevel::Editor),
        })
        .insert_resource(ToolState {
            active_tool: Tool::Brush,
            ..Default::default()
        })
        .insert_resource(SculptToolState {
            shape_mode: crate::actions::terrain_proposal::SculptShapeMode::Polygon,
            ..Default::default()
        })
        .init_resource::<BrushGesture>()
        .init_resource::<UiManager>()
        .init_resource::<crate::notice::NoticeClock>()
        .init_resource::<ButtonInput<MouseButton>>()
        .init_resource::<ClickArbiter>()
        .add_systems(Update, handle_brush_interaction);
        for (x, z) in [(0.0, 0.0), (4.0, 0.0), (0.0, 6.0)] {
            app.insert_resource(ClickArbiter::pressed_along(Ray3d::new(
                Vec3::new(x, 10.0, z),
                Dir3::NEG_Y,
            )));
            app.update();
        }
        assert_eq!(
            app.world().resource::<SculptToolState>().region_draft,
            vec![[0.0, 0.0], [2.0, 0.0], [0.0, 3.0]]
        );
        assert!(
            app.world_mut()
                .resource_mut::<UiManager>()
                .drain_actions()
                .is_empty(),
            "draft clicks never write"
        );
        app.insert_resource(ClickArbiter::default());
        app.world_mut()
            .resource_mut::<ToolState>()
            .terrain
            .request_pop();
        app.update();
        assert_eq!(
            app.world().resource::<SculptToolState>().region_draft.len(),
            2
        );
        app.insert_resource(ClickArbiter::pressed_along(Ray3d::new(
            Vec3::new(0.0, 10.0, 6.0),
            Dir3::NEG_Y,
        )));
        app.update();
        app.insert_resource(ClickArbiter::default());
        app.world_mut()
            .resource_mut::<ToolState>()
            .terrain
            .request_close();
        app.update();
        let UiAction::CommitTerrainAuthoring(commit) = app
            .world_mut()
            .resource_mut::<UiManager>()
            .drain_actions()
            .pop()
            .unwrap()
        else {
            panic!("polygon close must queue the scoped intent")
        };
        let nav = app.world().resource::<NavigationManager>();
        let map = app.world().resource::<PetalMapState>();
        let role = app.world().resource::<crate::plugin::LocalUserRole>();
        let UiAction::SculptShapeRegion { footprint, .. } =
            commit.clone().into_action(nav, map, role).unwrap()
        else {
            panic!("polygon must reuse the ordinary shape action")
        };
        assert_eq!(footprint, vec![[0.0, 0.0], [2.0, 0.0], [0.0, 3.0]]);
        let mut moved = NavigationManager {
            active_verse_id: Some("v".into()),
            active_fractal_id: Some("foreign".into()),
            active_petal_id: Some("p".into()),
            ..Default::default()
        };
        assert!(
            commit.clone().into_action(&moved, map, role).is_none(),
            "same Petal ID cannot hide a changed parent"
        );
        moved.active_fractal_id = Some("f".into());
        assert!(commit
            .clone()
            .into_action(
                &moved,
                map,
                &crate::plugin::LocalUserRole {
                    role: Some(fe_database::RoleLevel::Viewer)
                }
            )
            .is_none());
        let mut changed_map = map.clone();
        changed_map.terrain_json.as_mut().unwrap()["origin"]["lat"] = serde_json::json!(5);
        assert!(
            commit.into_action(&moved, &changed_map, role).is_none(),
            "map assignment changed before dispatch"
        );

        app.world_mut().resource_mut::<SculptToolState>().shape_mode =
            crate::actions::terrain_proposal::SculptShapeMode::Circle;
        app.world_mut().resource_mut::<SculptToolState>().radius = 3.0;
        app.world_mut()
            .resource_scope(|world, mut tool: bevy::prelude::Mut<ToolState>| {
                let nav = world.resource::<NavigationManager>();
                let map = world.resource::<PetalMapState>();
                let role = world.resource::<crate::plugin::LocalUserRole>();
                let sculpt = world.resource::<SculptToolState>();
                assert!(tool.terrain.arm_shape(nav, map, role, sculpt, Tool::Brush));
            });
        assert!(
            app.world_mut()
                .resource_mut::<UiManager>()
                .drain_actions()
                .is_empty(),
            "arming is not persistence"
        );
        app.insert_resource(ClickArbiter::pressed_along(Ray3d::new(
            Vec3::new(20.0, 10.0, -10.0),
            Dir3::NEG_Y,
        )));
        app.update();
        let UiAction::CommitTerrainAuthoring(commit) = app
            .world_mut()
            .resource_mut::<UiManager>()
            .drain_actions()
            .pop()
            .unwrap()
        else {
            panic!("shape placement")
        };
        let UiAction::SculptShapeRegion { footprint, .. } = commit
            .into_action(
                app.world().resource::<NavigationManager>(),
                app.world().resource::<PetalMapState>(),
                app.world().resource::<crate::plugin::LocalUserRole>(),
            )
            .unwrap()
        else {
            panic!("shape action")
        };
        let mean_x = footprint.iter().map(|p| p[0]).sum::<f32>() / footprint.len() as f32;
        let mean_z = footprint.iter().map(|p| p[1]).sum::<f32>() / footprint.len() as f32;
        assert!(
            (mean_x - 10.0).abs() < 0.0001 && (mean_z + 5.0).abs() < 0.0001,
            "clicked world point converts to the actual meter center, not the origin"
        );

        app.world_mut().resource_mut::<ToolState>().active_tool = Tool::Select;
        app.world_mut()
            .resource_scope(|world, mut tool: bevy::prelude::Mut<ToolState>| {
                assert!(tool.terrain.arm_proposal(
                    world.resource::<NavigationManager>(),
                    world.resource::<PetalMapState>(),
                    world.resource::<crate::plugin::LocalUserRole>(),
                    crate::terrain_proposal_state::ProposalOp::Raise,
                    4.0,
                    5.0,
                    1.0,
                    Tool::Select
                ));
            });
        app.insert_resource(ClickArbiter::pressed_along(Ray3d::new(
            Vec3::new(30.0, 10.0, 40.0),
            Dir3::NEG_Y,
        )));
        app.update();
        let UiAction::CommitTerrainAuthoring(commit) = app
            .world_mut()
            .resource_mut::<UiManager>()
            .drain_actions()
            .pop()
            .unwrap()
        else {
            panic!("proposal placement")
        };
        let UiAction::TerrainProposalAdd { footprint, .. } = commit
            .into_action(
                app.world().resource::<NavigationManager>(),
                app.world().resource::<PetalMapState>(),
                app.world().resource::<crate::plugin::LocalUserRole>(),
            )
            .unwrap()
        else {
            panic!("proposal action")
        };
        assert!(
            (footprint.iter().map(|p| p[0]).sum::<f32>() / footprint.len() as f32 - 30.0).abs()
                < 0.0001
        );
        assert!(
            (footprint.iter().map(|p| p[1]).sum::<f32>() / footprint.len() as f32 - 40.0).abs()
                < 0.0001
        );
        app.world_mut().resource_mut::<ToolState>().active_tool = Tool::Brush;
        app.world_mut().resource_mut::<SculptToolState>().shape_mode =
            crate::actions::terrain_proposal::SculptShapeMode::Brush;
        app.world_mut()
            .resource_mut::<crate::plugin::LocalUserRole>()
            .role = Some(fe_database::RoleLevel::Viewer);
        app.insert_resource(ClickArbiter::pressed_along(Ray3d::new(
            Vec3::new(1.0, 10.0, 1.0),
            Dir3::NEG_Y,
        )));
        app.update();
        assert!(!app.world().resource::<BrushGesture>().is_active());
        assert!(
            app.world_mut()
                .resource_mut::<UiManager>()
                .drain_actions()
                .is_empty(),
            "Viewer press cannot queue a sculpt operation"
        );
    }
}
