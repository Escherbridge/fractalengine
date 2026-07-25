//! Top-level UI shell: `gardener_console` orders the area managers
//! (`ui_shell::{topbar, left_sidebar, right_sidebar}`) around the status bar,
//! viewport, floating dialogs, and the toast overlay. The manager render bodies
//! live in `ui_shell` (FR-4/5/6); this module keeps the panel widgets they call
//! plus the still-floating windows. See `fe-ui/src/panels/AGENTS.md`.

pub(crate) mod gis_panel;
pub(crate) mod inspector;
pub(crate) mod proposal_report_panel;
pub(crate) mod query_tab;
pub(crate) mod sidebar;
pub(crate) mod status_bar;
pub(crate) mod terrain_tools_panel;
/// tool_inspector_ux_20260719: active tool as a legible UI MODE (left per-tool
/// inspector panel). See `fe-ui/src/panels/AGENTS.md` §tool-inspector.
pub(crate) mod tool_inspector;
pub(crate) mod tool_panel;
pub(crate) mod toolbar;

mod annotation_card;
mod asset_card;
mod egress_card;
mod gpx_import_card;
mod layer_manager_card;
// `pub(crate)` so `crate::viewport_labels` can reuse `type_glyph` (FR-3,
// data_icons_20260713); everything else in it is still crate-internal.
pub(crate) mod path_editor_card;
// `pub(crate)` so `ui_shell::right_sidebar` can call `right_portal_toolbar`
// when the portal owns the right region (FR-6).
pub(crate) mod portal_toolbar;
/// Shared reusable panel widgets (copy boxes, elision). See AGENTS.md §widgets.
mod widgets;

use bevy_egui::egui;

use crate::actions::UiManager;
use crate::asset_ops::AssetDownloadStatus;
use crate::navigation_manager::NavigationManager;
use crate::plugin::{
    CameraFocusTarget, InspectorFormState, LocalUserRole, SidebarState, ToolState,
    ViewportCursorWorld,
};
use crate::verse_manager::VerseManager;
use fe_runtime::messages::DbCommand;

// ---------------------------------------------------------------------------
// Top-level layout entry point
// ---------------------------------------------------------------------------

/// Renders the full UI shell in order: topbar -> status bar -> left sidebar ->
/// tool inspector -> right sidebar -> central viewport -> floating windows ->
/// toast. Returns the screen-space rect of the 3-D viewport (CentralPanel) so
/// the caller can store it for viewport-click gating in the gimbal system.
// NOTE: wide param list accepted (plain egui fn, not a Bevy system); group new
// params into the caller's SystemParam bundles before adding more here.
pub fn gardener_console(
    ctx: &egui::Context,
    sidebar: &mut SidebarState,
    tool: &mut ToolState,
    inspector: &mut InspectorFormState,
    nav: &mut NavigationManager,
    dashboard: &crate::atlas::DashboardState,
    hierarchy: &mut VerseManager,
    db_tx: &crossbeam::channel::Sender<DbCommand>,
    camera_focus: &mut CameraFocusTarget,
    cursor_world: &ViewportCursorWorld,
    sync_status: Option<&fe_sync::SyncStatus>,
    node_mgr: &mut crate::node_manager::NodeManager,
    ui_mgr: &mut UiManager,
    local_role: &LocalUserRole,
    petal_map: &mut crate::terrain_map::PetalMapState,
    asset_status: &AssetDownloadStatus,
    gis_panel: &mut crate::gis::GisPanelState,
    gpx_status: &crate::gpx_ops::GpxImportStatus,
    path_state: &mut crate::gis::PathEditorState,
    path_status: &crate::path_ops::PathEditStatus,
    tool_panel: &mut crate::panels::tool_panel::ToolPanelState,
    // FR-5/FR-6/D-78 (terrain_editor_overhaul_20260718 + p2p_asset_streaming_20260718):
    // new cross-worker resources (owned by w4b — `terrain_proposal_state.rs`/
    // `settings.rs`). NOTE FOR w4b: this is a genuine `gardener_console`
    // signature change (unlike `tool_panel`'s picker, which reused existing
    // params) — `plugin.rs::gardener_ui_system`'s call site needs these two
    // args threaded in (via `init_resource` + a `ResMut` bundle, same idiom
    // as `MiscUiParams`). See `panels/AGENTS.md` §terrain-tools.
    proposal_state: &mut crate::terrain_proposal_state::ProposalEditState,
    app_settings: &mut crate::settings::AppSettings,
    // ui_shell_architecture_20260724 Phase 2 (FR-4/5/6): area-manager state,
    // supplied by `plugin.rs::gardener_ui_system` via the `UiShellParams` bundle.
    topbar_state: &mut crate::ui_shell::topbar::TopbarState,
    left_state: &mut crate::ui_shell::left_sidebar::LeftSidebarState,
    right_state: &mut crate::ui_shell::right_sidebar::RightSidebarState,
) -> egui::Rect {
    // Topbar (FR-4): tool switcher, deselect, Data/Tools/Settings/Maps.
    crate::ui_shell::topbar::render_topbar(
        ctx,
        topbar_state,
        tool,
        node_mgr,
        ui_mgr,
        gis_panel,
        tool_panel,
        right_state,
    );
    status_bar::status_bar(ctx, dashboard, sync_status, nav, ui_mgr);

    // Left sidebar (FR-5): the manager owns the auto-collapse policy. `right_open`
    // reproduces today's formula exactly; the manager's default policy applies it
    // as `sidebar.open = !right_open` (replacing the old post-render stomp).
    let right_open = ui_mgr.portal_is_open() || node_mgr.selected_entity().is_some();
    crate::ui_shell::left_sidebar::render_left_sidebar(
        ctx,
        left_state,
        sidebar,
        right_open,
        nav,
        dashboard,
        hierarchy,
        camera_focus,
        db_tx,
        node_mgr,
        ui_mgr,
    );

    // tool_inspector_ux_20260719: the active tool as a legible MODE (inner left
    // panel). KEPT this phase — a Phase-5 sibling folds it into the Tool section.
    // Hidden while the portal webview is open so it doesn't clutter the portal.
    if !ui_mgr.portal_is_open() {
        tool_inspector::tool_inspector_panel(ctx, tool, node_mgr, path_state);
    }

    // Right sidebar (FR-6): portal toolbar when the portal owns the region, else
    // the active section (Inspector today; placeholders for the rest).
    crate::ui_shell::right_sidebar::render_right_sidebar(
        ctx,
        right_state,
        inspector,
        node_mgr,
        hierarchy,
        ui_mgr,
        local_role,
        db_tx,
        nav,
        asset_status,
    );

    let viewport_response =
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(ctx, |ui| {
                crate::viewport::viewport_overlay(
                    ui,
                    nav,
                    node_mgr,
                    hierarchy,
                    db_tx,
                    dashboard,
                    cursor_world,
                    ui_mgr,
                    local_role,
                );
            });

    // Floating dialogs / menus (rendered after panels so they layer on top)
    crate::dialogs::render_context_menu(ctx, ui_mgr);
    crate::dialogs::render_create_dialog(ctx, ui_mgr, hierarchy, nav, db_tx);
    crate::dialogs::render_gltf_import_dialog(ctx, ui_mgr, nav, db_tx);
    crate::dialogs::render_join_dialog(ctx, ui_mgr, db_tx);
    crate::dialogs::render_peer_debug_panel(ctx, ui_mgr, sync_status);
    crate::dialogs::render_node_options_dialog(ctx, ui_mgr, hierarchy, db_tx);
    crate::dialogs::render_entity_settings_dialog(ctx, ui_mgr, db_tx);
    crate::dialogs::render_hexon_manager(ctx, ui_mgr, petal_map, nav.active_petal_id.as_deref());
    crate::dialogs::render_petal_manifest(ctx, ui_mgr);
    // D-78: application settings window (see `dialogs/settings.rs`).
    crate::dialogs::settings_window(ctx, ui_mgr, app_settings);

    // GIS query & layer-manager panel (independent floating window, not part
    // of the mutual-exclusion `ActiveDialog` set — see panels/AGENTS.md §gis).
    gis_panel::render_gis_panel(
        ctx,
        gis_panel,
        node_mgr,
        hierarchy,
        nav,
        petal_map,
        ui_mgr,
        camera_focus,
        gpx_status,
        path_state,
        path_status,
        cursor_world,
    );

    // Tools panel (independent floating window, hosts hexon-path-asset
    // stamping controls; the "Stamp along path" button emits
    // `UiAction::PathAssetApply` for the Paths-tab track being edited — see
    // panels/AGENTS.md §tool-panel).
    tool_panel::render_tool_panel(ctx, tool_panel, ui_mgr, path_state, hierarchy);

    // FR-5/FR-6 (terrain_editor_overhaul_20260718): terrain proposal palette
    // (own floating window, toggled from the Tools panel's "Terrain Tools"
    // pointer section) and its report panel for the selected proposal.
    terrain_tools_panel::terrain_tools_panel(ctx, tool_panel, ui_mgr, proposal_state);
    proposal_report_panel::proposal_report_panel(ctx, proposal_state, petal_map.world_scale);

    // Toast overlay (bottom-left, semi-transparent)
    render_toast(ctx, ui_mgr);

    viewport_response.response.rect
}

fn render_toast(ctx: &egui::Context, ui_mgr: &UiManager) {
    let now = ctx.input(|i| i.time);
    let Some((msg, alpha)) = ui_mgr.active_toast(now) else {
        return;
    };

    let bg = egui::Color32::from_rgba_unmultiplied(30, 30, 40, (alpha * 220.0) as u8);
    let text_color = egui::Color32::from_rgba_unmultiplied(220, 220, 220, (alpha * 255.0) as u8);

    egui::Area::new(egui::Id::new("toast_overlay"))
        .anchor(egui::Align2::LEFT_BOTTOM, egui::vec2(16.0, -40.0))
        .interactable(false)
        .show(ctx, |ui| {
            egui::Frame::NONE
                .fill(bg)
                .inner_margin(egui::Margin::symmetric(12, 6))
                .corner_radius(6.0)
                .show(ui, |ui| {
                    ui.label(egui::RichText::new(msg).color(text_color).small());
                });
        });

    // Request repaint while toast is visible so the fade animates
    ctx.request_repaint();
}
