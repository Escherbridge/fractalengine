//! Road builder viewport system: claims the press, snaps the cursor, feeds the
//! chain reducer and maps its commits to existing `UiAction`s. See
//! `fe-ui/src/node_manager/AGENTS.md` §road-interaction.

use bevy::prelude::*;
use bevy_egui::input::EguiWantsInput;

use super::measure::measure_world_scale;
use super::path_point_interaction::{ForeignSnapSources, SnapCandidate};
use super::path_segment_interaction::TrackPickShape;
use super::road_chain::{
    reduce, shortcut_to_event, RoadBuilderState, RoadCommit, RoadEvent, RoadKey, RoadMode,
};
use super::road_snap::{resolve_snap, GuideSource, RoadSnapSettings, SnapInput, SnapOutcome};
use super::router::{ClickArbiter, ClickPriority, PointerPhase};
use crate::actions::{UiAction, UiManager};
use crate::dialogs::ActiveDialog;
use crate::geometry::meters_to_world;
use crate::gis::{PathEditorState, PendingPenCreate, PenCreationContext};
use crate::navigation_manager::NavigationManager;
use crate::panels::toolbar::Tool;
use crate::plugin::{SpawnedNodeMarker, ToolState};
use crate::terrain_map::PetalMapState;
use crate::undo::UndoState;
use crate::verse_manager::VerseManager;

/// Snap candidate and guide-source cap per frame (NFR-2).
const MAX_SNAP_ENTRIES: usize = 2048;

/// Keys the road tool reads (Escape belongs to `GestureParams.cancel_all`; A is the
/// camera's fly-left key, so angle snap toggles from the Options panel only).
const ROAD_KEYS: [(KeyCode, RoadKey); 4] = [
    (KeyCode::Digit1, RoadKey::Digit1),
    (KeyCode::Digit2, RoadKey::Digit2),
    (KeyCode::Digit3, RoadKey::Digit3),
    (KeyCode::Backspace, RoadKey::Backspace),
];

/// The resolved cursor and snap outcome of the latest frame, read by the overlay.
#[derive(Resource, Default)]
pub(crate) struct RoadHoverState {
    pub(crate) point: Option<[f32; 3]>,
    pub(crate) outcome: Option<SnapOutcome>,
}

/// Per-system state: petal tracking, freeform drag flag and reusable buffers.
#[derive(Default)]
pub(super) struct RoadLocals {
    last_petal: Option<String>,
    freeform_drag: bool,
    /// Editing track at the moment the first commit asked for a new track.
    track_before_create: Option<String>,
    guide_sources: Vec<GuideSource>,
    candidates: Vec<SnapCandidate>,
}

/// Unit-free previous-segment direction on the XZ plane, for angle snap.
fn reference_direction(state: &RoadBuilderState) -> Option<[f32; 2]> {
    let anchor = state.anchor?;
    let segments = &state.log.segments;
    let previous = match segments.len() {
        0 => return None,
        1 => state.log.origin?,
        count => segments[count - 2].end,
    };
    let delta = [anchor[0] - previous[0], anchor[2] - previous[2]];
    let length = delta[0].hypot(delta[1]);
    (length.is_finite() && length > f32::EPSILON).then(|| [delta[0] / length, delta[1] / length])
}

/// Endpoint of an open track with its outward XZ tangent.
fn push_endpoint(sources: &mut Vec<GuideSource>, endpoint: Vec3, neighbour: Vec3) {
    sources.push(GuideSource {
        position: endpoint.to_array(),
        tangent: Some([endpoint.x - neighbour.x, endpoint.z - neighbour.z]),
    });
}

/// Guide sources of every other open track in the petal (cap [`MAX_SNAP_ENTRIES`]).
fn collect_guide_sources<'a>(
    sources: &mut Vec<GuideSource>,
    shapes: impl Iterator<Item = (&'a SpawnedNodeMarker, &'a TrackPickShape)>,
    editing_track_id: Option<&str>,
    active_petal: &str,
) {
    for (marker, shape) in shapes {
        if sources.len() + 2 > MAX_SNAP_ENTRIES {
            break;
        }
        let count = shape.anchors.len();
        if shape.closed
            || count < 2
            || marker.petal_id != active_petal
            || Some(marker.node_id.as_str()) == editing_track_id
        {
            continue;
        }
        push_endpoint(sources, shape.anchors[0], shape.anchors[1]);
        push_endpoint(sources, shape.anchors[count - 1], shape.anchors[count - 2]);
    }
}

/// Snap `cursor` with the chain's settings converted to world units.
fn resolve_cursor(
    cursor: [f32; 3],
    state: &RoadBuilderState,
    world_scale: f64,
    suspended: bool,
    candidates: &[SnapCandidate],
    guide_sources: &[GuideSource],
) -> SnapOutcome {
    let settings = &state.settings;
    let freeform = settings.mode == RoadMode::Freeform;
    let snap_settings = RoadSnapSettings {
        // Freeform and curved control clicks keep the cursor's own bearing.
        angle_snap: settings.angle_snap && !freeform && state.pending.is_empty(),
        angle_increment_deg: settings.angle_increment_deg,
        snap_to_paths: settings.snap_to_paths,
        guidelines: settings.guidelines && !freeform,
        radius: meters_to_world(settings.snap_radius_m, world_scale),
    };
    resolve_snap(SnapInput {
        anchor: state.anchor,
        cursor,
        reference_dir: reference_direction(state),
        settings: &snap_settings,
        candidates,
        sources: guide_sources,
        suspended,
    })
}

/// Keep the chain tied to the track its first commit created; clear it when that track is gone.
fn sync_session_track(
    state: &mut RoadBuilderState,
    path_state: &PathEditorState,
    track_before_create: &Option<String>,
) {
    if state.log.is_empty() || path_state.has_pending_pen_create() {
        return;
    }
    let editing = path_state.editing_track_id.as_deref();
    match (state.session_track.as_deref(), editing) {
        (Some(session), Some(current)) if session == current => {}
        (None, Some(current)) if track_before_create.as_deref() != Some(current) => {
            state.session_track = Some(current.to_owned());
        }
        _ => {
            reduce(state, RoadEvent::Clear);
        }
    }
}

/// Map reducer commits to existing `UiAction`s (FR-10: no new op).
fn apply_commits(
    commits: Vec<RoadCommit>,
    state: &mut RoadBuilderState,
    path_state: &mut PathEditorState,
    nav: &NavigationManager,
    manager: Option<&VerseManager>,
    ui_mgr: &mut UiManager,
    track_before_create: &mut Option<String>,
) {
    for commit in commits {
        match commit {
            RoadCommit::CreateTrack { points } => {
                let Some(petal_id) = nav.active_petal_id.clone() else {
                    info!("Road: open a petal before drawing a road");
                    reduce(state, RoadEvent::Clear);
                    continue;
                };
                let Some(manager) = manager.filter(|manager| !manager.scene_access_blocked())
                else {
                    path_state.last_error =
                        Some("Scene context is unavailable; no road track was requested".into());
                    reduce(state, RoadEvent::Clear);
                    continue;
                };
                if path_state.has_pending_pen_create() {
                    continue;
                }
                let Some((&first_point, trailing)) = points.split_first() else {
                    continue;
                };
                let correlation_id = crate::gis::next_road_correlation_id();
                track_before_create.clone_from(&path_state.editing_track_id);
                path_state.pending_pen_create = Some(PendingPenCreate {
                    context: Some(PenCreationContext::capture(nav, manager)),
                    correlation_id: correlation_id.clone(),
                    first_point,
                    trailing_points: trailing.to_vec(),
                    ..Default::default()
                });
                ui_mgr.push_action(UiAction::PathCreateTrack {
                    petal_id,
                    name: state.track_name(),
                    correlation_id: Some(correlation_id),
                });
            }
            RoadCommit::Append(points) => {
                let Some(track_node_id) = path_state.editing_track_id.clone() else {
                    warn!("Road: no track is open for editing; chain ended");
                    reduce(state, RoadEvent::Clear);
                    continue;
                };
                match points.as_slice() {
                    [] => {}
                    [position] => ui_mgr.push_action(UiAction::PathAppendPoint {
                        track_node_id,
                        position: *position,
                    }),
                    _ => ui_mgr.push_action(UiAction::PathAppendShape { points }),
                }
            }
        }
    }
}

/// Issue the next undone point removal once the previous one has landed: a checked
/// remove holds the undo slot, so a batch would lose every removal after the first.
fn issue_next_removal(
    state: &mut RoadBuilderState,
    path_state: &PathEditorState,
    nav: &NavigationManager,
    undo_busy: bool,
    ui_mgr: &mut UiManager,
) {
    let Some(&index) = state.pending_removals.last() else {
        return;
    };
    let (Some(session), Some(editing)) = (
        state.session_track.as_deref(),
        path_state.editing_track_id.as_deref(),
    ) else {
        return;
    };
    if session != editing || path_state.points_pending {
        return;
    }
    let length = path_state.points.len();
    if index >= length {
        // Already gone from the track.
        state.pending_removals.pop();
        return;
    }
    // Only the current tail is removed, so a not-yet-confirmed removal is never repeated.
    if undo_busy || index + 1 != length {
        return;
    }
    let Some(origin) = crate::undo::PointEditOrigin::capture(path_state, nav) else {
        return;
    };
    state.pending_removals.pop();
    ui_mgr.push_action(UiAction::PathRemovePoint {
        track_node_id: editing.to_owned(),
        index,
        origin,
    });
}

/// Road tool: claims `ClickPriority::Road` presses, previews the snapped
/// cursor, and turns clicks / freeform drags / keys into path actions.
/// Clears the chain on tool exit or petal change. Drawing is the overlay's.
#[allow(clippy::too_many_arguments)]
pub(super) fn handle_road_interaction(
    tool: Res<ToolState>,
    nav: Res<NavigationManager>,
    mut path_state: ResMut<PathEditorState>,
    mut ui_mgr: ResMut<UiManager>,
    mut state: ResMut<RoadBuilderState>,
    mut hover: ResMut<RoadHoverState>,
    mut arbiter: ResMut<ClickArbiter>,
    keyboard: Res<ButtonInput<KeyCode>>,
    petal_map: Res<PetalMapState>,
    sources: ForeignSnapSources,
    shapes: Query<(&SpawnedNodeMarker, &TrackPickShape)>,
    verse_mgr: Option<Res<VerseManager>>,
    egui_input: Option<Res<EguiWantsInput>>,
    undo: Option<Res<UndoState>>,
    mut locals: Local<RoadLocals>,
) {
    let RoadLocals {
        last_petal,
        freeform_drag,
        track_before_create,
        guide_sources,
        candidates,
    } = &mut *locals;

    let petal_changed = *last_petal != nav.active_petal_id;
    if petal_changed {
        last_petal.clone_from(&nav.active_petal_id);
    }
    if petal_changed || tool.active_tool != Tool::Road {
        *freeform_drag = false;
        if state.is_active() || !state.log.is_empty() {
            reduce(&mut state, RoadEvent::Clear);
        }
        if hover.point.is_some() || hover.outcome.is_some() {
            *hover = RoadHoverState::default();
        }
        if tool.active_tool != Tool::Road {
            return;
        }
    }

    sync_session_track(&mut state, &path_state, track_before_create);

    // Cursor: terrain-snapped ground point, then the snap total order.
    let phase = arbiter.phase();
    let raw_ground = arbiter
        .ray()
        .filter(|_| arbiter.is_available())
        .and_then(|ray| sources.ground(&ray))
        .map(|ground| ground.to_array());
    let resolved = raw_ground.map(|cursor| {
        let active_petal = nav.active_petal_id.as_deref();
        sources.candidates_into(path_state.editing_track_id.as_deref(), active_petal, candidates);
        candidates.truncate(MAX_SNAP_ENTRIES);
        guide_sources.clear();
        if state.settings.guidelines && state.settings.mode != RoadMode::Freeform {
            if let Some(petal) = active_petal {
                collect_guide_sources(
                    guide_sources,
                    shapes.iter(),
                    path_state.editing_track_id.as_deref(),
                    petal,
                );
            }
        }
        let world_scale = measure_world_scale(&petal_map, active_petal);
        let suspended = keyboard.any_pressed([KeyCode::AltLeft, KeyCode::AltRight]);
        resolve_cursor(cursor, &state, world_scale, suspended, candidates, guide_sources)
    });
    let point = resolved.as_ref().map(|outcome| outcome.point);
    hover.point = point;
    hover.outcome = resolved;

    let mut commits: Vec<RoadCommit> = Vec::new();

    // Keyboard command boundary: text entry and modals own the keys.
    let keyboard_free = !egui_input
        .as_deref()
        .is_some_and(EguiWantsInput::wants_any_keyboard_input)
        && matches!(
            ui_mgr.active_dialog,
            ActiveDialog::None | ActiveDialog::HexonManager { .. }
        )
        && !keyboard.any_pressed([
            KeyCode::ControlLeft,
            KeyCode::ControlRight,
            KeyCode::SuperLeft,
            KeyCode::SuperRight,
        ]);
    if keyboard_free {
        for (code, key) in ROAD_KEYS {
            if !keyboard.just_pressed(code) {
                continue;
            }
            if let Some(event) = shortcut_to_event(key, &state) {
                commits.extend(reduce(&mut state, event));
            }
        }
    }

    match phase {
        PointerPhase::Press => {
            if let Some(point) = point {
                if arbiter.claim(ClickPriority::Road) {
                    let blocked_by_pending = path_state.has_pending_pen_create();
                    if nav.active_petal_id.is_none() {
                        info!("Road: open a petal before drawing a road");
                    } else if blocked_by_pending {
                        // Double-click guard: the track create has not echoed yet.
                    } else if state.log.is_empty()
                        && verse_mgr
                            .as_deref()
                            .is_none_or(VerseManager::scene_access_blocked)
                    {
                        path_state.last_error =
                            Some("Scene context is unavailable; no road track was requested".into());
                    } else if state.settings.mode == RoadMode::Freeform {
                        // The first stroke opens the chain at the press point, so it is not lost.
                        if state.anchor.is_none() {
                            commits.extend(reduce(&mut state, RoadEvent::Click(point)));
                        }
                        if let Some(anchor) = state.anchor {
                            *freeform_drag = true;
                            reduce(&mut state, RoadEvent::DragSample(anchor));
                            reduce(&mut state, RoadEvent::DragSample(point));
                        }
                    } else {
                        commits.extend(reduce(&mut state, RoadEvent::Click(point)));
                    }
                }
            }
        }
        PointerPhase::Hold if *freeform_drag => {
            if state.freeform_samples.is_empty() {
                // Escape (or a mode switch) cancelled the stroke mid-drag.
                *freeform_drag = false;
            } else {
                arbiter.claim(ClickPriority::Road);
                if let Some(sample) = raw_ground {
                    if state.freeform_samples.last() != Some(&sample) {
                        reduce(&mut state, RoadEvent::DragSample(sample));
                    }
                }
            }
        }
        PointerPhase::Release if *freeform_drag => {
            *freeform_drag = false;
            arbiter.claim(ClickPriority::Road);
            if let Some(end) = point {
                reduce(&mut state, RoadEvent::DragSample(end));
            }
            commits.extend(reduce(&mut state, RoadEvent::DragEnd));
        }
        PointerPhase::Hover => *freeform_drag = false,
        _ => {}
    }

    if !commits.is_empty() {
        apply_commits(
            commits,
            &mut state,
            &mut path_state,
            &nav,
            verse_mgr.as_deref(),
            &mut ui_mgr,
            track_before_create,
        );
    }
    // After input, so a Backspace issues its first removal in the same frame.
    issue_next_removal(
        &mut state,
        &path_state,
        &nav,
        undo.as_deref().is_some_and(UndoState::is_pending),
        &mut ui_mgr,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::visibility::VisibilityState;
    use fe_renderer::terrain_height::{HeightTile, TerrainHeightField};

    /// A ray straight down onto ground point (x, z).
    fn down_at(x: f32, z: f32) -> Ray3d {
        Ray3d::new(Vec3::new(x, 50.0, z), Dir3::NEG_Y)
    }

    fn road_app(tool: Tool) -> App {
        let mut app = App::new();
        let mut road = RoadBuilderState::default();
        // Exact coordinates: the snap order is covered by the road_snap tests.
        road.settings.angle_snap = false;
        road.settings.guidelines = false;
        app.insert_resource(ToolState {
            active_tool: tool,
            ..Default::default()
        })
        .insert_resource(NavigationManager {
            active_petal_id: Some("petal-1".into()),
            ..Default::default()
        })
        .insert_resource(ClickArbiter::default())
        .insert_resource(road)
        .init_resource::<PathEditorState>()
        .init_resource::<UiManager>()
        .init_resource::<RoadHoverState>()
        .init_resource::<ButtonInput<KeyCode>>()
        .init_resource::<PetalMapState>()
        .init_resource::<VisibilityState>()
        .init_resource::<VerseManager>()
        .add_systems(Update, handle_road_interaction);
        app
    }

    fn press(app: &mut App, x: f32, z: f32) {
        app.insert_resource(ClickArbiter::pressed_along(down_at(x, z)));
        app.update();
    }

    fn drained(app: &mut App) -> Vec<UiAction> {
        app.world_mut().resource_mut::<UiManager>().drain_actions()
    }

    /// Two clicks create the track; the echo is faked; the chain continues, then Backspace undoes it.
    #[test]
    fn road_chain_flows_through_the_arbiter_to_existing_actions() {
        let mut app = road_app(Tool::Road);

        press(&mut app, 0.0, 0.0);
        assert!(
            app.world().resource::<ClickArbiter>().is_owner(ClickPriority::Road),
            "AC-8.2: the press is Road's, so NodePick never selects"
        );
        assert!(drained(&mut app).is_empty(), "the first click only opens the chain");

        press(&mut app, 10.0, 0.0);
        let actions = drained(&mut app);
        assert!(
            matches!(
                actions.as_slice(),
                [UiAction::PathCreateTrack { petal_id, name, correlation_id: Some(id) }]
                    if petal_id == "petal-1" && name == "Road 1" && id.starts_with("road-track:")
            ),
            "AC-1.1: second click requests the track"
        );
        press(&mut app, 20.0, 0.0);
        assert!(
            drained(&mut app).is_empty(),
            "a click before the create echo is held back (double-click guard)"
        );

        // Fake the NodeCreated echo: the flush would append first_point + trailing_points.
        let pending = {
            let mut path = app.world_mut().resource_mut::<PathEditorState>();
            let pending = path.pending_pen_create.take().expect("pending create");
            path.start_editing("track-1".into());
            pending
        };
        assert_eq!(pending.first_point, [0.0, 0.0, 0.0]);
        assert_eq!(pending.trailing_points, vec![[10.0, 0.0, 0.0]]);

        press(&mut app, 10.0, 10.0);
        let actions = drained(&mut app);
        assert!(
            matches!(
                actions.as_slice(),
                [UiAction::PathAppendPoint { track_node_id, position }]
                    if track_node_id == "track-1" && *position == [10.0, 0.0, 10.0]
            ),
            "AC-2.1: the chain continues with one new point, no duplicate anchor"
        );

        // The action processor is not in this app: mirror the local buffer the flush and append build.
        app.world_mut().resource_mut::<PathEditorState>().points = [[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [10.0, 0.0, 10.0]]
            .map(|position| crate::gis::PathPointRow {
                position,
                ..Default::default()
            })
            .to_vec();

        app.insert_resource(ClickArbiter::default());
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::Backspace);
        app.update();
        let actions = drained(&mut app);
        assert!(
            matches!(
                actions.as_slice(),
                [UiAction::PathRemovePoint { track_node_id, index: 2, .. }] if track_node_id == "track-1"
            ),
            "AC-2.2: Backspace removes exactly the last segment's point"
        );
        app.world_mut().resource_mut::<ButtonInput<KeyCode>>().clear();
        app.update();
        assert!(
            drained(&mut app).is_empty(),
            "a removal is issued once, never repeated while it is unconfirmed"
        );

        app.world_mut()
            .resource_mut::<NavigationManager>()
            .active_petal_id = Some("petal-2".into());
        app.update();
        let state = app.world().resource::<RoadBuilderState>();
        assert!(!state.is_active() && state.log.is_empty(), "a petal switch clears the chain");
    }

    /// A click near a foreign endpoint stores that endpoint's exact coordinate (AC-5.1).
    #[test]
    fn click_near_a_foreign_endpoint_copies_its_coordinate_bitwise() {
        let mut app = road_app(Tool::Road);
        let endpoint = Vec3::new(30.123, 0.0, 5.321);
        app.world_mut().spawn((
            SpawnedNodeMarker {
                node_id: "other".into(),
                petal_id: "petal-1".into(),
            },
            TrackPickShape {
                source_witness: None,
                points: vec![endpoint, Vec3::new(40.0, 0.0, 5.0)],
                anchors: vec![endpoint, Vec3::new(40.0, 0.0, 5.0)],
                half_width: 1.0,
                closed: false,
                fill_triangles: Vec::new(),
                centroid: Vec3::ZERO,
            },
        ));
        press(&mut app, 0.0, 0.0);
        press(&mut app, 30.2, 5.4);
        let pending = app
            .world()
            .resource::<PathEditorState>()
            .pending_pen_create
            .clone()
            .expect("pending create");
        let stored = pending.trailing_points[0];
        assert_eq!(
            stored.map(f32::to_bits),
            endpoint.to_array().map(f32::to_bits),
            "vertex snap copies the foreign coordinate verbatim"
        );
    }

    /// D-38: the placed point rides the terrain surface when a tile is resident, Y=0 otherwise.
    #[test]
    fn placed_point_height_follows_the_terrain_field() {
        let mut flat = road_app(Tool::Road);
        let mut field = TerrainHeightField::default();
        field.insert(
            (0, 0, 0),
            HeightTile {
                anchor: Vec3::ZERO,
                tile_size: 100.0,
                grid: vec![4.0; 4],
                width: 2,
                height: 2,
            },
        );
        flat.insert_resource(field);
        press(&mut flat, 10.0, 10.0);
        assert_eq!(flat.world().resource::<RoadBuilderState>().anchor, Some([10.0, 4.0, 10.0]));

        let mut bare = road_app(Tool::Road);
        press(&mut bare, 10.0, 10.0);
        assert_eq!(bare.world().resource::<RoadBuilderState>().anchor, Some([10.0, 0.0, 10.0]));
    }

    #[test]
    fn other_tools_never_claim_and_freeform_first_press_opens_the_stroke() {
        let mut app = road_app(Tool::Select);
        press(&mut app, 1.0, 1.0);
        assert!(!app.world().resource::<ClickArbiter>().is_claimed());
        assert!(!app.world().resource::<RoadBuilderState>().is_active());

        // Freeform: the very first press opens the chain and starts the stroke (no lost drag).
        let mut freeform = road_app(Tool::Road);
        freeform
            .world_mut()
            .resource_mut::<RoadBuilderState>()
            .settings
            .mode = RoadMode::Freeform;
        press(&mut freeform, 4.0, 2.0);
        let state = freeform.world().resource::<RoadBuilderState>();
        assert_eq!(state.anchor, Some([4.0, 0.0, 2.0]));
        assert!(!state.freeform_samples.is_empty(), "the first press already samples the stroke");
    }
}
