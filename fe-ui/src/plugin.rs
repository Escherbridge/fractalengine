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

#[derive(Resource, Default)]
pub struct VerseHierarchy {
    pub verses: Vec<VerseEntry>,
}

#[derive(Clone, Default)]
pub struct VerseEntry {
    pub id: String,
    pub name: String,
    /// Phase E: hex-encoded namespace ID for iroh-docs replica.
    pub namespace_id: Option<String>,
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

// ---------------------------------------------------------------------------
// Phase F: Invite/Join dialog state
// ---------------------------------------------------------------------------

/// State for the "Generate Invite" dialog.
#[derive(Resource, Default)]
pub struct InviteDialogState {
    pub open: bool,
    pub invite_string: String,
    /// Whether to include write capability (namespace secret) in the invite.
    pub include_write_cap: bool,
    /// Expiry in hours (default 24).
    pub expiry_hours: u32,
}

/// State for the "Join Verse" dialog.
#[derive(Resource, Default)]
pub struct JoinDialogState {
    pub open: bool,
    pub invite_buf: String,
}

/// Phase F: whether the peer debug panel is visible.
#[derive(Resource, Default)]
pub struct PeerDebugPanelState {
    pub open: bool,
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

    #[test]
    fn viewport_cursor_world_default_is_none() {
        let cursor = ViewportCursorWorld::default();
        assert!(cursor.pos.is_none());
    }

    #[test]
    fn context_menu_captures_world_pos() {
        // Simulate the context menu capturing a non-zero world position.
        let cursor_world = ViewportCursorWorld {
            pos: Some([5.0, 0.0, -3.0]),
        };
        let mut ctx_menu = ContextMenuState::default();
        // Mimics the right-click handler in viewport_overlay.
        ctx_menu.world_pos = cursor_world.pos.unwrap_or([0.0, 0.0, 0.0]);
        assert_eq!(ctx_menu.world_pos, [5.0, 0.0, -3.0]);
    }

    #[test]
    fn context_menu_falls_back_to_origin_when_cursor_none() {
        let cursor_world = ViewportCursorWorld { pos: None };
        let mut ctx_menu = ContextMenuState::default();
        ctx_menu.world_pos = cursor_world.pos.unwrap_or([0.0, 0.0, 0.0]);
        assert_eq!(ctx_menu.world_pos, [0.0, 0.0, 0.0]);
    }

    #[test]
    fn gltf_import_state_carries_position_through() {
        let mut gltf = GltfImportState::default();
        // Simulates the context menu setting the position before opening the dialog.
        let world = [7.5, 0.0, -2.1];
        gltf.position = world;
        gltf.open = true;
        assert_eq!(gltf.position, [7.5, 0.0, -2.1]);
    }

    #[test]
    fn gltf_import_default_position_is_origin() {
        let gltf = GltfImportState::default();
        assert_eq!(gltf.position, [0.0, 0.0, 0.0]);
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
        app.init_resource::<InviteDialogState>();
        app.init_resource::<JoinDialogState>();
        app.init_resource::<PeerDebugPanelState>();
        app.add_systems(EguiPrimaryContextPass, gardener_ui_system);
        app.add_systems(
            Update,
            (
                apply_db_results_to_hierarchy,
                apply_camera_focus,
                despawn_and_spawn_on_petal_change,
                strip_gltf_embedded_cameras,
                update_viewport_cursor_world,
                auto_open_close_verse_replica,
            ),
        );
    }
}

/// Phase F: bundle of P2P dialog state to avoid exceeding Bevy's 16-param limit.
#[derive(bevy::ecs::system::SystemParam)]
struct P2pDialogParams<'w> {
    invite_dialog: ResMut<'w, InviteDialogState>,
    join_dialog: ResMut<'w, JoinDialogState>,
    sync_status: Option<Res<'w, fe_sync::SyncStatus>>,
    peer_debug: ResMut<'w, PeerDebugPanelState>,
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
    mut p2p: P2pDialogParams,
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
        &mut p2p.invite_dialog,
        &mut p2p.join_dialog,
        p2p.sync_status.as_deref(),
        &mut p2p.peer_debug,
    );
    let role_label = if inspector.is_admin {
        "admin"
    } else {
        "member"
    };
    role_chip::role_chip_hud(ectx, role_label);
}

fn apply_db_results_to_hierarchy(
    mut reader: MessageReader<DbResult>,
    mut hierarchy: ResMut<VerseHierarchy>,
    mut nav: ResMut<NavigationState>,
    db_sender: Res<fe_runtime::app::DbCommandSender>,
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    mut invite_dialog: ResMut<InviteDialogState>,
    sync_sender: Option<Res<fe_sync::SyncCommandSenderRes>>,
) {
    for result in reader.read() {
        match result {
            DbResult::Seeded { .. } => {
                // DB seed complete — load the full hierarchy from the database
                db_sender
                    .0
                    .send(fe_runtime::messages::DbCommand::LoadHierarchy)
                    .ok();
                continue;
            }
            DbResult::GltfImported {
                node_id,
                name,
                petal_id,
                asset_path,
                position,
                ..
            } => {
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
                    let spawned = commands
                        .spawn((
                            SceneRoot(gltf_handle),
                            Transform::from_xyz(position[0], position[1], position[2]),
                            Name::new(name.clone()),
                            SpawnedNodeMarker {
                                node_id: node_id.clone(),
                                petal_id: petal_id.clone(),
                            },
                        ))
                        .id();
                    bevy::log::debug!(
                        "Spawned GLTF scene '{}' entity={:?} at {:?} (asset={}, petal={})",
                        name,
                        spawned,
                        position,
                        asset_path,
                        petal_id
                    );
                } else {
                    bevy::log::debug!(
                        "Skipped spawn of '{}' (petal {} is not active: {:?})",
                        name,
                        petal_id,
                        nav.active_petal_id
                    );
                }
            }
            DbResult::NodeCreated {
                id,
                petal_id,
                name,
                has_asset,
            } => {
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

                // Auto-select verse and auto-nav to populated petal BEFORE the
                // hierarchy rebuild so the spawn filter uses the correct petal
                // on this same HierarchyLoaded event (single round-trip).
                if was_first_load && nav.active_petal_id.is_none() {
                    // Auto-select the first verse.
                    if let Some(v) = verses.first() {
                        nav.active_verse_id = Some(v.id.clone());
                        nav.active_verse_name = v.name.clone();
                    }
                    // Auto-nav to the first petal containing an asset node.
                    'outer_pre: for v in verses.iter() {
                        for f in &v.fractals {
                            for p in &f.petals {
                                if p.nodes.iter().any(|n| n.asset_path.is_some()) {
                                    nav.active_fractal_id = Some(f.id.clone());
                                    nav.active_fractal_name = f.name.clone();
                                    nav.active_petal_id = Some(p.id.clone());
                                    bevy::log::info!(
                                        "Auto-navigated to populated petal: {}/{}",
                                        f.name,
                                        p.name
                                    );
                                    break 'outer_pre;
                                }
                            }
                        }
                    }
                }

                let effective_petal = nav.active_petal_id.clone();
                hierarchy.verses = verses.iter().map(|v| VerseEntry {
                    id: v.id.clone(),
                    name: v.name.clone(),
                    namespace_id: v.namespace_id.clone(),
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
                                let should_spawn = effective_petal.as_deref() == Some(n.petal_id.as_str())
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
                                    bevy::log::debug!(
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
                // Non-first-load: auto-select verse if still unset (e.g. after a CreateVerse).
                if nav.active_verse_id.is_none() {
                    if let Some(v) = hierarchy.verses.first() {
                        nav.active_verse_id = Some(v.id.clone());
                        nav.active_verse_name = v.name.clone();
                    }
                }
            }
            DbResult::VerseCreated { id, name } => {
                hierarchy.verses.push(VerseEntry {
                    id: id.clone(),
                    name: name.clone(),
                    namespace_id: None, // Will be populated on next LoadHierarchy
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
            DbResult::PetalCreated {
                id,
                fractal_id,
                name,
            } => {
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
            DbResult::VerseInviteGenerated { invite_string, .. } => {
                invite_dialog.invite_string = invite_string.clone();
                invite_dialog.open = true;
            }
            DbResult::VerseJoined {
                verse_id,
                verse_name,
            } => {
                bevy::log::info!("Joined verse: {verse_name} ({verse_id})");
                // Open the replica for the joined verse.
                if let Some(ref sync_sender) = sync_sender {
                    let ns_secret = fe_database::get_namespace_secret_from_keyring(verse_id)
                        .ok()
                        .flatten();
                    // We need namespace_id — for now use the verse_id to look it up.
                    // The verse was just created with a namespace_id. We'll get it
                    // on the next LoadHierarchy. For immediate effect, schedule it.
                    // The auto_open_close_verse_replica system will handle opening
                    // when the user navigates to this verse.
                    let _ = sync_sender; // suppress unused warning
                    let _ = ns_secret;
                }
                // Refresh hierarchy to show the new verse.
                db_sender
                    .0
                    .send(fe_runtime::messages::DbCommand::LoadHierarchy)
                    .ok();
            }
            DbResult::DatabaseReset { .. } => {
                bevy::log::info!("Database reset complete — reloading hierarchy");
                // Clear in-memory hierarchy and despawn all scene entities
                hierarchy.verses.clear();
                // Reload from fresh DB
                db_sender
                    .0
                    .send(fe_runtime::messages::DbCommand::LoadHierarchy)
                    .ok();
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
        *last,
        new_active
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
        bevy::log::debug!(
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
/// Phase E: automatically open/close verse replicas when the active verse changes.
///
/// Watches `NavigationState::active_verse_id` for changes and sends
/// `CloseVerseReplica` for the previous verse and `OpenVerseReplica` for the
/// new one to the sync thread.
fn auto_open_close_verse_replica(
    nav: Res<NavigationState>,
    hierarchy: Res<VerseHierarchy>,
    sync_sender: Option<Res<fe_sync::SyncCommandSenderRes>>,
    mut last_verse: Local<Option<String>>,
    mut initialized: Local<bool>,
) {
    let Some(sync_sender) = sync_sender else {
        return;
    };

    if !*initialized {
        *last_verse = nav.active_verse_id.clone();
        *initialized = true;
        // If we already have an active verse on init, open its replica.
        if let Some(ref vid) = nav.active_verse_id {
            if let Some(ns_id) = find_namespace_id(&hierarchy, vid) {
                // Try to retrieve the namespace secret from the keyring.
                let ns_secret = fe_database::get_namespace_secret_from_keyring(vid)
                    .ok()
                    .flatten();
                sync_sender
                    .0
                    .send(fe_sync::SyncCommand::OpenVerseReplica {
                        verse_id: vid.clone(),
                        namespace_id: ns_id,
                        namespace_secret: ns_secret,
                    })
                    .ok();
            }
        }
        return;
    }

    if *last_verse == nav.active_verse_id {
        return;
    }

    // Close the old replica.
    if let Some(ref old_vid) = *last_verse {
        sync_sender
            .0
            .send(fe_sync::SyncCommand::CloseVerseReplica {
                verse_id: old_vid.clone(),
            })
            .ok();
    }

    // Open the new replica.
    if let Some(ref new_vid) = nav.active_verse_id {
        if let Some(ns_id) = find_namespace_id(&hierarchy, new_vid) {
            let ns_secret = fe_database::get_namespace_secret_from_keyring(new_vid)
                .ok()
                .flatten();
            sync_sender
                .0
                .send(fe_sync::SyncCommand::OpenVerseReplica {
                    verse_id: new_vid.clone(),
                    namespace_id: ns_id,
                    namespace_secret: ns_secret,
                })
                .ok();
        }
    }

    *last_verse = nav.active_verse_id.clone();
}

/// Look up the namespace_id for a verse in the hierarchy.
fn find_namespace_id(hierarchy: &VerseHierarchy, verse_id: &str) -> Option<String> {
    hierarchy
        .verses
        .iter()
        .find(|v| v.id == verse_id)
        .and_then(|v| v.namespace_id.clone())
}

fn update_viewport_cursor_world(
    windows: Query<&Window>,
    cameras: Query<(&Camera, &GlobalTransform), With<fe_renderer::camera::OrbitCameraController>>,
    mut cursor_world: ResMut<ViewportCursorWorld>,
    mut egui_ctx: EguiContexts,
) {
    // Only suppress the world cursor when egui is actively consuming pointer
    // input (e.g. dragging a slider, clicking a button).  The old check used
    // `is_pointer_over_area()` which returns true for the transparent
    // CentralPanel (the 3-D viewport), so the world position was *never*
    // computed and every placed model landed at 0,0,0.
    let Ok(ectx) = egui_ctx.ctx_mut() else {
        cursor_world.pos = None;
        return;
    };
    if ectx.wants_pointer_input() {
        cursor_world.pos = None;
        return;
    }
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
    if let Some(point) =
        ray.plane_intersection_point(ground_origin, InfinitePlane3d::new(ground_normal))
    {
        cursor_world.pos = Some([point.x, 0.0, point.z]);
    } else {
        cursor_world.pos = None;
    }
}
