//! Top-level UI shell: toolbar, status bar, sidebar, inspector/portal panel,
//! viewport, floating dialogs, and the toast overlay. See
//! `fe-ui/src/panels/AGENTS.md` for the per-panel file map.

pub(crate) mod gis_panel;
pub(crate) mod inspector;
pub(crate) mod query_tab;
pub(crate) mod sidebar;
pub(crate) mod status_bar;
pub(crate) mod toolbar;

mod annotation_card;
mod asset_card;
mod layer_manager_card;
mod portal_toolbar;

use bevy_egui::egui;

use crate::actions::UiManager;
use crate::asset_ops::AssetDownloadStatus;
use crate::navigation_manager::NavigationManager;
use crate::plugin::{CameraFocusTarget, InspectorFormState, LocalUserRole, SidebarState, ToolState, ViewportCursorWorld};
use crate::verse_manager::VerseManager;
use fe_runtime::messages::DbCommand;

// ---------------------------------------------------------------------------
// Top-level layout entry point
// ---------------------------------------------------------------------------

/// Renders the full UI shell: toolbar -> status bar -> sidebar -> inspector -> viewport.
/// Returns the screen-space rect of the 3-D viewport (CentralPanel) so the
/// caller can store it for viewport-click gating in the gimbal system.
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
) -> egui::Rect {
    toolbar::top_toolbar(ctx, sidebar, tool, node_mgr, ui_mgr, gis_panel);
    status_bar::status_bar(ctx, dashboard, sync_status, nav, ui_mgr);
    sidebar::left_sidebar(
        ctx,
        sidebar,
        nav,
        dashboard,
        hierarchy,
        camera_focus,
        db_tx,
        node_mgr,
        ui_mgr,
    );

    // Auto-collapse left sidebar when right panel is open, restore when closed.
    let right_panel_open = ui_mgr.portal_is_open() || node_mgr.selected_entity().is_some();
    sidebar.open = !right_panel_open;

    // When the portal webview is open, show the portal toolbar instead of inspector.
    if ui_mgr.portal_is_open() {
        portal_toolbar::right_portal_toolbar(ctx, ui_mgr);
    } else {
        inspector::right_inspector(ctx, inspector, node_mgr, hierarchy, ui_mgr, local_role, db_tx, nav, asset_status);
    }

    let viewport_response = egui::CentralPanel::default()
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

    // GIS query & layer-manager panel (independent floating window, not part
    // of the mutual-exclusion `ActiveDialog` set — see panels/AGENTS.md §gis).
    gis_panel::render_gis_panel(ctx, gis_panel, node_mgr, hierarchy, nav, petal_map, ui_mgr, camera_focus);

    // Toast overlay (bottom-left, semi-transparent)
    render_toast(ctx, ui_mgr);

    viewport_response.response.rect
}

fn render_toast(ctx: &egui::Context, ui_mgr: &UiManager) {
    let now = ctx.input(|i| i.time);
    let Some((msg, alpha)) = ui_mgr.active_toast(now) else { return };

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
