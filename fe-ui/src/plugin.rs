use crate::{atlas::DashboardState, panels, panels::Tool, role_chip};
use bevy::prelude::*;
use bevy::scene::SceneRoot;
use bevy_egui::{EguiContexts, EguiPrimaryContextPass};
use fe_runtime::messages::{DbCommand, DbResult};

/// Marker attached to every `SceneRoot` spawned from a DB node so that
/// the UI can despawn/refresh scene entities when the active petal changes.
#[derive(Component, Debug, Clone)]
pub struct SpawnedNodeMarker {
    pub node_id: String,
    pub petal_id: String,
}

/// Sidebar visibility and search state.
#[derive(Resource)]
pub struct SidebarState {
    pub open: bool,
    pub tag_filter_buf: String,
}

impl Default for SidebarState {
    fn default() -> Self {
        Self {
            open: true,
            tag_filter_buf: String::new(),
        }
    }
}

/// Currently active editor tool.
#[derive(Resource, Default)]
pub struct ToolState {
    pub active_tool: Tool,
}

/// Inspector panel state: selection, transform buffers, URL fields.
#[derive(Resource)]
pub struct InspectorState {
    pub selected_entity: Option<Entity>,
    pub is_admin: bool,
    pub external_url: String,
    pub config_url: String,
    pub pos: [String; 3],
    pub rot: [String; 3],
    pub scale: [String; 3],
}

impl Default for InspectorState {
    fn default() -> Self {
        Self {
            selected_entity: None,
            is_admin: false,
            external_url: String::new(),
            config_url: String::new(),
            pos: ["0.00".into(), "0.00".into(), "0.00".into()],
            rot: ["0.00".into(), "0.00".into(), "0.00".into()],
            scale: ["1.00".into(), "1.00".into(), "1.00".into()],
        }
    }
}

/// Verse and fractal navigation state.
#[derive(Resource, Default)]
pub struct NavigationState {
    pub active_verse_id: Option<String>,
    pub active_verse_name: String,
    pub active_fractal_id: Option<String>,
    pub active_fractal_name: String,
    pub active_petal_id: Option<String>,
}

// ---------------------------------------------------------------------------
// Verse hierarchy tree data
// ---------------------------------------------------------------------------

#[derive(Resource)]
pub struct VerseHierarchy {
    pub verses: Vec<VerseEntry>,
}

impl Default for VerseHierarchy {
    fn default() -> Self {
        Self {
            verses: Vec::new(),
        }
    }
}

#[derive(Clone, Default)]
pub struct VerseEntry {
    pub id: String,
    pub name: String,
    pub expanded: bool,
    pub fractals: Vec<FractalEntry>,
}

#[derive(Clone, Default)]
pub struct FractalEntry {
    pub id: String,
    pub name: String,
    pub expanded: bool,
    pub petals: Vec<PetalEntry>,
}

#[derive(Clone, Default)]
pub struct PetalEntry {
    pub id: String,
    pub name: String,
    pub expanded: bool,
    pub nodes: Vec<NodeEntry>,
}

#[derive(Clone, Default)]
pub struct NodeEntry {
    pub id: String,
    pub name: String,
    pub has_asset: bool,
    pub position: [f32; 3],
}

// ---------------------------------------------------------------------------
// Create dialog state
// ---------------------------------------------------------------------------

#[derive(Resource, Default)]
pub struct CreateDialogState {
    pub open: bool,
    pub kind: CreateKind,
    pub parent_id: String,
    pub name_buf: String,
}

#[derive(Default, Clone, Copy, PartialEq)]
pub enum CreateKind {
    #[default]
    Verse,
    Fractal,
    Petal,
    Node,
}

// ---------------------------------------------------------------------------
// Context menu state (viewport right-click)
// ---------------------------------------------------------------------------

#[derive(Resource, Default)]
pub struct ContextMenuState {
    pub open: bool,
    pub screen_pos: [f32; 2],
    pub world_pos: [f32; 3],
}

// ---------------------------------------------------------------------------
// Camera focus target (set by sidebar click, consumed by camera system)
// ---------------------------------------------------------------------------

#[derive(Resource, Default)]
pub struct CameraFocusTarget {
    pub target: Option<[f32; 3]>,
}

// ---------------------------------------------------------------------------
// GLTF import dialog state
// ---------------------------------------------------------------------------

#[derive(Resource, Default)]
pub struct GltfImportState {
    pub open: bool,
    pub file_path_buf: String,
    pub name_buf: String,
    pub position: [f32; 3],
}

// ---------------------------------------------------------------------------
// Viewport cursor world position (camera ray → Y=0 plane intersection)
// ---------------------------------------------------------------------------

/// Tracks the current cursor's world-space position projected onto Y=0.
/// Updated every frame by `update_viewport_cursor_world`.
/// Used by the context menu to place imported GLB models at the correct spot.
#[derive(Resource, Default)]
pub struct ViewportCursorWorld {
    pub pos: Option<[f32; 3]>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn navigation_state_default_has_no_active_verse() {
        let state = NavigationState::default();
        assert!(state.active_verse_id.is_none());
        assert!(state.active_fractal_id.is_none());
        assert!(state.active_petal_id.is_none());
    }

    #[test]
    fn spawned_node_marker_carries_both_ids() {
        let marker = SpawnedNodeMarker {
            node_id: "node-abc".to_string(),
            petal_id: "petal-xyz".to_string(),
        };
        let cloned = marker.clone();
        assert_eq!(cloned.node_id, "node-abc");
        assert_eq!(cloned.petal_id, "petal-xyz");
        // Debug derive is exercised so the format succeeds.
        let _ = format!("{marker:?}");
    }
}

pub struct GardenerConsolePlugin;

impl Plugin for GardenerConsolePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SidebarState>();
        app.init_resource::<ToolState>();
        app.init_resource::<InspectorState>();
        app.init_resource::<NavigationState>();
        app.init_resource::<DashboardState>();
        app.init_resource::<VerseHierarchy>();
        app.init_resource::<CreateDialogState>();
        app.init_resource::<ContextMenuState>();
        app.init_resource::<GltfImportState>();
        app.init_resource::<CameraFocusTarget>();
        app.init_resource::<ViewportCursorWorld>();
        app.add_systems(EguiPrimaryContextPass, gardener_ui_system);
        app.add_systems(
            Update,
            (
                apply_db_results_to_hierarchy,
                apply_camera_focus,
                despawn_and_spawn_on_petal_change,
                strip_gltf_embedded_cameras,
                update_viewport_cursor_world,
            ),
        );
    }
}

fn gardener_ui_system(
    mut ctx: EguiContexts,
    mut sidebar: ResMut<SidebarState>,
    mut tool: ResMut<ToolState>,
    mut inspector: ResMut<InspectorState>,
    mut nav: ResMut<NavigationState>,
    dashboard: Res<DashboardState>,
    mut hierarchy: ResMut<VerseHierarchy>,
    mut create_dialog: ResMut<CreateDialogState>,
    mut context_menu: ResMut<ContextMenuState>,
    mut gltf_import: ResMut<GltfImportState>,
    db_sender: Res<fe_runtime::app::DbCommandSender>,
    mut camera_focus: ResMut<CameraFocusTarget>,
    cursor_world: Res<ViewportCursorWorld>,
) {
    let Ok(ectx) = ctx.ctx_mut() else { return };

    panels::gardener_console(
        ectx,
        &mut sidebar,
        &mut tool,
        &mut inspector,
        &mut nav,
        &dashboard,
        &mut hierarchy,
        &mut create_dialog,
        &mut context_menu,
        &mut gltf_import,
        &db_sender.0,
        &mut camera_focus,
        &cursor_world,
    );
    let role_label = if inspector.is_admin { "admin" } else { "member" };
    role_chip::role_chip_hud(ectx, role_label);
}

fn apply_db_results_to_hierarchy(
    mut reader: MessageReader<DbResult>,
    mut hierarchy: ResMut<VerseHierarchy>,
    mut nav: ResMut<NavigationState>,
    db_sender: Res<fe_runtime::app::DbCommandSender>,
    mut commands: Commands,
    asset_server: Res<AssetServer>,
) {
    for result in reader.read() {
        match result {
            DbResult::Seeded { .. } => {
                // DB seed complete — load the full hierarchy from the database
                db_sender.0.send(fe_runtime::messages::DbCommand::LoadHierarchy).ok();
                continue;
            }
            DbResult::GltfImported { node_id, name, petal_id, asset_path, position, .. } => {
                // Insert the new node into the correct petal in the hierarchy
                for verse in hierarchy.verses.iter_mut() {
                    for fractal in verse.fractals.iter_mut() {
                        if let Some(petal) = fractal.petals.iter_mut().find(|p| p.id == *petal_id) {
                            petal.nodes.push(NodeEntry {
                                id: node_id.clone(),
                                name: name.clone(),
                                has_asset: true,
                                position: *position,
                            });
                        }
                    }
                }
                // Only spawn the scene entity if this import targets the currently active petal.
                if nav.active_petal_id.as_deref() == Some(petal_id.as_str()) {
                    let gltf_handle: Handle<Scene> =
                        asset_server.load(format!("{}#Scene0", asset_path));
                    let spawned = commands.spawn((
                        SceneRoot(gltf_handle),
                        Transform::from_xyz(position[0], position[1], position[2]),
                        Name::new(name.clone()),
                        SpawnedNodeMarker {
                            node_id: node_id.clone(),
                            petal_id: petal_id.clone(),
                        },
                    )).id();
                    bevy::log::info!(
                        "Spawned GLTF scene '{}' entity={:?} at {:?} (asset={}, petal={})",
                        name, spawned, position, asset_path, petal_id
                    );
                } else {
                    bevy::log::info!(
                        "Skipped spawn of '{}' (petal {} is not active: {:?})",
                        name, petal_id, nav.active_petal_id
                    );
                }
            }
            DbResult::NodeCreated { id, petal_id, name, has_asset } => {
                for verse in hierarchy.verses.iter_mut() {
                    for fractal in verse.fractals.iter_mut() {
                        if let Some(petal) = fractal.petals.iter_mut().find(|p| p.id == *petal_id) {
                            petal.nodes.push(NodeEntry {
                                id: id.clone(),
                                name: name.clone(),
                                has_asset: *has_asset,
                                position: [0.0, 0.0, 0.0],
                            });
                        }
                    }
                }
            }
            DbResult::HierarchyLoaded { verses } => {
                let was_first_load = nav.active_verse_id.is_none();
                let active_petal = nav.active_petal_id.clone();
                hierarchy.verses = verses.iter().map(|v| VerseEntry {
                    id: v.id.clone(),
                    name: v.name.clone(),
                    expanded: true,
                    fractals: v.fractals.iter().map(|f| FractalEntry {
                        id: f.id.clone(),
                        name: f.name.clone(),
                        expanded: true,
                        petals: f.petals.iter().map(|p| PetalEntry {
                            id: p.id.clone(),
                            name: p.name.clone(),
                            expanded: true,
                            nodes: p.nodes.iter().map(|n| {
                                // Spawn GLTF scenes only for nodes whose petal matches
                                // the active petal. When active_petal_id is None
                                // (verse/fractal browser open) we spawn nothing.
                                let should_spawn = active_petal.as_deref() == Some(n.petal_id.as_str())
                                    && n.asset_path.is_some();
                                if should_spawn {
                                    let ap = n.asset_path.as_ref().unwrap();
                                    let gltf_handle: Handle<Scene> = asset_server.load(format!("{}#Scene0", ap));
                                    let spawned = commands.spawn((
                                        SceneRoot(gltf_handle),
                                        Transform::from_xyz(n.position[0], n.position[1], n.position[2]),
                                        Name::new(n.name.clone()),
                                        SpawnedNodeMarker {
                                            node_id: n.id.clone(),
                                            petal_id: n.petal_id.clone(),
                                        },
                                    )).id();
                                    bevy::log::info!(
                                        "Spawned persisted GLTF '{}' entity={:?} at {:?} (asset={}, petal={})",
                                        n.name, spawned, n.position, ap, n.petal_id
                                    );
                                }
                                NodeEntry {
                                    id: n.id.clone(),
                                    name: n.name.clone(),
                                    has_asset: n.has_asset,
                                    position: n.position,
                                }
                            }).collect(),
                        }).collect(),
                    }).collect(),
                }).collect();
                // Auto-select the first verse if nothing is selected
                if nav.active_verse_id.is_none() {
                    if let Some(v) = hierarchy.verses.first() {
                        nav.active_verse_id = Some(v.id.clone());
                        nav.active_verse_name = v.name.clone();
                    }
                }
                // On the very first load (before any explicit navigation), auto-navigate
                // into the first petal that contains at least one asset node. This ensures
                // persisted models appear immediately after app restart rather than requiring
                // the user to manually re-navigate.
                if was_first_load && nav.active_petal_id.is_none() {
                    'outer: for v in &hierarchy.verses {
                        for f in &v.fractals {
                            for p in &f.petals {
                                if p.nodes.iter().any(|n| n.has_asset) {
                                    nav.active_fractal_id = Some(f.id.clone());
                                    nav.active_fractal_name = f.name.clone();
                                    nav.active_petal_id = Some(p.id.clone());
                                    bevy::log::info!(
                                        "Auto-navigated to populated petal: {}/{}", f.name, p.name
                                    );
                                    break 'outer;
                                }
                            }
                        }
                    }
                }
            }
            DbResult::VerseCreated { id, name } => {
                hierarchy.verses.push(VerseEntry {
                    id: id.clone(),
                    name: name.clone(),
                    expanded: true,
                    fractals: Vec::new(),
                });
            }
            DbResult::FractalCreated { id, verse_id, name } => {
                if let Some(verse) = hierarchy.verses.iter_mut().find(|v| v.id == *verse_id) {
                    verse.fractals.push(FractalEntry {
                        id: id.clone(),
                        name: name.clone(),
                        expanded: true,
                        petals: Vec::new(),
                    });
                }
            }
            DbResult::PetalCreated { id, fractal_id, name } => {
                for verse in hierarchy.verses.iter_mut() {
                    if let Some(fractal) = verse.fractals.iter_mut().find(|f| f.id == *fractal_id) {
                        fractal.petals.push(PetalEntry {
                            id: id.clone(),
                            name: name.clone(),
                            expanded: true,
                            nodes: Vec::new(),
                        });
                    }
                }
            }
            DbResult::Error(msg) => {
                bevy::log::error!("DB error: {msg}");
            }
            _ => {}
        }
    }
}

/// When the active petal changes, despawn every previously-spawned node
/// entity and trigger a fresh `LoadHierarchy` so the spawn path (scoped
/// via `active_petal_id`) re-materialises only the active petal's nodes.
fn despawn_and_spawn_on_petal_change(
    nav: Res<NavigationState>,
    mut last: Local<Option<String>>,
    mut initialized: Local<bool>,
    query: Query<(Entity, &SpawnedNodeMarker)>,
    mut commands: Commands,
    db_sender: Res<fe_runtime::app::DbCommandSender>,
) {
    if !*initialized {
        *last = nav.active_petal_id.clone();
        *initialized = true;
        return;
    }
    if *last == nav.active_petal_id {
        return;
    }
    let new_active = nav.active_petal_id.clone();
    bevy::log::info!(
        "Active petal changed from {:?} to {:?} — despawning stale scene entities",
        *last, new_active
    );
    for (entity, marker) in query.iter() {
        let keep = new_active
            .as_deref()
            .map(|pid| pid == marker.petal_id.as_str())
            .unwrap_or(false);
        if !keep {
            commands.entity(entity).despawn();
        }
    }
    // Re-request hierarchy so the spawn path re-materialises the new petal's nodes.
    db_sender.0.send(DbCommand::LoadHierarchy).ok();
    *last = new_active;
}

/// Bevy's GLTF loader can produce embedded `Camera3d`/`Camera` entities when a
/// `.glb` scene contains a camera node. These become secondary active cameras
/// that render into the same window and cause the "duplicate / ghost image"
/// visual artefact. This system removes any non-orbit camera added during the
/// frame so only the viewport's `OrbitCameraController` remains.
fn strip_gltf_embedded_cameras(
    added: Query<
        Entity,
        (
            Added<Camera>,
            Without<fe_renderer::camera::OrbitCameraController>,
        ),
    >,
    mut commands: Commands,
) {
    for entity in added.iter() {
        bevy::log::info!(
            "Despawning GLB-embedded camera entity={:?} (not the orbit camera)",
            entity
        );
        commands.entity(entity).despawn();
    }
}

fn apply_camera_focus(
    mut focus_target: ResMut<CameraFocusTarget>,
    mut query: Query<&mut fe_renderer::camera::OrbitCameraController>,
) {
    if let Some(pos) = focus_target.target.take() {
        if let Ok(mut controller) = query.single_mut() {
            controller.focus = Vec3::new(pos[0], pos[1], pos[2]);
            controller.distance = 5.0;
        }
    }
}

/// Projects the cursor position onto the Y=0 world plane each frame.
///
/// Uses a camera ray → infinite-plane intersection so the context menu can
/// place newly-imported GLB models at the exact world position the user
/// right-clicked, rather than a meaningless screen-space pixel delta.
fn update_viewport_cursor_world(
    windows: Query<&Window>,
    cameras: Query<(&Camera, &GlobalTransform), With<fe_renderer::camera::OrbitCameraController>>,
    mut cursor_world: ResMut<ViewportCursorWorld>,
) {
    let Ok(window) = windows.single() else {
        cursor_world.pos = None;
        return;
    };
    let Some(cursor) = window.cursor_position() else {
        cursor_world.pos = None;
        return;
    };
    let Ok((camera, cam_tx)) = cameras.single() else {
        cursor_world.pos = None;
        return;
    };
    let Ok(ray) = camera.viewport_to_world(cam_tx, cursor) else {
        cursor_world.pos = None;
        return;
    };
    // Intersect the ray with the infinite Y=0 plane.
    let ground_origin = Vec3::ZERO;
    let ground_normal = Dir3::Y;
    if let Some(point) = ray.plane_intersection_point(ground_origin, InfinitePlane3d::new(ground_normal)) {
        cursor_world.pos = Some([point.x, 0.0, point.z]);
    } else {
        cursor_world.pos = None;
    }
}

