//! Top-level UI shell: `gardener_console` orders the area managers
//! (`ui_shell::{topbar, left_sidebar, right_sidebar}`) around the status bar,
//! viewport, floating dialogs, and the toast overlay. The manager render bodies
//! live in `ui_shell` (FR-4/5/6); this module keeps the panel widgets they call
//! plus the still-floating `gis_panel` window and the `ActiveDialog` set.
//! Phase 4 (FR-9) retired the last three floating windows (Tools/Terrain
//! Tools/Proposal Report) — their bodies now live in
//! `ui_shell::right_sidebar`'s sections; ui_semantics_unification_20260808
//! then folded the PathTools section into the `Options` section's Pen arm
//! (`tool_options`). See `fe-ui/src/panels/AGENTS.md`.

pub(crate) mod gis_panel;
pub(crate) mod inspector;
mod property_grid;
pub(crate) mod property_value;
pub(crate) mod proposal_report_panel;
pub(crate) mod query_tab;
pub(crate) mod sidebar;
pub(crate) mod status_bar;
pub(crate) mod terrain_tools_panel;
/// Pure per-tool helpers (descriptor/selection/anchor), consumed by the
/// toolbar tooltip and the right-sidebar Tool section. No left panel anymore
/// (ui_shell_architecture_20260724 Phase 5) — see `panels/AGENTS.md` §tool-inspector.
pub(crate) mod tool_inspector;
/// ui_semantics_unification_20260808 Phase 3: the ONE active-tool options
/// dispatcher behind `RightSidebarSection::Options` — one arm per `Tool`, each
/// reusing an existing helper. See `panels/AGENTS.md` §tool-options.
pub(crate) mod tool_options;
pub(crate) mod transform_options;
pub(crate) mod tool_panel;
pub(crate) mod toolbar;
pub(crate) mod tree_appearance;
pub(crate) mod tree_entry;
/// terrain_proposal_delta_viz_20261025 S6: true-figures "Volume report" block
/// hosted below `proposal_report_panel` in the same renamed section. See
/// `panels/AGENTS.md` §volume-report.
pub(crate) mod volume_report_panel;

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
// W1-GATE compile-fix: `dialogs::{gltf_import, hexon_manager}` reuse these
// helpers cross-module per the Wave-1 contract; widgets stays crate-visible,
// not fully `pub`.
pub(crate) mod widgets;

use bevy_egui::egui;

use crate::accessibility::ResponseExt;
use crate::actions::UiManager;
use crate::asset_ops::AssetDownloadStatus;
use crate::dialogs::ActiveDialog;
use crate::navigation_manager::NavigationManager;
use crate::plugin::{
    CameraFocusTarget, InspectorFormState, LocalUserRole, SidebarState, ToolState,
    ViewportCursorWorld,
};
use crate::ui_shell::modal::{
    TransientLayer, TransientVisibility, guarded, is_dialog_family, transient_order,
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
    // M2/F6: per-verse timeseries fabric settings surface state.
    ts_settings: &mut crate::timeseries_settings::TimeseriesSettingsState,
    // Wave-1 scaffold: sculpt-tool state, threaded through to the TerrainTools
    // section so T3's folded sculpt UI reads/writes it (mirrors `proposal_state`).
    sculpt_state: &mut crate::actions::terrain_proposal::SculptToolState,
    // T4 contextual controls: the stamp-selection authority, read by the
    // right-click context menu (selected marker + live promotion gates).
    stamp_state: &crate::actions::asset::StampInteractionState,
    // ui_shell_architecture_20260724 Phase 2 (FR-4/5/6): area-manager state,
    // supplied by `plugin.rs::gardener_ui_system` via the `UiShellParams` bundle.
    topbar_state: &mut crate::ui_shell::topbar::TopbarState,
    left_state: &mut crate::ui_shell::left_sidebar::LeftSidebarState,
    right_state: &mut crate::ui_shell::right_sidebar::RightSidebarState,
    // Phase 3 (FR-7/Q-5): panel panic guard + transient-layer sequencing state,
    // also supplied via `UiShellParams`. See `ui_shell/modal.rs`.
    modal: &mut crate::ui_shell::modal::ModalManagerState,
    // hierarchy_visibility_groups_20260808 Phase 1: session-only visibility
    // state — sidebar eye toggles read/write it, the status bar reads the
    // effectively-hidden count off it. See `visibility/mod.rs`.
    vis_state: &mut crate::visibility::VisibilityState,
    // hexon_scale FR-9: Measure tool anchors, edited by the Options Measure arm.
    measure_state: &mut crate::node_manager::measure::MeasureState,
    // S4 (p2p_asset_streaming_20260718): active tileset downloads — read by
    // the Maps section, written only by `terrain_map::drain_tileset_events`.
    downloads: &crate::terrain_map::TilesetDownloads,
    // S6: session-only per-petal render-distance override (Settings section).
    render_override: &mut crate::settings::PetalRenderOverride,
    // F6: startup identity-degradation banner (persistent, click-to-dismiss
    // segment in the status bar) — see `panels/AGENTS.md` §identity-banner.
    identity_banner: &mut crate::plugin::IdentityBanner,
    node_identity: Option<&fe_identity::NodeIdentity>,
    known_peers: Option<&fe_runtime::PeerRegistry>,
    // terrain_proposal_delta_viz_20261025 S6: the Volume report's live
    // query + latest true-figures answer. Read-only here — `sync_volume_
    // report_request`/`drain_volume_report_result` (plugin.rs, PostUpdate)
    // are its sole writers.
    volume_report: &crate::panels::volume_report_panel::VolumeReportState,
    settings_session: &mut crate::settings::SettingsSession,
) -> egui::Rect {
    crate::native_accessibility::begin_pass(ctx, crate::native_accessibility::product_guard(
        ctx, ui_mgr, node_mgr, nav, hierarchy, local_role, topbar_state, modal, path_state));
    use crate::native_accessibility::surface::{self, Producer};
    surface::begin(ctx, crate::native_accessibility::product_guard(
        ctx, ui_mgr, node_mgr, nav, hierarchy, local_role, topbar_state, modal, path_state));
    crate::native_accessibility::product_guard(
        ctx, ui_mgr, node_mgr, nav, hierarchy, local_role, topbar_state, modal, path_state);
    crate::native_accessibility::navigation_ui::begin(ctx);
    crate::ui_shell::layout_policy::begin_ui_pass(ctx);
    crate::ui_shell::layout_policy::apply_style(ctx);
    // Repair conditional editor state; see `panels/AGENTS.md` §numeric-boundary.
    path_editor_card::sanitize_path_editor_numeric_state(path_state);
    tool_panel::sync_stamp_editor(tool_panel,stamp_state,path_state,nav,hierarchy,local_role,petal_map.world_scale);

    surface::enter(ctx, Producer::Topbar, !topbar_state.close_pending);
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
            local_role,
        )
    });
    // Never guarded: hosts the persistent guard-error segment below (Q-5) —
    // if this itself were quarantined, a disabled panel's error would vanish.
    surface::enter(ctx, Producer::Status, identity_banner.message.is_none()
        && modal.last_error.is_none() && ui_mgr.active_notice(ctx.input(|input| input.time)).is_none()
        && ui_mgr.error_history().next().is_none());
    status_bar::status_bar(
        ctx,
        dashboard,
        sync_status,
        nav,
        ui_mgr,
        modal,
        hierarchy,
        vis_state,
        identity_banner,
        local_role,
        node_identity,
    );

    surface::enter(ctx, Producer::Tree, left_state.user_intent);
    // Left sidebar (FR-3 shell_ux_sidebar): user-sticky. The manager applies
    // `sidebar.open = user_intent` only — the old per-frame `!right_open` stomp
    // is GONE (D-A11). Selection / right-section open no longer collapse it.
    let _ = guarded(modal, "left_sidebar", || {
        crate::ui_shell::left_sidebar::render_left_sidebar(
            ctx,
            left_state,
            topbar_state.close_pending,
            sidebar,
            nav,
            dashboard,
            hierarchy,
            camera_focus,
            db_tx,
            node_mgr,
            ui_mgr,
            vis_state,
            app_settings,
            local_role,
        )
    });

    crate::native_accessibility::product_guard(
        ctx, ui_mgr, node_mgr, nav, hierarchy, local_role, topbar_state, modal, path_state);
    sidebar::apply_native_petal_click(ctx, hierarchy, nav, node_mgr);
    if inspector.active_tab() == crate::plugin::InspectorTab::ApiAccess { surface::credential(ctx); }
    surface::enter(ctx, Producer::Inspector, node_mgr.selection_count() == 0
        && inspector.active_tab() == crate::plugin::InspectorTab::Properties
        && crate::ui_shell::right_sidebar::active_section(right_state, false, ui_mgr.portal_is_open())
            == Some(crate::ui_shell::right_sidebar::RightSidebarSection::Inspector));
    // Each right section owns its quarantine boundary.
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
        ts_settings,
        tool,
        measure_state,
        vis_state,
        downloads,
        render_override,
        volume_report,
        modal,
        settings_session,
        topbar_state,
    );


    surface::enter(ctx, Producer::Results, !inspector.query_results.open);
    crate::ui_shell::results_dock::render(
        ctx,
        &mut inspector.query_results,
        nav,
        hierarchy,
        node_mgr,
        camera_focus,
        ui_mgr,
    );

    surface::enter(ctx, Producer::Viewport,
        (nav.active_petal_id.is_some() || crate::native_accessibility::navigation_ui::eligible(ctx))
            && !ui_mgr.portal_is_open());
    fe_renderer::scene_pointer::ScenePointerChrome::begin_pass(ctx, nav.intent_revision());
    let viewport_response =
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(ctx, |ui| {
                // Guarded INSIDE the `.show` closure so a panic doesn't unwind
                // through `CentralPanel::show` — the returned rect stays valid.
                let _ = guarded(modal, "viewport", || {
                    let Some(content_rect) = crate::ui_shell::layout_policy::overlay_content_rect(
                        ctx, ui.max_rect()) else { return; };
                    let mut overlay = ui.new_child(egui::UiBuilder::new()
                        .id_salt("viewport-overlay-content").max_rect(content_rect));
                    overlay.set_clip_rect(ui.clip_rect().intersect(content_rect));
                    crate::viewport::viewport_overlay(
                        &mut overlay,
                        nav,
                        node_mgr,
                        hierarchy,
                        db_tx,
                        dashboard,
                        cursor_world,
                        ui_mgr,
                        local_role,
                        !gis_panel.open,
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

    if matches!(ui_mgr.active_dialog, ActiveDialog::JoinDialog { .. } | ActiveDialog::EntitySettings { .. }) {
        surface::credential(ctx);
    }
    surface::enter(ctx, Producer::Transient, matches!(ui_mgr.active_dialog, ActiveDialog::None)
        && !visibility.toast);
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
                    // L-06: a new node spawns at the camera focus target when
                    // one exists (the source `viewport.rs` frames selections with).
                    crate::dialogs::render_create_dialog(
                        ctx,
                        ui_mgr,
                        db_tx,
                        camera_focus.target.as_ref().map(|(_, pos)| *pos),
                    )
                });
                let _ = guarded(modal, "dialog_gltf_import", || {
                    // D-06: the meters readout needs the active petal's scale.
                    crate::dialogs::render_gltf_import_dialog(
                        ctx,
                        ui_mgr,
                        nav,
                        db_tx,
                        petal_map.world_scale,
                    )
                });
                let _ = guarded(modal, "dialog_join", || {
                    ui_mgr.validate_qa_join_input_domain(node_identity.map(|identity|identity.did.as_str()),nav.intent_revision());
                    crate::dialogs::render_join_dialog(ctx, ui_mgr, db_tx)
                });
                let _ = guarded(modal, "dialog_peer_debug", || {
                    crate::dialogs::render_peer_debug_panel(ctx, ui_mgr, sync_status)
                });
                let _ = guarded(modal, "dialog_tree_appearance", || {
                    tree_appearance::render(ctx, ui_mgr, app_settings, hierarchy)
                });
                let _ = guarded(modal, "dialog_node_options", || {
                    crate::dialogs::render_node_options_dialog(ctx, ui_mgr, hierarchy, db_tx, nav, local_role)
                });
                let _ = guarded(modal, "dialog_entity_settings", || {
                    crate::dialogs::render_entity_settings_dialog(ctx, ui_mgr, db_tx, local_role,known_peers,inspector.api_endpoint)
                });
                // FR-2 (shell_ux_sidebar): the Map Manager is a right-sidebar
                // section now (`ui_shell::right_sidebar::render_maps_section`),
                // not a floating dialog — removed from the transient family here.
                let _ = guarded(modal, "dialog_petal_manifest", || {
                    crate::dialogs::render_petal_manifest(ctx, ui_mgr, nav, db_tx)
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
                    crate::dialogs::render_context_menu(
                        ctx,
                        ui_mgr,
                        hierarchy,
                        stamp_state,
                        tool_panel,
                        &*path_state,
                        db_tx,
                        nav.active_petal_id.as_deref(),
                        nav,
                        local_role,
                    )
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
    surface::enter(ctx, Producer::Data, !gis_panel.open);
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
            vis_state,
            app_settings,
            local_role,
            inspector.api_endpoint,
        )
    });

    // Toast overlay (bottom-left, semi-transparent) — last in
    // `transient_order()`, painted topmost.
    if visibility.toast {
        let _ = guarded(modal, "toast", || render_toast(ctx, ui_mgr));
    }

    crate::native_accessibility::product_guard(
        ctx, ui_mgr, node_mgr, nav, hierarchy, local_role, topbar_state, modal, path_state);
    crate::native_accessibility::navigation_ui::finish(ctx, db_tx);
    surface::finish(ctx, crate::native_accessibility::product_guard(
        ctx, ui_mgr, node_mgr, nav, hierarchy, local_role, topbar_state, modal, path_state),
        node_mgr.selection_count() == 0 && inspector.active_tab() == crate::plugin::InspectorTab::Properties
        && !ui_mgr.portal_is_open() && !gis_panel.open && !inspector.query_results.open
        && identity_banner.message.is_none() && modal.last_error.is_none()
        && ui_mgr.active_notice(ctx.input(|input| input.time)).is_none());
    crate::native_accessibility::finish_pass(ctx, crate::native_accessibility::product_guard(
        ctx, ui_mgr, node_mgr, nav, hierarchy, local_role, topbar_state, modal, path_state));
    viewport_response.response.rect
}

fn render_toast(ctx: &egui::Context, ui_mgr: &mut UiManager) {
    let now = ctx.input(|i| i.time);
    let Some((notice, alpha)) = ui_mgr.active_notice(now) else {
        return;
    };
    let id = notice.id;
    let is_error = notice.severity == crate::notice::Severity::Error;
    let mut dismissed = false;
    let area = egui::Area::new(egui::Id::new("toast_overlay"))
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::LEFT_BOTTOM, egui::vec2(16.0, -40.0))
        .interactable(true)
        .show(ctx, |ui| {
            ui.set_max_width((ctx.viewport_rect().width() - 32.0).clamp(80.0, 560.0));
            egui::Frame::NONE
                .fill(crate::theme::notice_fill(notice.severity).gamma_multiply(alpha))
                .inner_margin(egui::Margin::symmetric(12, 8))
                .corner_radius(6.0)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let text = egui::RichText::new(&notice.text)
                            .color(crate::theme::NOTICE_TEXT.gamma_multiply(alpha));
                        let text = if is_error { text } else { text.small() };
                        let action_width = if is_error { 76.0 } else { 0.0 };
                        ui.allocate_ui_with_layout(
                            egui::vec2((ui.available_width() - action_width).max(24.0), 0.0),
                            egui::Layout::top_down(egui::Align::Min),
                            |ui| {
                                ui.add(egui::Label::new(text).wrap())
                                    .named(ui, format_args!("Notice: {}", notice.text));
                            },
                        );
                        if is_error {
                            dismissed = ui
                                .add(egui::Button::new("Dismiss").fill(crate::theme::BG_PRIMARY))
                                .named(ui, format_args!("Dismiss notice {id}"))
                                .clicked();
                        }
                    });
                });
        });
    if dismissed {
        ui_mgr.dismiss_notice(id);
    } else if is_error && area.response.contains_pointer() {
        ui_mgr.hold_notice(id, now);
    }
    ctx.request_repaint();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::panels::toolbar::Tool;
    use crate::terrain_proposal_state::ProposalOp;
    use crate::ui_shell::right_sidebar::{RightSidebarSection, RightSidebarState};

    #[test]
    fn gardener_console_render_smoke_sanitizes_all_numeric_tool_surfaces() {
        crate::viewport::browser_access_render_journey();
        {
            use crate::verse_manager::{FractalEntry, NodeEntry, PetalEntry, VerseEntry};
            use fe_runtime::messages::DbResult;
            let ctx = egui::Context::default();
            ctx.enable_accesskit();
            let mut tree = VerseManager::default();
            tree.verses.push(VerseEntry {
                id: "v".into(),
                fractals: vec![FractalEntry {
                    id: "f".into(),
                    petals: vec![PetalEntry {
                        id: "p".into(),
                        nodes: vec![NodeEntry {
                            id: "n".into(),
                            ..Default::default()
                        }],
                        ..Default::default()
                    }],
                    ..Default::default()
                }],
                ..Default::default()
            });
            let mut nav = NavigationManager::default();
            nav.navigate_to_petal_in_fractal("v", "Verse", "f", "Fractal", "p");
            let mut nodes = crate::node_manager::NodeManager::default();
            nodes.select(bevy::prelude::Entity::PLACEHOLDER, "n");
            let role = crate::plugin::LocalUserRole {
                role: Some(fe_policy::RoleLevel::Viewer),
            };
            let mut state = crate::object_data::ObjectDataState::default();
            crate::object_data::synchronize(&mut state, &nodes, &tree, &nav, &role);
            let (tx, rx) = crossbeam::channel::unbounded();
            let mut notices = UiManager::default();
            let mut frame = |state: &mut crate::object_data::ObjectDataState,
                             events: Vec<egui::Event>| {
                ctx.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(900.0, 1100.0),
                        )),
                        events,
                        ..Default::default()
                    },
                    |ctx| {
                        egui::CentralPanel::default().show(ctx, |ui| {
                            crate::object_data::render(ui, state, &role, &tx, &mut notices)
                        });
                    },
                )
            };
            let bounds = |output: &egui::FullOutput, name: &str| {
                let rect = output
                    .platform_output
                    .accesskit_update
                    .as_ref()
                    .unwrap()
                    .nodes
                    .iter()
                    .find_map(|(_, node)| {
                        (node.label() == Some(name))
                            .then(|| node.bounds())
                            .flatten()
                    })
                    .unwrap_or_else(|| panic!("missing actual native control {name}"));
                egui::pos2(
                    ((rect.x0 + rect.x1) / 2.0) as f32,
                    ((rect.y0 + rect.y1) / 2.0) as f32,
                )
            };
            let click = |point: egui::Pos2, pressed| {
                vec![
                    egui::Event::PointerMoved(point),
                    egui::Event::PointerButton {
                        pos: point,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ]
            };
            frame(&mut state, vec![]);
            let DbCommand::ReadNodeHistory {
                correlation_id: Some(page_id),
                at_version: None,
                ..
            } = rx.try_recv().unwrap()
            else {
                panic!("native history must request a page")
            };
            let coverage = serde_json::json!({"node_id":"n","journal_version":1,"current_version":4,"coverage_start_version":1,"coverage_started_at":"now","supported_operations":["baseline","property_set"],"order":"per_object_commit_version","event_time_ordering":false,"current_state_verified":true});
            let save = serde_json::json!({"batch_id":"save-n","first_version":2,"committed_version":4});
            let mut wire = coverage.clone();
            wire["next_after_version"] = serde_json::Value::Null;
            wire["entries"] = serde_json::json!([{"log_id":"fact2","node_id":"n","hlc_timestamp":42,"op":"property_set","source_did":"actor","payload":{},"row_version":2,"created_at":"now","atomic_save":save,"replay_availability":"inside_atomic_batch"}]);
            assert!(crate::object_data::route(
                &mut state,
                &DbResult::NodeHistoryLoaded {
                    page: serde_json::from_value(wire).unwrap(),
                    correlation_id: Some(page_id)
                }
            ));
            let output = frame(&mut state, vec![]);
            assert!(output.shapes.iter().any(|shape| matches!(&shape.shape,egui::epaint::Shape::Text(text) if text.galley.text().contains("saved together"))));
            let row = bounds(&output, "Replay object version 4");
            frame(&mut state, click(row, true));
            frame(&mut state, click(row, false));
            let DbCommand::ReadNodeHistory {
                correlation_id: Some(preview_id),
                at_version: Some(4),
                ..
            } = rx.try_recv().unwrap()
            else {
                panic!("an interior row must request its completed Save")
            };
            let mut preview = coverage.clone();
            preview["at_version"] = serde_json::json!(4);
            preview["availability"] = serde_json::json!("available");
            preview["atomic_save"] = save.clone();
            preview["baseline_version"] = serde_json::json!(1);
            preview["state"] = serde_json::json!({"node_id":"n","petal_id":"p","display_name":"Object","asset_id":null,"position":[0,0,0],"elevation":0,"rotation":[0,0,0],"scale":[1,1,1],"interactive":true,"created_at":"now","edit_seq":0,"properties":{},"tombstone":null});
            assert!(crate::object_data::route(
                &mut state,
                &DbResult::NodeStateAtVersionLoaded {
                    state: serde_json::from_value(preview).unwrap(),
                    correlation_id: Some(preview_id)
                }
            ));
            let output = frame(&mut state, vec![]);
            let earlier = bounds(&output, "Replay earlier object version");
            frame(&mut state, click(earlier, true));
            frame(&mut state, click(earlier, false));
            let DbCommand::ReadNodeHistory {
                correlation_id: Some(prior_id),
                at_version: Some(1),
                ..
            } = rx.try_recv().unwrap()
            else {
                panic!("Earlier must skip the entire saved group")
            };
            let mut unavailable = coverage.clone();
            unavailable["at_version"] = serde_json::json!(1);
            unavailable["availability"] = serde_json::json!("before_coverage");
            assert!(crate::object_data::route(
                &mut state,
                &DbResult::NodeStateAtVersionLoaded {
                    state: serde_json::from_value(unavailable).unwrap(),
                    correlation_id: Some(prior_id)
                }
            ));
            let output = frame(&mut state, vec![]);
            let input = bounds(&output, "Object replay version");
            frame(&mut state, click(input, true));
            frame(&mut state, click(input, false));
            frame(
                &mut state,
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
            );
            let output = frame(&mut state, vec![]);
            let read = bounds(&output, "Read exact object version");
            frame(&mut state, click(read, true));
            frame(&mut state, click(read, false));
            let DbCommand::ReadNodeHistory {
                correlation_id: Some(interior_id),
                at_version: Some(2),
                ..
            } = rx.try_recv().unwrap()
            else {
                panic!("manual exact requests must retain their requested version")
            };
            let mut unavailable = coverage;
            unavailable["at_version"] = serde_json::json!(2);
            unavailable["availability"] = serde_json::json!("inside_atomic_batch");
            unavailable["atomic_save"] = save;
            assert!(crate::object_data::route(
                &mut state,
                &DbResult::NodeStateAtVersionLoaded {
                    state: serde_json::from_value(unavailable).unwrap(),
                    correlation_id: Some(interior_id)
                }
            ));
            let output = frame(&mut state, vec![]);
            let texts: Vec<_> = output
                .shapes
                .iter()
                .filter_map(|shape| match &shape.shape {
                    egui::epaint::Shape::Text(text) => Some(text.galley.text()),
                    _ => None,
                })
                .collect();
            assert!(
                texts
                    .iter()
                    .any(|text| text.contains("never a separately committed object state"))
            );
            assert!(
                !texts.iter().any(|text| text.starts_with("Position:")),
                "an unavailable interior cannot paint a partial transform"
            );
            let completed = bounds(&output, "Read completed atomic Save");
            frame(&mut state, click(completed, true));
            frame(&mut state, click(completed, false));
            assert!(matches!(
                rx.try_recv().unwrap(),
                DbCommand::ReadNodeHistory {
                    at_version: Some(4),
                    ..
                }
            ));
            assert_eq!(
                tree.find_petal("p").unwrap().nodes[0].position,
                [0.0; 3],
                "read-only browsing cannot materialize historical field steps"
            );
        }
        let ctx = egui::Context::default();
        let mut sidebar = crate::plugin::SidebarState::default();
        let mut tool = crate::plugin::ToolState {
            active_tool: Tool::Brush,
            ..Default::default()
        };
        let mut inspector = crate::plugin::InspectorFormState::default();
        let mut nav = NavigationManager {
            active_petal_id: Some("petal-1".into()),
            ..Default::default()
        };
        let dashboard = crate::atlas::DashboardState::default();
        let mut hierarchy = VerseManager::default();
        let (db_tx, _db_rx) = crossbeam::channel::unbounded();
        let mut camera_focus = crate::plugin::CameraFocusTarget::default();
        let cursor = crate::plugin::ViewportCursorWorld::default();
        let mut node_mgr = crate::node_manager::NodeManager::default();
        let mut ui_mgr = UiManager::default();
        let role = crate::plugin::LocalUserRole::default();
        let mut petal_map = crate::terrain_map::PetalMapState::default();
        let asset_status = AssetDownloadStatus::default();
        let mut gis_panel = crate::gis::GisPanelState::default();
        let gpx_status = crate::gpx_ops::GpxImportStatus::default();
        let mut path_state = crate::gis::PathEditorState {
            editing_track_id: Some("track-1".into()),
            points: vec![crate::gis::PathPointRow {
                position: [0.0, f32::NAN, 0.0],
                ..Default::default()
            }],
            snap: crate::gis::PathSnapSettings {
                enabled: true,
                grid_step_m: f32::NAN,
                anchor_radius_m: f32::INFINITY,
            },
            edited_track_style: crate::gis::TrackStyleFields {
                width: f32::INFINITY,
                ..Default::default()
            },
            ..Default::default()
        };
        let path_status = crate::path_ops::PathEditStatus::default();
        let mut tool_panel = {
            let mut value = crate::panels::tool_panel::ToolPanelState::default();
            value.spacing_value = f32::NAN;
            value.shape_radius = f32::INFINITY;
            value.shape_radius_z = f32::NAN;
            value.stamp_scale = f32::NEG_INFINITY;
            value.stamp_yaw_deg = f32::NAN;
            value.stamp_arc_m = f32::INFINITY;
            value.terrain_footprint_radius = f32::NAN;
            value.terrain_target_height = f32::INFINITY;
            value.terrain_delta = f32::NEG_INFINITY;
            value
        };
        let mut proposal_state = crate::terrain_proposal_state::ProposalEditState::default();
        proposal_state.push_new(
            ProposalOp::Raise,
            vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]],
            None,
            Some(1.0),
        );
        let mut app_settings = crate::settings::AppSettings {
            render_distance: f32::INFINITY,
            ..Default::default()
        };
        let mut sculpt = crate::actions::terrain_proposal::SculptToolState {
            radius: f32::NAN,
            strength: f32::INFINITY,
            target_height: f32::NEG_INFINITY,
            delta: f32::NAN,
            ..Default::default()
        };
        let mut ts_settings = crate::timeseries_settings::TimeseriesSettingsState::default();
        let stamp_state = crate::actions::asset::StampInteractionState::default();
        let mut topbar = crate::ui_shell::topbar::TopbarState::default();
        let mut left = crate::ui_shell::left_sidebar::LeftSidebarState::default();
        let mut right = RightSidebarState {
            requested: Some(RightSidebarSection::Options),
            ..Default::default()
        };
        let mut modal = crate::ui_shell::modal::ModalManagerState::default();
        let mut vis_state = crate::visibility::VisibilityState::default();
        let mut measure_state = crate::node_manager::measure::MeasureState::default();
        let downloads = crate::terrain_map::TilesetDownloads::default();
        let mut render_override = crate::settings::PetalRenderOverride::default();
        let mut identity_banner = crate::plugin::IdentityBanner::default();
        let volume_report = crate::panels::volume_report_panel::VolumeReportState::default();

        {
            let mut render = |section: Option<RightSidebarSection>, active_tool: Tool| {
                right.requested = section;
                tool.active_tool = active_tool;
                let raw = egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1280.0, 800.0),
                    )),
                    ..Default::default()
                };
                let _ = ctx.run(raw, |ctx| {
                    gardener_console(
                        ctx,
                        &mut sidebar,
                        &mut tool,
                        &mut inspector,
                        &mut nav,
                        &dashboard,
                        &mut hierarchy,
                        &db_tx,
                        &mut camera_focus,
                        &cursor,
                        None,
                        &mut node_mgr,
                        &mut ui_mgr,
                        &role,
                        &mut petal_map,
                        &asset_status,
                        &mut gis_panel,
                        &gpx_status,
                        &mut path_state,
                        &path_status,
                        &mut tool_panel,
                        &mut proposal_state,
                        &mut app_settings,
                        &mut ts_settings,
                        &mut sculpt,
                        &stamp_state,
                        &mut topbar,
                        &mut left,
                        &mut right,
                        &mut modal,
                        &mut vis_state,
                        &mut measure_state,
                        &downloads,
                        &mut render_override,
                        &mut identity_banner,
                        None,
                        None,
                        &volume_report,
                        &mut crate::settings::SettingsSession::default(),
                    );
                });
            };

            // Every section, so no arm escapes the numeric-sanitize sweep.
            render(None, Tool::Brush);
            for section in crate::ui_shell::right_sidebar::ALL_SECTIONS {
                render(Some(section), Tool::Brush);
            }
            // Plus every Options arm — the dispatcher is the new numeric
            // boundary for the pen/stamp/sculpt buffers (D6-B/D7/D8).
            for t in [
                Tool::Select,
                Tool::Move,
                Tool::Pen,
                Tool::Brush,
                Tool::Measure,
            ] {
                render(Some(RightSidebarSection::Options), t);
            }
        }

        assert!(tool_panel.shape_radius_z.is_finite());
        assert!(tool_panel.stamp_scale.is_finite());
        assert!(tool_panel.terrain_footprint_radius.is_finite());
        assert!(sculpt.radius.is_finite());
        assert!(sculpt.strength.is_finite());
        assert!(path_state.points[0].position[1].is_finite());
        assert!(path_state.snap.grid_step_m.is_finite());
        assert!(path_state.snap.anchor_radius_m.is_finite());
        assert!(path_state.edited_track_style.width.is_finite());
        assert!(app_settings.render_distance.is_finite());

        let mut notices = UiManager::default();
        notices.show_error("Import failed: choose a readable file", 100.0);
        let notice_id = notices.active_notice(100.0).unwrap().0.id;
        let frame = |time: f64, events: Vec<egui::Event>, manager: &mut UiManager| {
            crate::ui_shell::layout_policy::begin_frame(&ctx, true);
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1280.0, 800.0),
                    )),
                    time: Some(time),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    crate::ui_shell::layout_policy::begin_ui_pass(ctx);
                    render_toast(ctx, manager);
                },
            );
            crate::ui_shell::layout_policy::observations(&ctx)
        };
        frame(100.1, vec![], &mut notices);
        let controls = frame(100.2, vec![], &mut notices);
        // The named notice label also senses clicks; Dismiss is the trailing action.
        let dismiss = controls
            .controls
            .iter()
            .rev()
            .find(|c| c.clickable)
            .expect("visible Dismiss action");
        let point = dismiss.hit.intersect(dismiss.clip).center();
        assert!(dismiss.allowed && dismiss.hit.contains(point));
        frame(100.3, vec![egui::Event::PointerMoved(point)], &mut notices);
        frame(108.0, vec![egui::Event::PointerMoved(point)], &mut notices);
        assert_eq!(
            notices.active_notice(108.0).unwrap().0.id,
            notice_id,
            "hover pins the rendered error past its lifetime"
        );
        notices.show_toast("Another action completed", 108.0);
        for (time, pressed) in [(108.1, true), (108.2, false)] {
            frame(
                time,
                vec![egui::Event::PointerButton {
                    pos: point,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                }],
                &mut notices,
            );
        }
        assert_eq!(
            notices.active_notice(108.2).unwrap().0.text,
            "Another action completed"
        );
        assert_eq!(
            notices.error_history().count(),
            1,
            "dismiss retains the error history"
        );
        assert!(notices.latest_unacknowledged_error().is_none());

        let luminance = |color: egui::Color32| {
            let channels = [color.r(), color.g(), color.b()].map(|v| {
                let v = f64::from(v) / 255.0;
                if v <= 0.04045 {
                    v / 12.92
                } else {
                    ((v + 0.055) / 1.055).powf(2.4)
                }
            });
            channels[0] * 0.2126 + channels[1] * 0.7152 + channels[2] * 0.0722
        };
        for foreground in [
            crate::theme::TEXT_DIM,
            crate::theme::TEXT_MUTED,
            crate::theme::STATUS_NORMAL,
        ] {
            for background in [
                crate::theme::BG_PANEL,
                crate::theme::BG_STATUSBAR,
                crate::theme::BG_DIALOG,
                crate::theme::BG_BUTTON,
            ] {
                assert!((luminance(foreground) + 0.05) / (luminance(background) + 0.05) >= 4.5);
            }
        }
        for severity in [
            crate::notice::Severity::Info,
            crate::notice::Severity::Advisory,
            crate::notice::Severity::Error,
        ] {
            assert!(
                (luminance(crate::theme::NOTICE_TEXT) + 0.05)
                    / (luminance(crate::theme::notice_fill(severity)) + 0.05)
                    >= 4.5
            );
        }
    }
}
