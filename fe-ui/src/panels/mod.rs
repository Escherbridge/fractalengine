//! Top-level UI shell: `gardener_console` orders the area managers
//! (`ui_shell::{topbar, left_sidebar, right_sidebar}`) around the status bar,
//! viewport, floating dialogs, and the toast overlay. The manager render bodies
//! live in `ui_shell` (FR-4/5/6); this module keeps the panel widgets they call
//! plus the still-floating `gis_panel` window and the `ActiveDialog` set.
//! Phase 4 (FR-9) retired the last three floating windows (Tools/Terrain
//! Tools/Proposal Report) — their bodies now live in
//! `ui_shell::right_sidebar`'s PathTools/TerrainTools/ProposalReport
//! sections. See `fe-ui/src/panels/AGENTS.md`.

pub(crate) mod gis_panel;
pub(crate) mod inspector;
pub(crate) mod proposal_report_panel;
pub(crate) mod query_tab;
pub(crate) mod sidebar;
pub(crate) mod status_bar;
pub(crate) mod terrain_tools_panel;
/// Pure per-tool helpers (descriptor/selection/anchor), consumed by the
/// toolbar tooltip and the right-sidebar Tool section. No left panel anymore
/// (ui_shell_architecture_20260724 Phase 5) — see `panels/AGENTS.md` §tool-inspector.
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
use crate::dialogs::ActiveDialog;
use crate::navigation_manager::NavigationManager;
use crate::plugin::{
    CameraFocusTarget, InspectorFormState, LocalUserRole, SidebarState, ToolState,
    ViewportCursorWorld,
};
use crate::ui_shell::modal::{
    guarded, is_dialog_family, transient_order, TransientLayer, TransientVisibility,
};
use crate::verse_manager::VerseManager;
use fe_runtime::messages::DbCommand;

// ---------------------------------------------------------------------------
// Top-level layout entry point
// ---------------------------------------------------------------------------

/// Renders the full UI shell in order: topbar -> status bar -> left sidebar ->
/// right sidebar -> central viewport -> dialogs/context menu -> gis panel ->
/// toast. Returns the screen-space rect of the 3-D viewport (CentralPanel) so
/// the caller can store it for viewport-click gating in the gimbal system.
///
/// Phase 3 (FR-7/Q-5): every render step but `status_bar` runs through
/// [`crate::ui_shell::modal::guarded`] — a panicking panel is quarantined for
/// the session instead of taking down the pass; `status_bar` stays unguarded
/// since it hosts the resulting error segment. See `ui_shell/modal.rs`.
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
    // Wave-1 scaffold: sculpt-tool state, threaded through to the TerrainTools
    // section so T3's folded sculpt UI reads/writes it (mirrors `proposal_state`).
    sculpt_state: &mut crate::actions::terrain_proposal::SculptToolState,
    // ui_shell_architecture_20260724 Phase 2 (FR-4/5/6): area-manager state,
    // supplied by `plugin.rs::gardener_ui_system` via the `UiShellParams` bundle.
    topbar_state: &mut crate::ui_shell::topbar::TopbarState,
    left_state: &mut crate::ui_shell::left_sidebar::LeftSidebarState,
    right_state: &mut crate::ui_shell::right_sidebar::RightSidebarState,
    // Phase 3 (FR-7/Q-5): panel panic guard + transient-layer sequencing state,
    // also supplied via `UiShellParams`. See `ui_shell/modal.rs`.
    modal: &mut crate::ui_shell::modal::ModalManagerState,
) -> egui::Rect {
    // Topbar (FR-4): tool switcher, deselect, Data/Tools/Settings/Maps.
    let _ = guarded(modal, "topbar", || {
        crate::ui_shell::topbar::render_topbar(
            ctx,
            topbar_state,
            tool,
            node_mgr,
            gis_panel,
            left_state,
            right_state,
        )
    });
    // Never guarded: hosts the persistent guard-error segment below (Q-5) —
    // if this itself were quarantined, a disabled panel's error would vanish.
    status_bar::status_bar(ctx, dashboard, sync_status, nav, ui_mgr, modal);

    // Left sidebar (FR-3 shell_ux_sidebar): user-sticky. The manager applies
    // `sidebar.open = user_intent` only — the old per-frame `!right_open` stomp
    // is GONE (D-A11). Selection / right-section open no longer collapse it.
    let _ = guarded(modal, "left_sidebar", || {
        crate::ui_shell::left_sidebar::render_left_sidebar(
            ctx,
            left_state,
            sidebar,
            nav,
            dashboard,
            hierarchy,
            camera_focus,
            db_tx,
            node_mgr,
            ui_mgr,
        )
    });

    // Right sidebar (FR-6): portal toolbar when the portal owns the region, else
    // the active section (Inspector by default; Path/Terrain Tools and the
    // Proposal Report host the former floating windows as of Phase 4/FR-9).
    // One guard covers all five mutually-exclusive sections (incl. Terrain
    // Tools, directive-3's crash surface) — `right_sidebar.rs` is out of this
    // slice's owned-file set, so a panic anywhere in the active section
    // quarantines the whole region rather than just that section; see report.
    let _ = guarded(modal, "right_sidebar", || {
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
            tool_panel,
            path_state,
            proposal_state,
            sculpt_state,
            petal_map,
            app_settings,
            tool,
        )
    });

    let viewport_response =
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(ctx, |ui| {
                // Guarded INSIDE the `.show` closure so a panic doesn't unwind
                // through `CentralPanel::show` — the returned rect stays valid.
                let _ = guarded(modal, "viewport", || {
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
            });

    // Transient overlay layer (FR-7/Q-5): dialogs, context menu, toast, LAST
    // in the pass so they layer on top (behavior-preserving vs. today).
    // `transient_order()` fixes the paint order (Dialog -> ContextMenu ->
    // Toast); `resolve_exclusive()` enforces at-most-one dialog-family
    // overlay while toast stays independent.
    let now = ctx.input(|i| i.time);
    let visibility = TransientVisibility {
        dialog: !matches!(ui_mgr.active_dialog, ActiveDialog::None)
            && !matches!(ui_mgr.active_dialog, ActiveDialog::ContextMenu { .. }),
        context_menu: matches!(ui_mgr.active_dialog, ActiveDialog::ContextMenu { .. }),
        toast: ui_mgr.active_toast(now).is_some(),
    }
    .resolve_exclusive();
    debug_assert!(
        visibility.dialog_family_exclusive(),
        "transient layer must keep at most one dialog-family overlay visible"
    );

    // Dialog-family layers only here (Toast is handled after `gis_panel`
    // below — see that block's comment for why the split preserves today's
    // paint order exactly).
    for layer in transient_order() {
        if layer == TransientLayer::Toast {
            continue;
        }
        // Regression guard: keeps `transient_order()`'s variant set and
        // `is_dialog_family`'s classification in sync across module edits —
        // both live in `modal.rs`, this loop is their one production call site.
        debug_assert!(
            is_dialog_family(layer),
            "transient layer {layer:?} reached the dialog-family loop but isn't classified as one"
        );
        match layer {
            TransientLayer::Dialog => {
                if !visibility.dialog {
                    continue;
                }
                // Each dialog fn independently gates on its own `ActiveDialog`
                // variant (early return otherwise, unchanged from today); the
                // guard just quarantines that one dialog if it panics.
                let _ = guarded(modal, "dialog_create_entity", || {
                    crate::dialogs::render_create_dialog(ctx, ui_mgr, hierarchy, nav, db_tx)
                });
                let _ = guarded(modal, "dialog_gltf_import", || {
                    crate::dialogs::render_gltf_import_dialog(ctx, ui_mgr, nav, db_tx)
                });
                let _ = guarded(modal, "dialog_join", || {
                    crate::dialogs::render_join_dialog(ctx, ui_mgr, db_tx)
                });
                let _ = guarded(modal, "dialog_peer_debug", || {
                    crate::dialogs::render_peer_debug_panel(ctx, ui_mgr, sync_status)
                });
                let _ = guarded(modal, "dialog_node_options", || {
                    crate::dialogs::render_node_options_dialog(ctx, ui_mgr, hierarchy, db_tx)
                });
                let _ = guarded(modal, "dialog_entity_settings", || {
                    crate::dialogs::render_entity_settings_dialog(ctx, ui_mgr, db_tx)
                });
                // FR-2 (shell_ux_sidebar): the Map Manager is a right-sidebar
                // section now (`ui_shell::right_sidebar::render_maps_section`),
                // not a floating dialog — removed from the transient family here.
                let _ = guarded(modal, "dialog_petal_manifest", || {
                    crate::dialogs::render_petal_manifest(ctx, ui_mgr)
                });
                // FR-1 (shell_ux_sidebar): application Settings is a right-sidebar
                // section now (`render_settings_section`), not a floating window —
                // the old `ActiveDialog::Settings` + `settings_window` are removed.
            }
            TransientLayer::ContextMenu => {
                if !visibility.context_menu {
                    continue;
                }
                let _ = guarded(modal, "context_menu", || {
                    crate::dialogs::render_context_menu(ctx, ui_mgr)
                });
            }
            TransientLayer::Toast => unreachable!("skipped by the `continue` above"),
        }
    }

    // GIS query & layer-manager panel (independent floating window, not part
    // of the mutual-exclusion `ActiveDialog` set — see panels/AGENTS.md §gis).
    // Sits between the dialog family and the toast overlay, same as today —
    // NOT part of `transient_order()` (only 3 variants: Dialog/ContextMenu/
    // Toast), so it renders outside that loop, but its position preserves
    // today's layering: toast always paints last/topmost.
    let _ = guarded(modal, "gis_panel", || {
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
        )
    });

    // Toast overlay (bottom-left, semi-transparent) — last in
    // `transient_order()`, painted topmost.
    if visibility.toast {
        let _ = guarded(modal, "toast", || render_toast(ctx, ui_mgr));
    }

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
