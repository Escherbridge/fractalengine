//! Viewport click → select / deselect nearest spawned node. Claims `NodePick`.
//! Precise ray/AABB pick against glb geometry — see `node_manager/AGENTS.md`
//! §glb-mesh-picking.

use bevy::camera::primitives::Aabb;
use bevy::math::Affine3A;
use bevy::prelude::*;

use super::dispatch::{resolve_operation, HitTarget, Operation};
use super::path_segment_interaction::{ray_polyline_hit, TrackPickShape};
use super::router::{ClickArbiter, ClickPriority};
use super::selection::SelectionKind;
use super::NodeManager;
use crate::navigation_manager::NavigationManager;
use crate::plugin::{SpawnedNodeMarker, ToolState};
use crate::visibility::{pickable, rendered, VisibilityState};

pub(super) fn handle_viewport_click(
    node_query: Query<(
        Entity,
        &SpawnedNodeMarker,
        Option<&TrackPickShape>,
        Option<&Visibility>,
    )>,
    g_transform_query: Query<&GlobalTransform>,
    aabb_query: Query<&Aabb>,
    children_query: Query<&Children>,
    mut manager: ResMut<NodeManager>,
    nav: Res<NavigationManager>,
    tool: Res<ToolState>,
    visibility: Res<VisibilityState>,
    mut arbiter: ResMut<ClickArbiter>,
    mut stamps: super::stamp_selection::StampPickParams,
    keyboard: Option<Res<ButtonInput<KeyCode>>>,
    mut ui: Option<ResMut<crate::actions::UiManager>>,
    clock: Option<Res<crate::notice::NoticeClock>>,
) {
    // Only act on a fresh left-press that reached the viewport (egui/rect gating
    // already applied by `resolve_pointer_frame`).
    if !arbiter.is_fresh_press() {
        return;
    }
    // Yield if a higher-priority consumer (gimbal / path-point) already claimed
    // this frame's click.
    if !arbiter.claim(ClickPriority::NodePick) {
        return;
    }
    let Some(ray) = arbiter.ray() else { return };

    let Some(active_petal) = nav.active_petal_id.as_deref() else {
        return;
    };
    let mut best: Option<(Entity, f32, String)> = None;

    for (entity, marker, pick_shape, drawn) in node_query.iter() {
        if active_petal != marker.petal_id {
            continue;
        }
        // D-13/D-14: hidden nodes never win a pick (shared picker rule).
        if !pickable(&marker.node_id, &nav, &visibility) || !rendered(drawn) {
            continue;
        }
        // FR-1: a rendered track ribbon carries `TrackPickShape` (its actual
        // polyline). Narrow-phase ray-vs-segment against it instead of the giant
        // flat AABB, so a km-scale track no longer swallows clicks for nearby
        // objects. `t` is the closest-approach distance along the ray, directly
        // comparable to the AABB entry `t` below (nearer objects still win).
        let t = if let Some(shape) = pick_shape {
            ray_polyline_hit(
                &shape.points,
                ray.origin,
                *ray.direction,
                shape.half_width,
                &shape.fill_triangles,
            )
        } else {
            // Resolve the pickable Aabb: glTF scenes place it on a child mesh,
            // not the root marker entity — mirror `gimbal_center`'s
            // entity-then-children walk (`gimbal.rs`), but slab-test instead of
            // centering. Whatever child we hit, selection resolves to this root
            // `entity` (FR-2).
            pick_node_aabb(
                entity,
                &ray,
                &g_transform_query,
                &aabb_query,
                &children_query,
            )
        };
        let Some(t) = t else { continue };
        if best.as_ref().is_none_or(|b| t < b.1) {
            best = Some((entity, t, marker.node_id.clone()));
        }
    }

    // Preserve the existing priority tiers; classify owned stamps at the node tier.
    let hit = match best.as_ref() {
        Some((entity, _, _)) if stamps.is_stamp(*entity) => HitTarget::Stamp(*entity),
        Some((entity, _, _)) => HitTarget::Node(*entity),
        None => HitTarget::Empty,
    };
    let command = keyboard.as_ref().is_some_and(|keys| {
        keys.any_pressed([
            KeyCode::ControlLeft,
            KeyCode::ControlRight,
            KeyCode::SuperLeft,
            KeyCode::SuperRight,
        ])
    });
    let shift = keyboard
        .as_ref()
        .is_some_and(|keys| keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]));
    let mode = if command {
        super::SelectionMode::Toggle
    } else if shift {
        super::SelectionMode::Extend
    } else {
        super::SelectionMode::Replace
    };
    match resolve_operation(tool.active_tool, &SelectionKind::Empty, hit) {
        Operation::SelectNode(_) => {
            if let Some((entity, _, node_id)) = best {
                let result = manager.reduce_selection(
                    Some(active_petal),
                    vec![super::SelectionTarget { entity, node_id }],
                    mode,
                );
                match result {
                    Ok(_) => stamps.clear_components(),
                    Err(reason) => {
                        if let Some(ui) = ui.as_mut() {
                            ui.show_advisory(
                                reason,
                                clock.as_ref().map_or(0.0, |clock| clock.elapsed_secs_f64()),
                            );
                        }
                    }
                }
            }
        }
        Operation::SelectStamp(entity) => {
            stamps.queue_mode(entity, manager.selection_generation, mode)
        }
        Operation::Deselect if !command && !shift => {
            manager.deselect();
            stamps.clear_components();
        }
        // The only other verbs a `Node`/`Empty` hit can produce here are Pen's
        // `PlacePathPoint` (unreachable — `PathPlace` claims the frame first)
        // and Brush's `None` (unreachable — `Brush` claims 2nd in the chain).
        // The wildcard keeps the match total over the whole `Operation` set.
        _ => {}
    }
}

/// Nearest per-entity hit `t` across `root` and ALL its descendants (iterative
/// DFS). Pure over the two lookups so the *depth* traversal — the crux of the
/// glTF-scene fix below — is unit-testable without a Bevy App. `hit_of` yields a
/// candidate along-ray distance for an entity's own geometry (if any);
/// `children_of` yields its direct children.
fn nearest_in_subtree(
    root: Entity,
    hit_of: impl Fn(Entity) -> Option<f32>,
    children_of: impl Fn(Entity) -> Vec<Entity>,
) -> Option<f32> {
    let mut best: Option<f32> = None;
    let mut stack = vec![root];
    while let Some(ent) = stack.pop() {
        if let Some(t) = hit_of(ent) {
            if best.is_none_or(|b| t < b) {
                best = Some(t);
            }
        }
        stack.extend(children_of(ent));
    }
    best
}

/// Nearest along-ray hit `t` for a node's geometry Aabb, or `None` on a miss.
///
/// Walks the root `entity` AND every descendant (not just immediate children):
/// glTF `SceneRoot`s nest the mesh + `Aabb` several levels down, so a one-level
/// scan finds nothing and silently breaks GLB selection (the shipped
/// regression). Returns the smallest entry distance across all candidates so
/// overlapping child meshes resolve to the closest surface. `t` is measured
/// along the world-space ray direction and is comparable across entities.
/// `pub(super)`: shared with the right-click classifier (`context_pick`).
pub(super) fn pick_node_aabb(
    entity: Entity,
    ray: &Ray3d,
    g_transform_query: &Query<&GlobalTransform>,
    aabb_query: &Query<&Aabb>,
    children_query: &Query<&Children>,
) -> Option<f32> {
    nearest_in_subtree(
        entity,
        |ent| {
            let g_tx = g_transform_query.get(ent).ok()?;
            let aabb = aabb_query.get(ent).ok()?;
            ray_aabb_hit(
                ray.origin,
                *ray.direction,
                g_tx.affine(),
                Vec3::from(aabb.center),
                Vec3::from(aabb.half_extents),
            )
        },
        |ent| {
            children_query
                .get(ent)
                .map(|c| c.iter().collect())
                .unwrap_or_default()
        },
    )
}

/// Ray/AABB slab intersection in the box's local space.
///
/// The `Aabb` is axis-aligned in the entity's local space (`center ±
/// half_extents`); transforming the world ray into that space via the inverse
/// `affine` turns the world-space OBB test into a plain slab test while keeping
/// the parametric `t` identical to the along-world-ray distance (both origin
/// and direction share the same transform). Returns the entry `t` when the ray
/// enters the box in front of its origin, else `None` (miss or box behind).
fn ray_aabb_hit(
    origin: Vec3,
    dir: Vec3,
    affine: Affine3A,
    center: Vec3,
    half_extents: Vec3,
) -> Option<f32> {
    let inv = affine.inverse();
    let local_origin = inv.transform_point3(origin);
    // Direction is a vector, not a point — do NOT normalize: sharing the world
    // ray's magnitude keeps `t` comparable across entities.
    let local_dir = inv.transform_vector3(dir);

    let min = center - half_extents;
    let max = center + half_extents;

    let mut t_enter = f32::NEG_INFINITY;
    let mut t_exit = f32::INFINITY;

    for axis in 0..3 {
        let o = local_origin[axis];
        let d = local_dir[axis];
        let lo = min[axis];
        let hi = max[axis];
        if d.abs() < 1e-8 {
            // Ray parallel to this slab: miss if the origin is outside it.
            if o < lo || o > hi {
                return None;
            }
        } else {
            let inv_d = 1.0 / d;
            let mut t0 = (lo - o) * inv_d;
            let mut t1 = (hi - o) * inv_d;
            if t0 > t1 {
                std::mem::swap(&mut t0, &mut t1);
            }
            t_enter = t_enter.max(t0);
            t_exit = t_exit.min(t1);
            if t_enter > t_exit {
                return None;
            }
        }
    }

    // Box is entirely behind the ray origin.
    if t_exit < 0.0 {
        return None;
    }
    // Entry distance, clamped to 0 when the origin is already inside the box.
    Some(t_enter.max(0.0))
}

// ---------------------------------------------------------------------------
// Tests — pure ray/AABB math, plus one App-level pick-refusal flow. Mesh-picking
// correctness is validated in-app.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Queued commands other than a selection's stored-capture read.
    fn mutations(
        rx: &crossbeam::channel::Receiver<fe_runtime::messages::DbCommand>,
    ) -> Vec<fe_runtime::messages::DbCommand> {
        rx.try_iter()
            .filter(|command| {
                !matches!(command, fe_runtime::messages::DbCommand::ReadEditorBatch { .. })
            })
            .collect()
    }

    /// A ribbon its map layer (or style) hides — `Visibility::Hidden` set outside
    /// the lattice — refuses the click; shown again, the same click selects it.
    #[test]
    fn a_ribbon_hidden_by_its_layer_is_not_picked_until_shown() {
        // The retry leg queues writes on the process-wide GUI door; hold the lock so no other test sees them.
        let _door_lock = crate::actions::node::GUI_DOOR_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        use crate::panels::toolbar::Tool;
        let down =
            || ClickArbiter::pressed_along(Ray3d::new(Vec3::new(5.0, 50.0, 0.0), Dir3::NEG_Y));
        let mut app = App::new();
        let (stamp_tx, stamp_rx) = crossbeam::channel::unbounded();
        app.add_message::<fe_runtime::messages::DbResult>()
            .insert_resource(fe_runtime::app::DbCommandSender(stamp_tx))
            .init_resource::<crate::gis::PathEditorState>()
            .init_resource::<crate::verse_manager::PathAssetCache>()
            .init_resource::<crate::verse_manager::VerseManager>()
            .init_resource::<crate::terrain_map::PetalMapState>()
            .init_resource::<crate::actions::asset::StampInteractionState>()
            .init_resource::<crate::actions::UiManager>()
            .init_resource::<crate::notice::NoticeClock>()
            .init_resource::<crate::plugin::LocalUserRole>()
            .init_resource::<crate::panels::tool_panel::ToolPanelState>();
        super::super::stamp_selection::register(&mut app);
        app.insert_resource(NavigationManager {
            active_petal_id: Some("p1".into()),
            ..Default::default()
        })
        .insert_resource(ToolState {
            active_tool: Tool::Select,
            ..Default::default()
        })
        .init_resource::<VisibilityState>()
        .init_resource::<NodeManager>()
        .insert_resource(down())
        .add_systems(Update, handle_viewport_click);
        let ribbon = app
            .world_mut()
            .spawn((
                SpawnedNodeMarker {
                    node_id: "t1".into(),
                    petal_id: "p1".into(),
                },
                TrackPickShape {
                    source_witness: None,
                    points: vec![Vec3::ZERO, Vec3::new(10.0, 0.0, 0.0)],
                    anchors: vec![Vec3::ZERO, Vec3::new(10.0, 0.0, 0.0)],
                    half_width: 1.0,
                    closed: false,
                    fill_triangles: Vec::new(),
                    centroid: Vec3::new(5.0, 0.0, 0.0),
                },
                Visibility::Hidden,
            ))
            .id();

        app.update();
        assert!(
            app.world().resource::<NodeManager>().selected.is_none(),
            "an invisible ribbon must not take the click"
        );

        app.world_mut()
            .entity_mut(ribbon)
            .insert(Visibility::Inherited);
        app.insert_resource(down());
        app.update();
        assert_eq!(
            app.world()
                .resource::<NodeManager>()
                .selected
                .as_ref()
                .map(|s| s.node_id.as_str()),
            Some("t1")
        );

        use crate::verse_manager::{FractalEntry, NodeEntry, PetalEntry, VerseEntry};
        use bevy_egui::egui;
        use fe_runtime::messages::{DbCommand, DbResult};
        app.insert_resource(crate::verse_manager::VerseManager::from_verses(vec![
            VerseEntry {
                id: "v".into(),
                fractals: vec![FractalEntry {
                    id: "f".into(),
                    petals: vec![PetalEntry {
                        id: "p1".into(),
                        nodes: vec![
                            NodeEntry {
                                id: "t1".into(),
                                ..Default::default()
                            },
                            NodeEntry {
                                id: "n1".into(),
                                ..Default::default()
                            },
                        ],
                        ..Default::default()
                    }],
                    ..Default::default()
                }],
                ..Default::default()
            },
        ]));
        {
            let mut nav = app.world_mut().resource_mut::<NavigationManager>();
            nav.navigate_to_verse("v", "Verse");
            nav.navigate_to_fractal("f", "Fractal");
            nav.navigate_to_petal("p1");
        }
        let descriptor = serde_json::json!({"asset_path":"blob://cube.glb","spacing_mode":"fixed_count","spacing_value":0.0,"count":3,"tangent_align":true});
        let properties = serde_json::json!({"gpx_points":[[0.0,0.0,0.0,0.0],[10.0,0.0,0.0,0.0]],"path_asset":descriptor});
        app.world_mut()
            .resource_mut::<crate::verse_manager::PathAssetCache>()
            .note_properties("t1", Some("p1"), &properties);
        {
            let mut path = app
                .world_mut()
                .resource_mut::<crate::gis::PathEditorState>();
            path.start_editing("t1".into());
            path.points = crate::gis::decode_gpx_points(&properties["gpx_points"]);
            path.edited_track_path_asset =
                Some(fe_sdk::path_asset::PathAssetDescriptor::from_json(&descriptor).unwrap());
        }
        let stamp = app
            .world_mut()
            .spawn((
                SpawnedNodeMarker {
                    node_id: crate::verse_manager::stamp_marker_id("t1", 1),
                    petal_id: "p1".into(),
                },
                crate::verse_manager::PathAssetInstance {
                    source_track_id: "t1".into(),
                    petal_id: "p1".into(),
                },
                crate::verse_manager::StampSelectionSource(std::sync::Arc::new(
                    crate::verse_manager::StampRenderSource {
                        descriptor: fe_sdk::path_asset::PathAssetDescriptor::from_json(&descriptor)
                            .unwrap(),
                        points: vec![[0.0, 0.0, 0.0], [10.0, 0.0, 0.0]],
                        world_scale: 1.0,
                    },
                )),
                Transform::from_xyz(5.0, 0.0, 0.0),
                GlobalTransform::from(Transform::from_xyz(5.0, 0.0, 0.0)),
                Aabb::from_min_max(Vec3::splat(-2.0), Vec3::splat(2.0)),
                Visibility::Inherited,
            ))
            .id();
        let ordinary = app
            .world_mut()
            .spawn((
                SpawnedNodeMarker {
                    node_id: "n1".into(),
                    petal_id: "p1".into(),
                },
                GlobalTransform::from(Transform::from_xyz(20.0, 0.0, 0.0)),
                Aabb::from_min_max(Vec3::splat(-0.5), Vec3::splat(0.5)),
            ))
            .id();
        app.add_systems(
            Update,
            super::super::path_segment_interaction::handle_path_segment_interaction
                .before(handle_viewport_click),
        );
        app.world_mut()
            .resource_mut::<crate::plugin::LocalUserRole>()
            .role = Some(fe_database::RoleLevel::Viewer);
        app.insert_resource(ClickArbiter::pressed_along(Ray3d::new(
            Vec3::new(5.0, 50.0, 1.8),
            Dir3::NEG_Y,
        )));
        app.update();
        assert_eq!(
            app.world().resource::<NodeManager>().selected_entity(),
            Some(stamp),
            "the real node-tier AABB pick selects the owned stamp entity"
        );
        assert_eq!(
            app.world()
                .resource::<crate::actions::asset::StampInteractionState>()
                .selected()
                .unwrap()
                .stamp_index,
            1
        );
        assert!(
            stamp_rx.try_recv().is_err(),
            "Viewer stamp selection cannot auto-promote or write"
        );
        app.insert_resource(ClickArbiter::pressed_along(Ray3d::new(
            Vec3::new(20.0, 50.0, 0.0),
            Dir3::NEG_Y,
        )));
        app.update();
        assert_eq!(
            app.world().resource::<NodeManager>().selected_entity(),
            Some(ordinary)
        );
        assert!(app
            .world()
            .resource::<crate::actions::asset::StampInteractionState>()
            .selected()
            .is_none());
        app.world_mut()
            .resource_mut::<crate::actions::UiManager>()
            .open_dialog(crate::dialogs::ActiveDialog::ContextMenu {
                screen_pos: [20.0, 20.0],
                world_pos: [5.0, 0.0, 0.0],
                target: Some(crate::dialogs::ContextTarget {
                    petal_id: Some("p1".into()),
                    hit: super::super::HitTarget::Stamp(stamp),
                    node_id: None,
                    stamp: Some(("t1".into(), 1)),
                }),
                pending_delete: false,
                descendant_count: None,
            });
        app.insert_resource(ClickArbiter::default());
        app.update();
        assert_eq!(
            app.world().resource::<NodeManager>().selected_entity(),
            Some(stamp),
            "classified context stamp uses the same live adapter"
        );
        assert!(
            stamp_rx.try_recv().is_err(),
            "context Viewer inspection stays read-only"
        );
        app.world_mut()
            .resource_mut::<crate::actions::UiManager>()
            .close_dialog();
        app.insert_resource(ClickArbiter::pressed_along(Ray3d::new(
            Vec3::new(5.0, 50.0, 0.0),
            Dir3::NEG_Y,
        )));
        app.update();
        assert!(
            app.world()
                .resource::<ClickArbiter>()
                .is_owner(ClickPriority::PathSegment),
            "edited ribbon keeps F10 priority over the nearer cube"
        );
        assert!(app
            .world()
            .resource::<crate::actions::asset::StampInteractionState>()
            .selected()
            .is_none());
        assert!(stamp_rx.try_recv().is_err());

        // Register the real action dispatcher, not a direct stamp handler in the fixture.
        app.add_message::<fe_webview::ipc::BrowserCommand>()
            .add_message::<fe_webview::ipc::BrowserEvent>()
            .init_resource::<crate::plugin::InspectorFormState>()
            .init_resource::<crate::notice::NoticeClock>()
            .init_resource::<crate::terrain_map::PendingHexonOps>()
            .init_resource::<crate::asset_ops::PendingAssetOps>()
            .init_resource::<crate::gis::GisPanelState>()
            .init_resource::<crate::gpx_ops::PendingGpxOps>()
            .init_resource::<crate::path_ops::PendingPathOps>()
            .init_resource::<crate::undo::UndoState>()
            .init_resource::<crate::terrain_proposal_state::ProposalEditState>()
            .init_resource::<crate::actions::terrain_proposal::SculptToolState>()
            .init_resource::<crate::actions::terrain_proposal::EarthworkNodeMap>()
            .init_resource::<crate::actions::dimension_callout::DimensionCalloutMap>()
            .init_resource::<crate::ui_shell::right_sidebar::RightSidebarState>()
            .init_resource::<crate::settings::AppSettings>()
            .init_resource::<crate::settings::PetalRenderOverride>()
            .init_resource::<crate::terrain_map::TilesetDownloads>()
            .add_systems(
                Update,
                crate::actions::process_ui_actions.before(handle_viewport_click),
            );
        app.world_mut()
            .resource_mut::<crate::plugin::LocalUserRole>()
            .role = Some(fe_database::RoleLevel::Editor);
        app.insert_resource(ClickArbiter::default());
        app.update();
        // A queued capture refuses source reflow before the renderer replaces the old entity.
        let stale = app
            .world()
            .resource::<crate::node_manager::StampSelectionCatalog>()
            .origin(1)
            .unwrap();
        app.world_mut()
            .resource_mut::<crate::actions::UiManager>()
            .push_action(crate::actions::UiAction::SelectStamp {
                origin: stale.clone(),
            });
        let mut changed = properties.clone();
        changed["path_asset"]["count"] = serde_json::json!(2);
        app.world_mut()
            .resource_mut::<crate::verse_manager::PathAssetCache>()
            .note_properties("t1", Some("p1"), &changed);
        app.update();
        assert!(
            stamp_rx.try_recv().is_err(),
            "source descriptor reflow refuses the queued old index"
        );
        assert!(
            app.world()
                .resource::<crate::node_manager::StampSelectionCatalog>()
                .targets
                .is_empty(),
            "pre-restamp entities cannot be offered under new descriptor semantics"
        );
        app.world_mut()
            .resource_mut::<crate::verse_manager::PathAssetCache>()
            .note_properties("t1", Some("p1"), &properties);
        app.update();
        // A scope change also refuses an otherwise-live scene target.
        let stale = app
            .world()
            .resource::<crate::node_manager::StampSelectionCatalog>()
            .origin(1)
            .unwrap();
        app.world_mut()
            .resource_mut::<crate::actions::UiManager>()
            .push_action(crate::actions::UiAction::SelectStamp { origin: stale });
        app.world_mut()
            .resource_mut::<NavigationManager>()
            .navigate_to_petal("different-petal");
        app.update();
        assert!(
            stamp_rx.try_recv().is_err(),
            "stale navigation cannot queue promotion"
        );
        app.world_mut()
            .resource_mut::<NavigationManager>()
            .navigate_to_petal("p1");
        app.update();
        assert!(
            app.world()
                .resource::<crate::actions::asset::StampInteractionState>()
                .selected()
                .is_none(),
            "restoring a scope does not resurrect the old stamp selection"
        );
        let role_capture = app
            .world()
            .resource::<crate::node_manager::StampSelectionCatalog>()
            .origin(1)
            .unwrap();
        app.world_mut()
            .resource_mut::<crate::actions::UiManager>()
            .push_action(crate::actions::UiAction::SelectStamp {
                origin: role_capture,
            });
        app.world_mut()
            .resource_mut::<crate::plugin::LocalUserRole>()
            .role = Some(fe_database::RoleLevel::Viewer);
        app.update();
        assert!(
            stamp_rx.try_recv().is_err(),
            "Editor-to-Viewer queue race must not auto-promote"
        );
        app.world_mut()
            .resource_mut::<crate::plugin::LocalUserRole>()
            .role = Some(fe_database::RoleLevel::Editor);
        let ctx = egui::Context::default();
        let mut time = 0.0;
        let native = |events: Vec<egui::Event>, time: &mut f64, app: &mut App| {
            *time += 0.05;
            let mut tools = app
                .world_mut()
                .remove_resource::<crate::panels::tool_panel::ToolPanelState>()
                .unwrap();
            let mut ui = app
                .world_mut()
                .remove_resource::<crate::actions::UiManager>()
                .unwrap();
            {
                let world = app.world();
                let path = world.resource::<crate::gis::PathEditorState>();
                let hierarchy = world.resource::<crate::verse_manager::VerseManager>();
                crate::panels::tool_panel::sync_stamp_editor(
                    &mut tools,
                    world.resource::<crate::actions::asset::StampInteractionState>(),
                    path,
                    world.resource::<NavigationManager>(),
                    hierarchy,
                    world.resource::<crate::plugin::LocalUserRole>(),
                    1.0,
                );
                crate::ui_shell::layout_policy::begin_frame(&ctx, true);
                let _ = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(1000.0, 1500.0),
                        )),
                        time: Some(*time),
                        events,
                        ..Default::default()
                    },
                    |ctx| {
                        crate::ui_shell::layout_policy::begin_ui_pass(ctx);
                        egui::CentralPanel::default().show(ctx, |panel| {
                            crate::panels::tool_panel::render_path_asset_section(
                                panel, &mut tools, &mut ui, path, hierarchy,
                            )
                        });
                    },
                );
            }
            app.insert_resource(tools).insert_resource(ui);
            crate::ui_shell::layout_policy::observations(&ctx)
        };
        native(vec![], &mut time, &mut app);
        let picker = native(vec![], &mut time, &mut app);
        assert_eq!(
            app.world()
                .resource::<crate::panels::tool_panel::ToolPanelState>()
                .stamp_edit_index,
            2,
            "only actual rendered index1 is offered as Stamp2"
        );
        let at = picker.controls.last().unwrap().hit.center();
        for pressed in [true, false] {
            native(
                vec![
                    egui::Event::PointerMoved(at),
                    egui::Event::PointerButton {
                        pos: at,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
                &mut time,
                &mut app,
            );
        }
        app.update();
        let commands: Vec<_> = stamp_rx.try_iter().collect();
        let [DbCommand::PromoteCheckedStamp {
            capture,
            correlation_id,
        }] = commands.as_slice()
        else {
            panic!("native Select must queue one checked promotion: {commands:?}")
        };
        assert_eq!(capture.before_properties, properties);
        assert_eq!(capture.instance_index, 1);
        assert!(app
            .world()
            .resource::<crate::gis::PathEditorState>()
            .selected_segment
            .is_none());
        let mut capture = capture.clone();
        let mut promotion_request = correlation_id.clone();
        for remaining_role in [Some(fe_database::RoleLevel::Viewer), None] {
            app.world_mut()
                .resource_mut::<crate::plugin::LocalUserRole>()
                .role = remaining_role;
            app.update();
            let state = app
                .world()
                .resource::<crate::actions::asset::StampInteractionState>();
            assert!(
                state.selection_promotion.is_none(),
                "Editor loss cancels the pending promotion"
            );
            if remaining_role.is_some() {
                assert!(
                    state.selected().is_some(),
                    "Viewer inspection remains selected"
                );
                assert!(
                    !app.world()
                        .resource::<crate::node_manager::StampSelectionCatalog>()
                        .targets
                        .is_empty(),
                    "Viewer inspection retains the live selector"
                );
            } else {
                assert!(
                    state.selected().is_none(),
                    "view loss clears the stamp projection"
                );
                assert!(app
                    .world()
                    .resource::<crate::node_manager::StampSelectionCatalog>()
                    .targets
                    .is_empty());
                assert!(app
                    .world()
                    .resource::<crate::panels::tool_panel::ToolPanelState>()
                    .stamp_selection_catalog
                    .is_none());
            }
            app.world_mut().write_message(DbResult::CheckedStampPromoted {
                capture: capture.clone(), node_id: "t1#inst-1".into(),
                properties: serde_json::json!({"node_kind":"stamp","path_id":"t1","owning_path_id":"t1","instance_index":1}),
                newly_promoted: true, correlation_id: promotion_request.clone(),
            });
            app.update();
            assert_eq!(
                app.world()
                    .resource::<NodeManager>()
                    .selected
                    .as_ref()
                    .unwrap()
                    .node_id,
                "t1::stamp::1",
                "a receipt after Editor loss cannot bind a durable address"
            );
            let stamp_ref = crate::actions::asset::StampRef {
                track_node_id: "t1".into(),
                stamp_index: 1,
            };
            assert!(
                app.world()
                    .resource::<crate::actions::asset::StampInteractionState>()
                    .loaded_properties(&stamp_ref)
                    .is_none(),
                "unauthorized receipt cannot hydrate properties"
            );
            assert!(
                stamp_rx.try_recv().is_err(),
                "role-loss handling cannot write or re-promote"
            );
            app.world_mut()
                .resource_mut::<crate::plugin::LocalUserRole>()
                .role = Some(fe_database::RoleLevel::Editor);
            app.world_mut().write_message(DbResult::CheckedStampPromoted {
                capture: capture.clone(), node_id: "t1#inst-1".into(),
                properties: serde_json::json!({"node_kind":"stamp","path_id":"t1","owning_path_id":"t1","instance_index":1}),
                newly_promoted: false, correlation_id: promotion_request.clone(),
            });
            app.update();
            assert!(
                app.world()
                    .resource::<crate::actions::asset::StampInteractionState>()
                    .loaded_properties(&stamp_ref)
                    .is_none(),
                "a regrant cannot revive the invalidated receipt"
            );
            if remaining_role.is_none() {
                assert!(
                    app.world()
                        .resource::<crate::actions::asset::StampInteractionState>()
                        .selected()
                        .is_none(),
                    "view regrant alone cannot resurrect stamp inspection"
                );
            }
            assert!(
                stamp_rx.try_recv().is_err(),
                "regrant needs an explicit selection before promotion"
            );
            let origin = app
                .world()
                .resource::<crate::node_manager::StampSelectionCatalog>()
                .origin(1)
                .unwrap();
            // Retry leg: the DB channel is full at reselect, so the promotion waits in the scene-write door.
            let unbounded_sender = app
                .world()
                .resource::<fe_runtime::app::DbCommandSender>()
                .0
                .clone();
            let (full_tx, full_rx) = crossbeam::channel::bounded(1);
            full_tx.send(DbCommand::Ping).unwrap();
            app.insert_resource(fe_runtime::app::DbCommandSender(full_tx));
            app.world_mut()
                .resource_mut::<crate::actions::UiManager>()
                .push_action(crate::actions::UiAction::SelectStamp { origin });
            app.update();
            {
                let state = app
                    .world()
                    .resource::<crate::actions::asset::StampInteractionState>();
                assert!(
                    state.selection_promotion.is_some(),
                    "a queued promotion stays pending while the door holds it"
                );
                assert!(
                    state.edit_error().is_none(),
                    "a full channel queues the promotion, it does not report busy"
                );
            }
            let notice = app
                .world()
                .resource::<crate::actions::UiManager>()
                .active_notice(0.0)
                .map(|(notice, _)| notice.text.to_lowercase())
                .unwrap_or_default();
            assert!(
                !notice.contains("busy") && !notice.contains("not promoted"),
                "no error toast while the promotion is queued: {notice:?}"
            );
            assert!(
                matches!(full_rx.try_recv(), Ok(DbCommand::Ping)),
                "only the prefill is in the channel"
            );
            assert!(
                full_rx.try_recv().is_err(),
                "the promotion waits in the door, not the channel"
            );
            assert_eq!(
                crate::actions::node::pump_gui_door_after(std::time::Duration::from_millis(100)),
                1,
                "the first backoff has elapsed: the door settles the promotion"
            );
            app.insert_resource(fe_runtime::app::DbCommandSender(unbounded_sender));
            let command = full_rx
                .try_recv()
                .expect("the retried promotion reaches the channel once");
            let DbCommand::PromoteCheckedStamp {
                capture: fresh_capture,
                correlation_id: fresh_request,
            } = command
            else {
                panic!("an explicit authorized reselect queues one fresh promotion: {command:?}")
            };
            assert_eq!(
                app.world()
                    .resource::<crate::actions::asset::StampInteractionState>()
                    .selection_promotion
                    .as_deref(),
                Some(fresh_request.as_str()),
                "the retried command carries the pending correlation id"
            );
            assert_ne!(promotion_request, fresh_request);
            capture = fresh_capture;
            promotion_request = fresh_request;
        }
        let late_capture = capture.clone();
        let late_request = promotion_request.clone();
        let observe = |app: &mut App| {
            let mut state = bevy::ecs::system::SystemState::<
                crate::node_manager::PromotedStampReader,
            >::new(app.world_mut());
            let reader = state.get(app.world());
            reader.current().map(|stamp| {
                (
                    stamp.entity,
                    stamp.node_id.to_owned(),
                    stamp.path_id.to_owned(),
                    stamp.instance_index,
                    *stamp.transform,
                )
            })
        };
        assert!(
            observe(&mut app).is_none(),
            "a pending promotion is not a durable stamp observation"
        );
        app.world_mut().write_message(DbResult::CheckedStampPromoted {capture,node_id:"t1#inst-1".into(),
            properties:serde_json::json!({"node_kind":"stamp","path_id":"t1","owning_path_id":"t1","instance_index":1}),newly_promoted:true,correlation_id:promotion_request});
        app.update();
        app.update();
        assert_eq!(
            app.world()
                .resource::<NodeManager>()
                .selected
                .as_ref()
                .unwrap()
                .node_id,
            "t1#inst-1",
            "only the authoritative receipt binds the durable address"
        );
        assert!(
            app.world()
                .resource::<crate::actions::asset::StampInteractionState>()
                .selection_promotion
                .is_none(),
            "the reply to the retried promotion resolves the pending request"
        );
        assert_eq!(
            app.world().get::<SpawnedNodeMarker>(stamp).unwrap().node_id,
            "t1::stamp::1",
            "the live marker fence is preserved"
        );
        assert!(
            app.world()
                .resource::<crate::verse_manager::VerseManager>()
                .all_nodes()
                .all(|node| node.id != "t1#inst-1"),
            "fresh promotion need not have an ordinary tree entry"
        );
        let original_transform = *app.world().get::<Transform>(stamp).unwrap();
        let rendered_transform = Transform {
            translation: Vec3::new(5.25, 1.5, -0.75),
            rotation: Quat::from_rotation_y(0.4),
            scale: Vec3::new(1.25, 0.75, 2.0),
        };
        app.world_mut().entity_mut(stamp).insert(rendered_transform);
        assert_eq!(observe(&mut app), Some((stamp, "t1#inst-1".into(), "t1".into(), 1, rendered_transform)),
            "the read seam reports the selected entity's actual ECS pose under its canonical promoted address");
        {
            let mut hierarchy = app
                .world_mut()
                .resource_mut::<crate::verse_manager::VerseManager>();
            hierarchy.verses[0].fractals[0].id = "other-fractal".into();
        }
        assert!(
            observe(&mut app).is_none(),
            "a same-Petal/path label under different actual ancestry is insufficient"
        );
        app.world_mut()
            .resource_mut::<crate::verse_manager::VerseManager>()
            .verses[0]
            .fractals[0]
            .id = "f".into();
        let generation = app.world().resource::<NodeManager>().selection_generation;
        app.world_mut()
            .resource_mut::<NodeManager>()
            .selection_generation = generation.wrapping_add(1);
        assert!(
            observe(&mut app).is_none(),
            "the retained origin must match the exact current selection generation"
        );
        app.world_mut()
            .resource_mut::<NodeManager>()
            .selection_generation = generation;
        app.world_mut()
            .resource_mut::<NodeManager>()
            .pending_sidebar_select = Some("n1".into());
        assert!(
            observe(&mut app).is_none(),
            "a queued replacement cannot leave an observable stamp selection"
        );
        app.world_mut()
            .resource_mut::<NodeManager>()
            .pending_sidebar_select = None;
        app.world_mut()
            .entity_mut(stamp)
            .remove::<crate::verse_manager::PathAssetInstance>();
        assert!(
            observe(&mut app).is_none(),
            "a marker string without an actual typed instance is insufficient"
        );
        app.world_mut()
            .entity_mut(stamp)
            .insert(crate::verse_manager::PathAssetInstance {
                source_track_id: "t1".into(),
                petal_id: "p1".into(),
            });
        app.world_mut().entity_mut(stamp).insert(Visibility::Hidden);
        assert!(
            observe(&mut app).is_none(),
            "hidden rendered instances are not observations"
        );
        app.world_mut()
            .entity_mut(stamp)
            .insert(Visibility::Inherited);
        crate::actions::asset::hydrate_promoted_stamp(
            "t1#inst-1",
            &serde_json::json!({"node_kind":"stamp","path_id":"t1","instance_index":1}),
            &mut app
                .world_mut()
                .resource_mut::<crate::actions::asset::StampInteractionState>(),
        );
        assert!(
            observe(&mut app).is_none(),
            "canonical text cannot replace the complete loaded property binding"
        );
        crate::actions::asset::hydrate_promoted_stamp(
            "t1#inst-1",
            &serde_json::json!({"node_kind":"stamp","path_id":"t1","owning_path_id":"t1","instance_index":1}),
            &mut app
                .world_mut()
                .resource_mut::<crate::actions::asset::StampInteractionState>(),
        );
        assert_eq!(observe(&mut app).unwrap().4, rendered_transform);
        app.world_mut().entity_mut(stamp).insert(original_transform);
        native(vec![], &mut time, &mut app);
        assert!(
            app.world()
                .resource::<crate::panels::tool_panel::ToolPanelState>()
                .stamp_edit_target
                .is_some(),
            "actual selection and receipt load the atomic editor"
        );
        // The loaded selection may request its stored capture; it never promotes again.
        let commands: Vec<_> = stamp_rx.try_iter().collect();
        assert!(
            !commands
                .iter()
                .any(|command| matches!(command, DbCommand::PromoteCheckedStamp { .. })),
            "idle receipt projection cannot re-promote: {commands:?}"
        );
        let observed = native(vec![], &mut time, &mut app);
        let x = observed.controls[observed.controls.len() - 8].hit.center();
        for pressed in [true, false] {
            native(
                vec![
                    egui::Event::PointerMoved(x),
                    egui::Event::PointerButton {
                        pos: x,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
                &mut time,
                &mut app,
            );
        }
        native(
            vec![
                egui::Event::Key {
                    key: egui::Key::A,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers {
                        ctrl: true,
                        command: true,
                        ..Default::default()
                    },
                },
                egui::Event::Text("2".into()),
            ],
            &mut time,
            &mut app,
        );
        let observed = native(
            vec![egui::Event::Key {
                key: egui::Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            &mut time,
            &mut app,
        );
        let apply = observed.controls[observed.controls.len() - 3].hit.center();
        for pressed in [true, false] {
            native(
                vec![
                    egui::Event::PointerMoved(apply),
                    egui::Event::PointerButton {
                        pos: apply,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
                &mut time,
                &mut app,
            );
        }
        app.update();
        let commands: Vec<_> = stamp_rx.try_iter().collect();
        assert!(matches!(commands.as_slice(),[DbCommand::CommitStampOverrides {edit,..}] if edit.node_id=="t1#inst-1" && edit.instance_index==1 && edit.scale==[2.0,1.0,1.0]),
            "actual native Apply and production dispatcher emit one atomic stamp batch: {commands:?}");
        let [DbCommand::CommitStampOverrides { correlation_id: reset_save, .. }] = commands.as_slice() else { unreachable!() };
        let reset_save = reset_save.clone();
        let reset_capture = late_capture.clone();
        let reset_request = late_request.clone();
        app.insert_resource(ClickArbiter::pressed_along(Ray3d::new(
            Vec3::new(20.0, 50.0, 0.0),
            Dir3::NEG_Y,
        )));
        app.update();
        app.world_mut().write_message(DbResult::CheckedStampPromoted {capture:late_capture,node_id:"t1#inst-1".into(),
            properties:serde_json::json!({"node_kind":"stamp","path_id":"t1","owning_path_id":"t1","instance_index":1}),newly_promoted:false,correlation_id:late_request});
        app.update();
        assert_eq!(
            app.world().resource::<NodeManager>().selected_entity(),
            Some(ordinary)
        );
        assert!(
            app.world()
                .resource::<crate::actions::asset::StampInteractionState>()
                .selected()
                .is_none(),
            "late promotion properties cannot reopen a stamp after another object was picked"
        );
        assert!(
            mutations(&stamp_rx).is_empty(),
            "late receipts cannot resubmit or promote"
        );

        let reset_stamp = crate::actions::asset::StampRef { track_node_id: "t1".into(), stamp_index: 1 };
        let reset_origin = app.world().resource::<crate::node_manager::StampSelectionCatalog>().origin(1).unwrap();
        {
            let state = app.world().resource::<crate::actions::asset::StampInteractionState>();
            assert!(state.loaded_properties(&reset_stamp).is_some());
            assert_eq!(state.promoted_node_id(&reset_stamp), Some("t1#inst-1"));
            assert!(state.edit_pending(), "the actual native save is still awaiting its receipt");
        }
        assert!(app.world().resource::<crate::panels::tool_panel::ToolPanelState>().stamp_selection_catalog.is_some());
        app.world_mut().resource_mut::<crate::node_manager::StampSelectionRequests>().push(reset_origin);
        crate::maintenance::clear_read_models(app.world_mut());
        let catalog = app.world().resource::<crate::node_manager::StampSelectionCatalog>();
        assert!(catalog.targets.is_empty() && catalog.origin(1).is_none());
        assert!(app.world().resource::<crate::panels::tool_panel::ToolPanelState>().stamp_selection_catalog.is_none());
        assert!(app.world().resource::<NodeManager>().selected_entity().is_none());
        {
            let state = app.world().resource::<crate::actions::asset::StampInteractionState>();
            assert!(state.selected().is_none() && state.promoted_node_id(&reset_stamp).is_none());
            assert!(state.loaded_properties(&reset_stamp).is_none() && !state.edit_pending());
        }
        app.insert_resource(ClickArbiter::default());
        app.world_mut().write_message(DbResult::CheckedStampPromoted { capture: reset_capture,
            node_id: "t1#inst-1".into(), properties: serde_json::json!({"node_kind":"stamp","path_id":"t1","owning_path_id":"t1","instance_index":1}),
            newly_promoted: false, correlation_id: reset_request });
        app.update();
        app.world_mut().resource_scope(|world, mut state: Mut<crate::actions::asset::StampInteractionState>| {
            crate::actions::asset::handle_edit_reply(&mut state,
                &DbResult::RequestFailed { correlation_id: reset_save, error: "late save refusal".into() },
                &mut world.resource_mut::<crate::actions::UiManager>(), 0.0);
        });
        let state = app.world().resource::<crate::actions::asset::StampInteractionState>();
        assert!(state.selected().is_none() && state.loaded_properties(&reset_stamp).is_none());
        assert!(!state.edit_pending() && state.edit_error().is_none() && state.saved_revision() == 0);
        assert!(app.world().resource::<NodeManager>().selected_entity().is_none());
        assert!(mutations(&stamp_rx).is_empty(), "reset invalidation and late replies cannot enqueue another mutation");
    }

    /// A unit box centered at the origin in identity local space.
    fn unit_box() -> (Affine3A, Vec3, Vec3) {
        (Affine3A::IDENTITY, Vec3::ZERO, Vec3::splat(0.5))
    }

    #[test]
    fn hits_box_straight_on() {
        let (aff, c, he) = unit_box();
        // Ray from z=+5 looking toward -z straight at the box center.
        let t = ray_aabb_hit(Vec3::new(0.0, 0.0, 5.0), -Vec3::Z, aff, c, he);
        // Enters the front face at z = 0.5, i.e. 4.5 units along the ray.
        assert!(t.is_some());
        assert!((t.unwrap() - 4.5).abs() < 1e-4, "t = {:?}", t);
    }

    #[test]
    fn misses_box_beside_it() {
        let (aff, c, he) = unit_box();
        // Parallel ray offset well outside the box on x.
        let t = ray_aabb_hit(Vec3::new(5.0, 0.0, 5.0), -Vec3::Z, aff, c, he);
        assert!(t.is_none(), "expected miss, got {:?}", t);
    }

    #[test]
    fn misses_box_behind_origin() {
        let (aff, c, he) = unit_box();
        // Ray points AWAY from the box (+z) so the box is entirely behind.
        let t = ray_aabb_hit(Vec3::new(0.0, 0.0, 5.0), Vec3::Z, aff, c, he);
        assert!(t.is_none(), "expected miss (box behind), got {:?}", t);
    }

    #[test]
    fn origin_inside_box_returns_zero() {
        let (aff, c, he) = unit_box();
        let t = ray_aabb_hit(Vec3::ZERO, Vec3::X, aff, c, he);
        assert_eq!(t, Some(0.0));
    }

    #[test]
    fn respects_translation_offset() {
        // Box translated to x = 10; a ray from the origin along +x should hit
        // its near face at t = 9.5 (box spans x ∈ [9.5, 10.5]).
        let aff = Affine3A::from_translation(Vec3::new(10.0, 0.0, 0.0));
        let t = ray_aabb_hit(Vec3::ZERO, Vec3::X, aff, Vec3::ZERO, Vec3::splat(0.5));
        assert!(t.is_some());
        assert!((t.unwrap() - 9.5).abs() < 1e-4, "t = {:?}", t);
    }

    #[test]
    fn respects_scale() {
        // A 10× scaled unit box spans x ∈ [-5, 5]; ray from x=+20 toward -x
        // enters at x = 5, i.e. 15 units along the ray.
        let aff = Affine3A::from_scale(Vec3::splat(10.0));
        let t = ray_aabb_hit(
            Vec3::new(20.0, 0.0, 0.0),
            -Vec3::X,
            aff,
            Vec3::ZERO,
            Vec3::splat(0.5),
        );
        assert!(t.is_some());
        assert!((t.unwrap() - 15.0).abs() < 1e-3, "t = {:?}", t);
    }

    #[test]
    fn picks_nearest_of_two_hits() {
        // Two boxes along the ray: nearer one at z=0, farther at z=-10. The
        // helper reports each box's own entry t; the system keeps the min.
        let near = ray_aabb_hit(
            Vec3::new(0.0, 0.0, 5.0),
            -Vec3::Z,
            Affine3A::IDENTITY,
            Vec3::ZERO,
            Vec3::splat(0.5),
        );
        let far = ray_aabb_hit(
            Vec3::new(0.0, 0.0, 5.0),
            -Vec3::Z,
            Affine3A::from_translation(Vec3::new(0.0, 0.0, -10.0)),
            Vec3::ZERO,
            Vec3::splat(0.5),
        );
        assert!(near.unwrap() < far.unwrap());
    }

    // --- descendant traversal (the GLB-selection depth fix) ---

    use std::collections::HashMap;

    fn ent(n: u64) -> Entity {
        Entity::from_bits(n)
    }

    #[test]
    fn subtree_finds_hit_on_a_deep_grandchild() {
        // root(no geometry) → child(no geometry) → grandchild(hit @ t=5). glTF
        // SceneRoots put the Aabb this deep; the old immediate-children scan
        // returned None here and deselected every GLB. The DFS must find it.
        let children: HashMap<Entity, Vec<Entity>> =
            HashMap::from([(ent(1), vec![ent(2)]), (ent(2), vec![ent(3)])]);
        let hits: HashMap<Entity, f32> = HashMap::from([(ent(3), 5.0)]);
        let t = nearest_in_subtree(
            ent(1),
            |e| hits.get(&e).copied(),
            |e| children.get(&e).cloned().unwrap_or_default(),
        );
        assert_eq!(t, Some(5.0));
    }

    #[test]
    fn subtree_keeps_the_closest_hit_across_depths() {
        // Immediate child hits at t=9, its own child (grandchild) at t=3 — the
        // nearer surface, even though it's deeper, must win.
        let children: HashMap<Entity, Vec<Entity>> =
            HashMap::from([(ent(1), vec![ent(2)]), (ent(2), vec![ent(3)])]);
        let hits: HashMap<Entity, f32> = HashMap::from([(ent(2), 9.0), (ent(3), 3.0)]);
        let t = nearest_in_subtree(
            ent(1),
            |e| hits.get(&e).copied(),
            |e| children.get(&e).cloned().unwrap_or_default(),
        );
        assert_eq!(t, Some(3.0));
    }

    #[test]
    fn subtree_is_none_when_no_entity_has_geometry() {
        let children: HashMap<Entity, Vec<Entity>> =
            HashMap::from([(ent(1), vec![ent(2), ent(3)])]);
        let t = nearest_in_subtree(
            ent(1),
            |_| None,
            |e| children.get(&e).cloned().unwrap_or_default(),
        );
        assert_eq!(t, None);
    }
}
