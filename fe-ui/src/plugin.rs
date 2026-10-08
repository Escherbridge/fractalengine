//! Crate entry plugin: registers all fe-ui resources/systems and re-exports
//! the public API. See `fe-ui/src/AGENTS.md` §plugin for the module map and
//! §compat for the re-export shims kept for `fractalengine`/`fe-webview`.

use bevy::prelude::*;
use bevy_egui::{EguiContexts, EguiPrimaryContextPass};
use fe_database::RoleLevel;

use crate::{atlas::DashboardState, panels, panels::toolbar::Tool, role_chip};

// ---------------------------------------------------------------------------
// Compat re-exports — symbols reachable at `fe_ui::plugin::*` before the
// module decomposition. `fractalengine` and `fe-webview` import these paths
// directly and must not need edits.
// ---------------------------------------------------------------------------

pub use crate::actions::{UiAction, UiManager};
pub use crate::dialogs::ActiveDialog;
pub use crate::terrain_map::{
    HexonOp, InstalledTilesetDto, PendingHexonOps, PetalMapState, StorageInfoDto,
};

// ---------------------------------------------------------------------------
// UI-only resources (form buffers, tool state, role cache).
// ---------------------------------------------------------------------------

/// Marker attached to every `SceneRoot` spawned from a DB node so that
/// the UI can despawn/refresh scene entities when the active petal changes.
#[derive(Component, Debug, Clone)]
pub struct SpawnedNodeMarker {
    pub node_id: String,
    pub petal_id: String,
}

/// Marker for entities that should always face the viewport camera — a flat
/// icon quad reads as an "icon", not a solid, when kept parallel to the camera
/// plane. `billboard_face_camera` (fe-ui `node_manager`) rewrites their
/// `Transform.rotation` each frame. Constructable from both fe-ui and
/// `fractalengine::gpx_bridge` (single-point track nodes). See
/// `fe-ui/src/AGENTS.md` §data-icons. `data_icons_20260713`.
#[derive(Component, Debug, Default, Clone, Copy)]
pub struct Billboard;

/// Ceiling on renderable mesh instances before spawners stop and the user is
/// warned — sized under the GPU 2 GiB bind-group limit; see AGENTS.md §mesh-budget.
pub const MAX_MESH_INSTANCES: usize = 2_000_000;

/// App-wide mesh-instance budget: `exceeded` gates all fe-ui spawn paths so a
/// runaway can't grow past the GPU bind-group limit. See AGENTS.md §mesh-budget.
#[derive(Resource, Debug)]
pub struct MeshInstanceBudget {
    pub ceiling: usize,
    pub exceeded: bool,
    pub last_count: usize,
    /// Last count/time emitted by the growth-curve diagnostic log.
    pub last_logged_count: usize,
    pub last_log_at: f64,
    /// Mesh3d entities added/removed since the last diagnostic log line.
    pub churn_added: usize,
    pub churn_removed: usize,
}

impl Default for MeshInstanceBudget {
    fn default() -> Self {
        Self {
            ceiling: MAX_MESH_INSTANCES,
            exceeded: false,
            last_count: 0,
            last_logged_count: 0,
            last_log_at: -2.0,
            churn_added: 0,
            churn_removed: 0,
        }
    }
}

/// `(exceeded, announce)` for a watchdog pass: announce only on the rising edge.
fn watchdog_transition(prev_exceeded: bool, count: usize, ceiling: usize) -> (bool, bool) {
    let exceeded = count > ceiling;
    (exceeded, exceeded && !prev_exceeded)
}

/// Counts `Mesh3d` entities each frame (archetypal filter — cheap) and trips
/// the budget gate + toast when the ceiling is crossed. Runs in `Last` so the
/// gate is set before the next frame's spawners. See AGENTS.md §mesh-budget.
fn mesh_instance_watchdog(
    meshes: Query<(), With<Mesh3d>>,
    scene_roots: Query<(), With<bevy::scene::SceneRoot>>,
    node_markers: Query<(), With<SpawnedNodeMarker>>,
    stamps: Query<(), With<crate::verse_manager::spawn::PathAssetInstance>>,
    added: Query<Option<&Name>, Added<Mesh3d>>,
    mut removed: RemovedComponents<Mesh3d>,
    mut budget: ResMut<MeshInstanceBudget>,
    mut ui_mgr: ResMut<UiManager>,
    time: Res<Time>,
) {
    let count = meshes.iter().len();
    let (exceeded, announce) = watchdog_transition(budget.exceeded, count, budget.ceiling);
    // Churn tracking: per-frame Mesh3d add/remove accumulate between log
    // intervals — steady churn leaks GPU mesh-input slots even at a flat
    // entity count (see AGENTS.md §mesh-budget).
    let added_names: Vec<String> = added
        .iter()
        .take(3)
        .map(|n| n.map(|n| n.as_str().to_string()).unwrap_or_default())
        .collect();
    budget.churn_added += added.iter().count();
    budget.churn_removed += removed.read().count();
    // Growth-curve diagnostic: log every ~2s, and immediately on any jump >10k
    // since the last logged value (crash forensics — see AGENTS.md §mesh-budget).
    let elapsed = time.elapsed_secs_f64();
    let jumped = count.abs_diff(budget.last_logged_count) > 10_000;
    if jumped || elapsed - budget.last_log_at >= 2.0 {
        bevy::log::info!(
            "mesh instances: {count} (ceiling {}) | scene_roots {} nodes {} stamps {} | churn +{}/-{} sample {:?}",
            budget.ceiling,
            scene_roots.iter().len(),
            node_markers.iter().len(),
            stamps.iter().len(),
            budget.churn_added,
            budget.churn_removed,
            added_names
        );
        budget.last_log_at = elapsed;
        budget.last_logged_count = count;
        budget.churn_added = 0;
        budget.churn_removed = 0;
    }
    budget.last_count = count;
    if announce {
        bevy::log::error!(
            "mesh instance watchdog: {} instances > ceiling {} — further scene spawning halted",
            count,
            budget.ceiling
        );
        ui_mgr.show_toast(
            format!(
                "Scene too large: {count} mesh instances (limit {}). Spawning paused.",
                budget.ceiling
            ),
            time.elapsed_secs_f64(),
        );
    }
    budget.exceeded = exceeded;
}

/// Sidebar visibility state.
#[derive(Resource)]
pub struct SidebarState {
    pub open: bool,
}

impl Default for SidebarState {
    fn default() -> Self {
        Self { open: true }
    }
}

/// Currently active editor tool. `activate` is its SINGLE writer.
#[derive(Resource, Default)]
pub struct ToolState {
    pub active_tool: Tool,
}

impl ToolState {
    /// The SINGLE tool-activation writer (D1/D9). Re-pressing the already-active
    /// non-Select tool toggles back to `Select` (the neutral rest tool); anything
    /// else switches to `tool`. Then it writes a ONE-SHOT reveal of the
    /// `Options` surface — same rule for every tool, at the activation edge
    /// only. Never re-assert `right.requested` per frame: that would pin the
    /// panel and turn tool activation into a mode.
    ///
    /// This writes the enum directly rather than queueing
    /// `UiAction::RevealSection` (D11): it already holds `&mut
    /// RightSidebarState`, and the reveal must land on the SAME frame as the
    /// activation edge — an action round-trip would arrive a frame later and,
    /// worse, could re-assert the request after the user had toggled it away.
    ///
    /// Landing on Select UN-REVEALS, it does not close (finding F9): it clears
    /// `requested` only when it is still the `Options` section this activation
    /// path opened. A section the USER opened — Maps, Settings, the Inspector —
    /// is never touched, because D9 ratified the reveal, not a blanket close.
    pub fn activate(
        &mut self,
        tool: Tool,
        right: &mut crate::ui_shell::right_sidebar::RightSidebarState,
    ) {
        use crate::ui_shell::right_sidebar::RightSidebarSection;

        self.active_tool = if self.active_tool == tool && tool != Tool::Select {
            Tool::Select
        } else {
            tool
        };
        if self.active_tool == Tool::Select {
            if right.requested == Some(RightSidebarSection::Options) {
                right.requested = None;
            }
        } else {
            right.reveal(RightSidebarSection::Options);
        }
    }
}

/// Which tab is active in the inspector panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InspectorTab {
    #[default]
    Properties,
    ApiAccess,
    Query,
}

/// Default page size for paginated API token listings.
pub const API_TOKEN_PAGE_SIZE: u32 = 20;

/// A single field definition (property schema entry) for display in the inspector.
#[derive(Debug, Clone)]
pub struct FieldDefEntry {
    pub field_def_id: String,
    pub key: String,
    pub value_type: String,
    pub description: String,
    pub required: bool,
    pub default_val: Option<serde_json::Value>,
}

/// Inspector panel state: form buffers for transform editing & URL fields.
/// Selection state lives in [`crate::node_manager::NodeManager`] — this
/// resource only holds the mutable text buffers that the egui widgets edit.
#[derive(Resource)]
pub struct InspectorFormState {
    pub active_tab: InspectorTab,
    pub external_url: String,
    pub config_url: String,
    pub pos: [String; 3],
    pub rot: [String; 3],
    pub scale: [String; 3],
    // Real-unit display buffers (FR-2, inspector_units_width_20260716): position
    // and asset size in meters, kept in sync by
    // `panels::inspector::sync_inspector_units`. Edits live-write the converted
    // world/scale values back into `pos`/`scale` above (Apply path unchanged).
    pub pos_m: [String; 3],
    pub size_m: [String; 3],
    /// Selected node's combined child-AABB extents at scale 1 (world units);
    /// `None` when nothing pickable is spawned. Basis for the size↔scale row.
    pub base_extents: Option<[f32; 3]>,
    /// Active petal's `world_scale` mirrored for panel unit conversions (world
    /// units per meter; ≤0 / non-finite is treated as 1.0).
    pub world_scale: f64,
    // API Access tab state
    pub api_token_scope_buf: String,
    pub api_token_role_buf: String,
    pub api_token_expiry_buf: u32,
    pub generated_api_token: Option<String>,
    pub api_tokens: Vec<crate::dialogs::ApiTokenEntry>,
    pub api_tokens_loading: bool,
    pub api_tokens_page: u32,
    pub api_tokens_total: u64,
    // Query tab state
    pub query_sql_buf: String,
    pub query_result: Option<String>,
    pub query_loading: bool,
    // Property value editing state
    pub node_properties: serde_json::Value,
    pub node_properties_loading: bool,
    pub prop_add_key_buf: String,
    pub prop_add_value_buf: String,
    pub prop_add_type_buf: String,
    // Annotation card state (gis.annotation.* reserved property editor)
    pub annotation_title_buf: String,
    pub annotation_body_buf: String,
    pub annotation_color_buf: String,
    // Field definition (schema) editing state
    pub field_defs: Vec<FieldDefEntry>,
    pub field_defs_loading: bool,
    pub field_def_add_key_buf: String,
    pub field_def_add_type_buf: String,
    pub field_def_add_desc_buf: String,
    pub field_def_add_required: bool,
}

impl Default for InspectorFormState {
    fn default() -> Self {
        Self {
            active_tab: InspectorTab::Properties,
            external_url: String::new(),
            config_url: String::new(),
            pos: ["0.00".into(), "0.00".into(), "0.00".into()],
            rot: ["0.00".into(), "0.00".into(), "0.00".into()],
            scale: ["1.00".into(), "1.00".into(), "1.00".into()],
            pos_m: ["0.000".into(), "0.000".into(), "0.000".into()],
            size_m: ["0.000".into(), "0.000".into(), "0.000".into()],
            base_extents: None,
            world_scale: 1.0,
            api_token_scope_buf: String::new(),
            api_token_role_buf: "viewer".into(),
            api_token_expiry_buf: 720,
            generated_api_token: None,
            api_tokens: Vec::new(),
            api_tokens_loading: false,
            api_tokens_page: 0,
            api_tokens_total: 0,
            query_sql_buf: String::new(),
            query_result: None,
            query_loading: false,
            node_properties: serde_json::Value::Object(Default::default()),
            node_properties_loading: false,
            prop_add_key_buf: String::new(),
            prop_add_value_buf: String::new(),
            prop_add_type_buf: "string".into(),
            annotation_title_buf: String::new(),
            annotation_body_buf: String::new(),
            annotation_color_buf: String::new(),
            field_defs: Vec::new(),
            field_defs_loading: false,
            field_def_add_key_buf: String::new(),
            field_def_add_type_buf: "string".into(),
            field_def_add_desc_buf: String::new(),
            field_def_add_required: false,
        }
    }
}

/// The local user's resolved role at the current scope.
/// Populated by a system that queries RoleManager.
#[derive(Resource, Debug, Default)]
pub struct LocalUserRole {
    pub role: Option<RoleLevel>,
}

impl LocalUserRole {
    /// Check if the local user can manage (assign roles, create entities).
    pub fn can_manage(&self) -> bool {
        self.role.is_some_and(|r| r.can_manage())
    }

    /// Check if the local user can edit content.
    pub fn can_edit(&self) -> bool {
        self.role.is_some_and(|r| r.can_edit())
    }
}

// ---------------------------------------------------------------------------
// Camera focus target (set by sidebar click, consumed by camera system)
// ---------------------------------------------------------------------------

/// `(node_id, fallback position)`. `apply_camera_focus` (camera_focus_clip_20260716
/// FR-2) resolves the live spawned entity's `GlobalTransform` for `node_id` first
/// and only falls back to the cached position — avoids the stale-origin
/// two-step teleport on freshly created nodes.
#[derive(Resource, Default)]
pub struct CameraFocusTarget {
    pub target: Option<(String, [f32; 3])>,
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

/// The egui screen-space rect of the 3-D viewport (CentralPanel).
/// Updated every frame by `gardener_ui_system` and read by the gimbal pick
/// system to reject clicks that land inside sidebar / inspector panels.
#[derive(Resource)]
pub struct ViewportRect(pub bevy_egui::egui::Rect);

impl Default for ViewportRect {
    fn default() -> Self {
        Self(bevy_egui::egui::Rect::EVERYTHING)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn navigation_manager_default_has_no_active_verse() {
        let state = crate::navigation_manager::NavigationManager::default();
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
    fn watchdog_under_ceiling_is_quiet() {
        assert_eq!(watchdog_transition(false, 100, 2_000_000), (false, false));
        // At exactly the ceiling: still within budget.
        assert_eq!(
            watchdog_transition(false, 2_000_000, 2_000_000),
            (false, false)
        );
    }

    #[test]
    fn watchdog_announces_only_on_rising_edge() {
        // Crossing the ceiling announces once…
        assert_eq!(
            watchdog_transition(false, 2_000_001, 2_000_000),
            (true, true)
        );
        // …and stays silent while it remains exceeded.
        assert_eq!(
            watchdog_transition(true, 2_000_001, 2_000_000),
            (true, false)
        );
    }

    #[test]
    fn watchdog_recovers_when_count_drops() {
        assert_eq!(watchdog_transition(true, 10, 2_000_000), (false, false));
    }

    #[test]
    fn mesh_budget_default_uses_named_ceiling() {
        let b = MeshInstanceBudget::default();
        assert_eq!(b.ceiling, MAX_MESH_INSTANCES);
        assert!(!b.exceeded);
    }

    // --- ToolState::activate — the single tool-activation writer (D1/D9) ---

    use crate::ui_shell::right_sidebar::{RightSidebarSection, RightSidebarState};

    #[test]
    fn activate_switches_to_a_different_tool() {
        let (mut tool, mut right) = (ToolState::default(), RightSidebarState::default());
        tool.activate(Tool::Pen, &mut right);
        assert_eq!(tool.active_tool, Tool::Pen);
    }

    #[test]
    fn re_press_toggles_the_active_tool_off_to_select() {
        let (mut tool, mut right) = (ToolState::default(), RightSidebarState::default());
        for t in [
            Tool::Move,
            Tool::Rotate,
            Tool::Scale,
            Tool::Pen,
            Tool::Brush,
        ] {
            tool.activate(t, &mut right);
            assert_eq!(tool.active_tool, t, "{t:?} should activate");
            tool.activate(t, &mut right);
            assert_eq!(
                tool.active_tool,
                Tool::Select,
                "re-press of {t:?} must rest"
            );
        }
    }

    #[test]
    fn re_press_of_select_stays_select_never_toggles_to_nothing() {
        // Select is the rest tool — there is no tool-less state to toggle into.
        let (mut tool, mut right) = (ToolState::default(), RightSidebarState::default());
        tool.activate(Tool::Select, &mut right);
        assert_eq!(tool.active_tool, Tool::Select);
        tool.activate(Tool::Select, &mut right);
        assert_eq!(tool.active_tool, Tool::Select);
    }

    #[test]
    fn activation_reveals_options_for_every_non_select_tool() {
        // One rule for ALL tools — this replaces Brush's old special-case
        // auto-open (finding #8).
        for t in [
            Tool::Move,
            Tool::Rotate,
            Tool::Scale,
            Tool::Pen,
            Tool::Brush,
        ] {
            let (mut tool, mut right) = (ToolState::default(), RightSidebarState::default());
            tool.activate(t, &mut right);
            assert_eq!(
                right.requested,
                Some(RightSidebarSection::Options),
                "{t:?} must reveal the options section"
            );
        }
    }

    #[test]
    fn resting_on_select_un_reveals_only_the_options_section_it_opened() {
        let (mut tool, mut right) = (ToolState::default(), RightSidebarState::default());
        tool.activate(Tool::Brush, &mut right);
        assert_eq!(right.requested, Some(RightSidebarSection::Options));
        tool.activate(Tool::Brush, &mut right); // toggle off → Select
        assert_eq!(tool.active_tool, Tool::Select);
        assert_eq!(right.requested, None, "its own reveal is undone");
    }

    #[test]
    fn resting_on_select_leaves_a_user_opened_section_alone() {
        // F9: pressing S (or Escape rung 3) used to close whatever the user had
        // open — Maps, Settings, the Inspector. D9 ratified the reveal only, so
        // activation may un-reveal Options and nothing else.
        for section in [
            RightSidebarSection::Maps,
            RightSidebarSection::Settings,
            RightSidebarSection::Inspector,
            RightSidebarSection::TerrainTools,
        ] {
            let (mut tool, mut right) = (ToolState::default(), RightSidebarState::default());
            right.reveal(section);
            tool.activate(Tool::Select, &mut right);
            assert_eq!(tool.active_tool, Tool::Select);
            assert_eq!(
                right.requested,
                Some(section),
                "{section:?} was opened by the user and must survive"
            );
        }
    }

    #[test]
    fn a_tool_toggle_off_leaves_a_section_the_user_opened_mid_tool() {
        // The realistic sequence: activate Brush (reveals Options), user opens
        // Maps, user re-presses B to rest on Select. Maps stays open.
        let (mut tool, mut right) = (ToolState::default(), RightSidebarState::default());
        tool.activate(Tool::Brush, &mut right);
        right.reveal(RightSidebarSection::Maps);
        tool.activate(Tool::Brush, &mut right); // toggle off → Select
        assert_eq!(tool.active_tool, Tool::Select);
        assert_eq!(right.requested, Some(RightSidebarSection::Maps));
    }

    #[test]
    fn resting_on_select_with_nothing_open_stays_closed() {
        let (mut tool, mut right) = (ToolState::default(), RightSidebarState::default());
        tool.activate(Tool::Select, &mut right);
        assert_eq!(right.requested, None);
    }

    // --- D-13/D-14: camera focus refuses an effectively-hidden node ---

    #[test]
    fn focus_is_refused_for_an_effectively_hidden_node() {
        use crate::visibility::{NodeAncestry, OverrideState, VisibilityState};

        let ancestry = NodeAncestry {
            verse_id: "v1",
            fractal_id: "f1",
            petal_id: "p1",
        };
        let mut visibility = VisibilityState::default();
        assert!(focus_target_visible("n1", ancestry, &visibility));
        visibility
            .node_overrides
            .insert("n1".into(), OverrideState::Hide);
        assert!(!focus_target_visible("n1", ancestry, &visibility));
        assert!(
            focus_target_visible("n2", ancestry, &visibility),
            "only the hidden node refuses focus"
        );
    }

    #[test]
    fn focus_is_refused_under_a_hidden_ancestor_scope() {
        use crate::visibility::{NodeAncestry, VisibilityState};

        let ancestry = NodeAncestry {
            verse_id: "v1",
            fractal_id: "f1",
            petal_id: "p1",
        };
        let mut visibility = VisibilityState::default();
        visibility.hidden_fractals.insert("f1".into());
        assert!(!focus_target_visible("n1", ancestry, &visibility));
    }

    #[test]
    fn reveal_is_one_shot_a_user_toggle_survives_until_the_next_activation() {
        // Mode-risk mitigation: `requested` is written only at the activation
        // edge, so the rail/topbar toggle wins in between.
        let (mut tool, mut right) = (ToolState::default(), RightSidebarState::default());
        tool.activate(Tool::Pen, &mut right);
        right.toggle(RightSidebarSection::Options); // user closes it
        assert_eq!(right.requested, None);
        assert_eq!(
            tool.active_tool,
            Tool::Pen,
            "closing the panel keeps the tool"
        );
        // Only the next activation re-requests it.
        tool.activate(Tool::Move, &mut right);
        assert_eq!(right.requested, Some(RightSidebarSection::Options));
    }

    #[test]
    fn activation_reveal_survives_the_user_parking_on_another_section() {
        // D11 interplay: a cross-link/rail reveal of another surface is replaced
        // (never stacked) by the next tool activation — one section at a time.
        let (mut tool, mut right) = (ToolState::default(), RightSidebarState::default());
        right.reveal(RightSidebarSection::TerrainTools);
        tool.activate(Tool::Brush, &mut right);
        assert_eq!(right.requested, Some(RightSidebarSection::Options));
    }
}

// ---------------------------------------------------------------------------
// Plugin + system ordering
// ---------------------------------------------------------------------------

#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub enum UiSet {
    /// Drains UI actions, processes portal/URL side effects.
    ProcessActions,
    /// NodeManager selection, gimbal, transform broadcast.
    Selection,
    /// Systems that read finalized state (portal sync, camera focus, cursor).
    PostSelection,
}

pub struct GardenerConsolePlugin;

impl Plugin for GardenerConsolePlugin {
    fn build(&self, app: &mut App) {
        // Domain managers — each owns its state and systems.
        app.add_plugins(crate::navigation_manager::NavigationManagerPlugin);
        app.add_plugins(crate::verse_manager::VerseManagerPlugin);
        app.add_plugins(crate::node_manager::NodeManagerPlugin);
        // UI-only resources (form buffers, dialog flags, etc.)
        app.init_resource::<MeshInstanceBudget>();
        // Mesh-instance watchdog runs in Last so its gate lands before the
        // next frame's spawners (see AGENTS.md §mesh-budget).
        app.add_systems(Last, mesh_instance_watchdog);
        app.init_resource::<SidebarState>();
        app.init_resource::<ToolState>();
        app.init_resource::<InspectorFormState>();
        app.init_resource::<LocalUserRole>();
        app.init_resource::<DashboardState>();
        app.init_resource::<CameraFocusTarget>();
        app.init_resource::<ViewportCursorWorld>();
        app.init_resource::<ViewportRect>();
        app.init_resource::<UiManager>();
        app.init_resource::<PetalMapState>();
        // M2/F6: per-verse timeseries fabric settings surface (mirrors the
        // verse row's ts_* columns; see timeseries_settings.rs).
        app.init_resource::<crate::timeseries_settings::TimeseriesSettingsState>();
        // Application settings (D-78) + terrain-proposal editor state (FR-5).
        app.init_resource::<crate::settings::AppSettings>();
        app.init_resource::<crate::terrain_proposal_state::ProposalEditState>();
        // Wave-1 (spatial-builder-program-20260725): interaction states homed in
        // the action leaf files. StampInteractionState is the third selection
        // authority (N-3); the T2 integration pass threads it read-only into
        // `materialize_path_assets` (override application) and mutably into the
        // db-results / lifecycle consumers. See actions/AGENTS.md §stamped-assets.
        app.init_resource::<crate::actions::asset::StampInteractionState>();
        app.init_resource::<crate::actions::terrain_proposal::SculptToolState>();
        // T3: earthwork region↔node bookkeeping + volume changed-value gate.
        app.init_resource::<crate::actions::terrain_proposal::EarthworkNodeMap>();
        // ui_shell_architecture_20260724 (FR-4/5/6): area-manager state resources.
        app.init_resource::<crate::ui_shell::topbar::TopbarState>();
        app.init_resource::<crate::ui_shell::left_sidebar::LeftSidebarState>();
        app.init_resource::<crate::ui_shell::right_sidebar::RightSidebarState>();
        // Phase 3 (FR-7/Q-5): panel panic guard + transient-layer state.
        app.init_resource::<crate::ui_shell::modal::ModalManagerState>();
        // hierarchy_visibility_groups_20260808 Phases 0-1: session-only
        // per-node/petal/fractal/verse visibility + Group placeholder state,
        // and the spawn-then-hide apply system (RATIFICATION #7). See
        // `visibility/mod.rs`.
        app.init_resource::<crate::visibility::VisibilityState>();
        app.add_systems(Update, crate::visibility::sync_node_visibility);
        // Mirror AppSettings.mesh_budget_ceiling → MeshInstanceBudget.ceiling live.
        app.add_systems(Update, crate::settings::sync_app_settings_to_mesh_budget);
        // Guarantee the renderer scale resource exists so fe-ui can drive it
        // (idempotent with CameraControllerPlugin's own init).
        app.init_resource::<fe_renderer::camera::CameraScaleSettings>();
        app.init_resource::<PendingHexonOps>();
        app.init_resource::<crate::asset_ops::PendingAssetOps>();
        app.init_resource::<crate::asset_ops::AssetDownloadStatus>();
        app.init_resource::<crate::gis::GisPanelState>();
        app.init_resource::<crate::gpx_ops::PendingGpxOps>();
        app.init_resource::<crate::gpx_ops::GpxImportStatus>();
        app.init_resource::<crate::gis::PathEditorState>();
        app.init_resource::<crate::panels::tool_panel::ToolPanelState>();
        app.init_resource::<crate::path_ops::PendingPathOps>();
        app.init_resource::<crate::path_ops::PathEditStatus>();
        app.init_resource::<fe_sync::TilesetEventBuffer>();
        // Register BrowserCommand so MessageWriter<BrowserCommand> is usable.
        // fe-webview's WebViewPlugin also registers this; calling add_message
        // twice is idempotent.
        app.add_message::<fe_webview::ipc::BrowserCommand>();
        // T3 shared seam: fe-terrain's TerrainPlugin also registers this;
        // add_message twice is idempotent (same pattern as BrowserCommand).
        app.add_message::<fe_renderer::terrain_overlay::EarthworkVolumeReport>();
        // T3: persist bake-reported cut/fill volumes onto region nodes
        // (changed-value gated inside the system).
        app.add_systems(
            Update,
            crate::actions::terrain_proposal::persist_earthwork_volumes,
        );
        app.add_systems(EguiPrimaryContextPass, gardener_ui_system);
        // FR-3 (data_icons_20260713): floating point labels over the viewport.
        // After `gardener_ui_system` so it reads the same-frame `ViewportRect`
        // that system writes (no one-frame lag in the panel-edge gating).
        app.add_systems(
            EguiPrimaryContextPass,
            crate::viewport_labels::draw_viewport_point_labels.after(gardener_ui_system),
        );
        app.configure_sets(
            Update,
            (
                UiSet::ProcessActions,
                UiSet::Selection,
                UiSet::PostSelection,
            )
                .chain(),
        );

        app.add_systems(
            Update,
            crate::actions::process_ui_actions.in_set(UiSet::ProcessActions),
        );
        app.add_systems(
            Update,
            resolve_local_role_on_nav_change.in_set(UiSet::ProcessActions),
        );
        // Surface asset-download outcomes (written by the main binary's bridge) as toasts.
        app.add_systems(Update, crate::asset_ops::surface_asset_download_status);
        // Surface GPX-import outcomes (written by the main binary's bridge) as toasts.
        app.add_systems(Update, crate::gpx_ops::surface_gpx_import_status);
        // Surface path-editor outcomes (written by the main binary's bridge) as toasts.
        app.add_systems(Update, crate::path_ops::surface_path_edit_status);
        app.add_systems(
            Update,
            crate::terrain_map::load_petal_terrain_on_nav_change
                .before(crate::actions::process_ui_actions)
                .in_set(UiSet::ProcessActions),
        );
        app.add_systems(
            Update,
            crate::terrain_map::drain_tileset_events.in_set(UiSet::ProcessActions),
        );
        // Mirror the active petal's world scale into the renderer's camera settings.
        app.add_systems(
            Update,
            crate::terrain_map::sync_camera_scale_from_petal_map.in_set(UiSet::PostSelection),
        );

        app.add_systems(
            Update,
            (
                apply_camera_focus,
                strip_gltf_embedded_cameras,
                update_viewport_cursor_world,
                // Real-unit inspector display buffers (inspector_units_width_20260716 FR-2).
                crate::panels::inspector::sync_inspector_units,
                // T3 sculpt brush ring — after the cursor system so the ring
                // reads the same-frame world position.
                crate::sculpt_cursor::draw_sculpt_brush_ring.after(update_viewport_cursor_world),
            )
                .in_set(UiSet::PostSelection),
        );
    }
}

/// Phase F: bundle of P2P-related params to avoid exceeding Bevy's 16-param limit.
/// Also carries NodeManager so the toolbar deselect button can route through it.
#[derive(bevy::ecs::system::SystemParam)]
struct P2pDialogParams<'w> {
    sync_status: Option<Res<'w, fe_sync::SyncStatus>>,
    node_mgr: ResMut<'w, crate::node_manager::NodeManager>,
    ui_mgr: ResMut<'w, UiManager>,
    portal_rect: ResMut<'w, fe_webview::plugin::PortalPanelRect>,
    // TODO: add node_identity once fe-identity is a dependency of fe-ui
    // node_identity: Res<'w, fe_identity::NodeIdentity>,
}

/// Small bundle for miscellaneous read/write-once resources that don't fit an
/// existing group — keeps `gardener_ui_system`'s own param list under Bevy's
/// 16-param `SystemParam` tuple limit as new cross-cutting UI surfaces land.
#[derive(bevy::ecs::system::SystemParam)]
struct MiscUiParams<'w> {
    asset_status: Res<'w, crate::asset_ops::AssetDownloadStatus>,
    gis_panel: ResMut<'w, crate::gis::GisPanelState>,
    gpx_status: Res<'w, crate::gpx_ops::GpxImportStatus>,
    path_state: ResMut<'w, crate::gis::PathEditorState>,
    path_status: Res<'w, crate::path_ops::PathEditStatus>,
    tool_panel: ResMut<'w, crate::panels::tool_panel::ToolPanelState>,
    // FR-5/D-78: terrain proposal editor state + app settings (w4b resources).
    proposal_state: ResMut<'w, crate::terrain_proposal_state::ProposalEditState>,
    app_settings: ResMut<'w, crate::settings::AppSettings>,
    // M2/F6: per-verse timeseries fabric settings surface state.
    ts_settings: ResMut<'w, crate::timeseries_settings::TimeseriesSettingsState>,
    // Wave-1 scaffold: sculpt-tool state threaded to the TerrainTools section
    // (T3 fold). Mirrors `proposal_state`; distinct schedule from
    // `process_ui_actions`'s `ResMut`, so no resource-access conflict.
    sculpt_state: ResMut<'w, crate::actions::terrain_proposal::SculptToolState>,
    // T4: stamp-selection authority, read-only for the context menu's selected
    // marker + live promotion gates (mutation stays in `process_ui_actions`).
    stamp_state: Res<'w, crate::actions::asset::StampInteractionState>,
    // hierarchy_visibility_groups_20260808 Phase 1: sidebar eye toggles +
    // status-bar "N hidden" chip read/write this. Bundled here (rather than a
    // new top-level `gardener_ui_system` param) — that system is already at
    // Bevy's 16-`SystemParam` ceiling.
    vis_state: ResMut<'w, crate::visibility::VisibilityState>,
}

/// ui_shell_architecture_20260724 (FR-4/5/6): the area-manager state resources,
/// bundled to keep `gardener_ui_system` under Bevy's 16-`SystemParam` tuple
/// ceiling. All three are `ResMut` so downstream slices can fill in mutation
/// without re-touching this system's signature.
#[derive(bevy::ecs::system::SystemParam)]
struct UiShellParams<'w> {
    topbar: ResMut<'w, crate::ui_shell::topbar::TopbarState>,
    left_sidebar: ResMut<'w, crate::ui_shell::left_sidebar::LeftSidebarState>,
    right_sidebar: ResMut<'w, crate::ui_shell::right_sidebar::RightSidebarState>,
    // Phase 3 (FR-7/Q-5): panel panic guard + transient-layer state, read by
    // `gardener_console` and `status_bar::status_bar` (persistent error
    // segment). See `ui_shell/modal.rs`.
    modal: ResMut<'w, crate::ui_shell::modal::ModalManagerState>,
}

fn gardener_ui_system(
    mut ctx: EguiContexts,
    mut sidebar: ResMut<SidebarState>,
    mut tool: ResMut<ToolState>,
    mut inspector: ResMut<InspectorFormState>,
    mut nav: ResMut<crate::navigation_manager::NavigationManager>,
    dashboard: Res<DashboardState>,
    mut verse_mgr: ResMut<crate::verse_manager::VerseManager>,
    db_sender: Res<fe_runtime::app::DbCommandSender>,
    mut camera_focus: ResMut<CameraFocusTarget>,
    cursor_world: Res<ViewportCursorWorld>,
    mut p2p: P2pDialogParams,
    mut viewport_rect: ResMut<ViewportRect>,
    local_role: Res<LocalUserRole>,
    mut petal_map: ResMut<PetalMapState>,
    mut misc: MiscUiParams,
    mut ui_shell: UiShellParams,
) {
    let Ok(ectx) = ctx.ctx_mut() else { return };

    let rect = panels::gardener_console(
        ectx,
        &mut sidebar,
        &mut tool,
        &mut inspector,
        &mut nav,
        &dashboard,
        &mut verse_mgr,
        &db_sender.0,
        &mut camera_focus,
        &cursor_world,
        p2p.sync_status.as_deref(),
        &mut p2p.node_mgr,
        &mut p2p.ui_mgr,
        &local_role,
        &mut petal_map,
        &misc.asset_status,
        &mut misc.gis_panel,
        &misc.gpx_status,
        &mut misc.path_state,
        &misc.path_status,
        &mut misc.tool_panel,
        &mut misc.proposal_state,
        &mut misc.app_settings,
        &mut misc.ts_settings,
        &mut misc.sculpt_state,
        &misc.stamp_state,
        &mut ui_shell.topbar,
        &mut ui_shell.left_sidebar,
        &mut ui_shell.right_sidebar,
        &mut ui_shell.modal,
        &mut misc.vis_state,
    );
    viewport_rect.0 = rect;

    // Tell the webview plugin where the right panel is so the popup tracks it.
    // Inset for the portal toolbar header and status bar; see
    // `crate::portal::compute_portal_rect` for the exact math + its tests.
    let screen = ectx.viewport_rect();
    let insets = crate::portal::compute_portal_rect(screen, rect);
    p2p.portal_rect.x = insets.x;
    p2p.portal_rect.y = insets.y;
    p2p.portal_rect.width = insets.width;
    p2p.portal_rect.height = insets.height;

    let role_label = match &local_role.role {
        Some(role) => role.to_string(),
        None => "viewer".to_string(),
    };
    role_chip::role_chip_hud(ectx, &role_label);
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

/// Whether a pending camera-focus request may fly: an EFFECTIVELY hidden node
/// (own override, hidden ancestor scope, group, or a solo lens it is not in)
/// refuses focus — ratified D-13/D-14, `hierarchy_visibility_groups_20260808`.
/// Pure wrapper so the camera's own rule is named and testable.
fn focus_target_visible(
    node_id: &str,
    ancestry: crate::visibility::NodeAncestry,
    visibility: &crate::visibility::VisibilityState,
) -> bool {
    crate::visibility::effective_visibility(node_id, ancestry, visibility)
}

/// Resolves a pending focus request to a world position, preferring the live
/// spawned entity's transform over the cached fallback (camera_focus_clip_20260716
/// FR-2 — see `SpawnedNodeMarker` resolution idiom in `node_manager/sidebar_sync.rs`).
///
/// NOTE: the visibility refusal below is the minimal D-13/D-14 slice alongside
/// `node_manager::viewport_pick`. The remaining surfaces — `context_pick`
/// (right-click), `pointer`, `sidebar_sync`, and the path segment/vertex/handle
/// filters — stay Phase-2 work of `hierarchy_visibility_groups_20260808`.
fn apply_camera_focus(
    mut focus_target: ResMut<CameraFocusTarget>,
    mut query: Query<&mut fe_renderer::camera::OrbitCameraController>,
    spawned: Query<(&SpawnedNodeMarker, &GlobalTransform)>,
    nav: Res<crate::navigation_manager::NavigationManager>,
    visibility: Res<crate::visibility::VisibilityState>,
) {
    if let Some((node_id, fallback)) = focus_target.target.take() {
        // Built exactly as `visibility::sync_node_visibility` builds it, so the
        // camera and the renderer agree on what "hidden" means.
        let ancestry = crate::visibility::NodeAncestry {
            verse_id: nav.active_verse_id.as_deref().unwrap_or(""),
            fractal_id: nav.active_fractal_id.as_deref().unwrap_or(""),
            petal_id: nav.active_petal_id.as_deref().unwrap_or(""),
        };
        // Never fly to something the user cannot see: DROP the request (it was
        // already `take`n) rather than parking the camera on empty space.
        if !focus_target_visible(&node_id, ancestry, &visibility) {
            return;
        }
        if let Ok(mut controller) = query.single_mut() {
            let pos = spawned
                .iter()
                .find(|(marker, _)| marker.node_id == node_id)
                .map(|(_, transform)| transform.translation())
                .unwrap_or_else(|| Vec3::from(fallback));
            // ux_interaction_hardening FR-5: write easing targets so the camera
            // flies to the node instead of teleporting.
            controller.target_focus = Some(pos);
            controller.target_distance =
                5.0_f32.clamp(controller.min_distance, controller.max_distance);
        }
    }
}

/// Projects the cursor position onto the Y=0 world plane each frame.
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
    if ectx.is_using_pointer() {
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

/// Sends ResolveLocalRole when the navigation scope changes.
fn resolve_local_role_on_nav_change(
    nav: Res<crate::navigation_manager::NavigationManager>,
    db_sender: Res<fe_runtime::app::DbCommandSender>,
    mut last_scope: Local<Option<String>>,
) {
    let current_scope = nav.active_verse_id.as_ref().map(|vid| {
        fe_database::build_scope(
            vid,
            nav.active_fractal_id.as_deref(),
            nav.active_petal_id.as_deref(),
        )
    });

    if *last_scope == current_scope {
        return;
    }
    *last_scope = current_scope.clone();

    if let Some(scope) = current_scope {
        db_sender
            .0
            .send(fe_runtime::messages::DbCommand::ResolveLocalRole { scope })
            .ok();
    }
}
