//! Data panel ("Data — Query, Layers & Export"): a floating window with a
//! Query tab (three query modes — nodes with annotations, property key/value
//! filter, local-coords bbox, plus GPX import), an Annotations tab (all
//! annotated nodes in the active petal), and a Layers tab
//! (`layer_manager_card`, view mode). See `fe-ui/src/AGENTS.md`
//! §gis-query-ui.

use bevy_egui::egui;

use crate::accessibility::ResponseExt;

use crate::actions::{UiAction, UiManager};
use crate::gis::{self, GisPanelState, GisPanelTab, GisQueryMode};
use crate::gpx_ops::GpxImportStatus;
use crate::navigation_manager::NavigationManager;
use crate::node_manager::NodeManager;
use crate::panels::widgets::{meters_to_world, world_to_meters};
use crate::path_ops::PathEditStatus;
use crate::plugin::{CameraFocusTarget, ViewportCursorWorld};
use crate::terrain_map::PetalMapState;
use crate::theme;
use crate::verse_manager::VerseManager;

/// Default half-extent (meters) for the "Center on Selection"/"Center
/// on Origin" bbox shortcuts.
const DEFAULT_BBOX_HALF_EXTENT: f32 = 50.0;

pub(crate) fn render_gis_panel(
    ctx: &egui::Context,
    gis_state: &mut GisPanelState,
    node_mgr: &mut NodeManager,
    verse_mgr: &VerseManager,
    nav: &NavigationManager,
    petal_map: &mut PetalMapState,
    ui_mgr: &mut UiManager,
    camera_focus: &mut CameraFocusTarget,
    gpx_status: &GpxImportStatus,
    path_state: &mut gis::PathEditorState,
    path_status: &PathEditStatus,
    cursor_world: &ViewportCursorWorld,
    // VIS-PH5: the unified overlay registry's Group + ruler-overlay rows.
    visibility: &crate::visibility::VisibilityState,
    app_settings: &crate::settings::AppSettings,
    local_role: &crate::plugin::LocalUserRole,
    api_endpoint: Option<std::net::SocketAddr>,
) {
    gis_state.sync_petal(nav.active_petal_id.as_deref());
    if !gis_state.open {
        return;
    }

    let world_scale = petal_map.world_scale;
    let mut still_open = true;
    crate::ui_shell::layout_policy::bounded_window(ctx, "Data — Query, Layers & Export")
        .open(&mut still_open)
        .resizable(true)
        .default_width(360.0)
        .min_width(crate::ui_shell::layout_policy::dialog_width(ctx, 300.0))
        .frame(
            egui::Frame::NONE
                .fill(theme::BG_DIALOG)
                .inner_margin(egui::Margin::same(12))
                .corner_radius(6.0)
                .stroke(egui::Stroke::new(1.0_f32, theme::TEXT_DIM)),
        )
        .show(ctx, |ui| {
            let Some(petal_id) = nav.active_petal_id.clone() else {
                ui.label(
                    egui::RichText::new("Navigate to a petal to query its nodes.")
                        .color(theme::TEXT_MUTED)
                        .italics(),
                );
                return;
            };

            render_tab_bar(ui, gis_state);
            ui.add_space(4.0);
            ui.separator();
            ui.add_space(4.0);

            match gis_state.active_tab {
                GisPanelTab::Query => {
                    render_query_section(
                        ui,
                        gis_state,
                        node_mgr,
                        verse_mgr,
                        ui_mgr,
                        camera_focus,
                        &petal_id,
                        gpx_status,
                        world_scale,
                        nav.active_verse_id.as_deref(),
                    );
                }
                GisPanelTab::Annotations => {
                    render_annotations_tab(
                        ui,
                        gis_state,
                        node_mgr,
                        verse_mgr,
                        ui_mgr,
                        camera_focus,
                        &petal_id,
                        world_scale,
                    );
                }
                GisPanelTab::Layers => {
                    crate::panels::layer_manager_card::layer_manager_section(
                        ui,
                        petal_map,
                        ui_mgr,
                        &petal_id,
                        visibility,
                        app_settings,
                        local_role,
                    );
                }
                GisPanelTab::Paths => {
                    // Auto-populate the track list on first view (empty + idle +
                    // not editing) so persisted tracks show without a manual
                    // Refresh; the pending flag prevents a per-frame query loop.
                    if path_state.editing_track_id.is_none()
                        && path_state.tracks.is_empty()
                        && !path_state.tracks_pending
                        && path_state.last_error.is_none()
                    {
                        ui_mgr.push_action(UiAction::PathQueryTracks {
                            petal_id: petal_id.clone(),
                        });
                    }
                    crate::panels::path_editor_card::path_editor_section(
                        ui,
                        path_state,
                        path_status,
                        ui_mgr,
                        cursor_world,
                        &petal_id,
                        nav,
                        world_scale,
                        local_role.role.is_some_and(|role| role.can_edit()),
                    );
                }
                GisPanelTab::Export => {
                    crate::panels::egress_card::egress_section(
                        ui,
                        gis_state,
                        node_mgr,
                        ui_mgr,
                        &petal_id,
                        world_scale,
                        api_endpoint,
                    );
                }
            }
        });

    if !still_open {
        gis_state.open = false;
    }
}

fn render_tab_bar(ui: &mut egui::Ui, gis_state: &mut GisPanelState) {
    ui.horizontal_wrapped(|ui| {
        for (tab, label) in [
            (GisPanelTab::Query, "Query"),
            (GisPanelTab::Annotations, "Annotations"),
            (GisPanelTab::Layers, "Layers"),
            (GisPanelTab::Paths, "Paths"),
            (GisPanelTab::Export, "Export"),
        ] {
            let active = gis_state.active_tab == tab;
            let btn = egui::Button::new(egui::RichText::new(label).small().color(if active {
                theme::TEXT_BRIGHT
            } else {
                theme::TEXT_DIM
            }))
            .fill(theme::tab_fill(active));
            if ui.add(btn).observed(ui).clicked() {
                gis_state.active_tab = tab;
            }
        }
    });
}

/// Renders the petal-scoped annotation read model.
fn render_annotations_tab(
    ui: &mut egui::Ui,
    gis_state: &mut GisPanelState,
    node_mgr: &mut NodeManager,
    _verse_mgr: &VerseManager,
    ui_mgr: &mut UiManager,
    camera_focus: &mut CameraFocusTarget,
    petal_id: &str,
    world_scale: f64,
) {
    let state = &gis_state.annotations;
    let auto_load = !state.loaded && state.request_id.is_none() && state.error.is_none();
    let refresh = ui
        .horizontal(|ui| {
            ui.strong("Annotated Nodes");
            ui.add_enabled(state.request_id.is_none(), egui::Button::new("Refresh"))
                .named(ui, "Refresh annotations")
                .clicked()
        })
        .inner;
    if auto_load || refresh {
        // Reserve before dispatch so another frame cannot queue the same load.
        gis_state.annotations.loaded = true;
        ui_mgr.push_action(UiAction::GisRefreshAnnotations {
            petal_id: petal_id.to_owned(),
        });
    }
    let state = &gis_state.annotations;
    if state.request_id.is_some() {
        ui.label("Loading annotations...");
    }
    if let Some(error) = &state.error {
        ui.colored_label(theme::STATUS_OFFLINE, error);
    }
    render_result_rows(
        ui,
        &state.rows,
        state.loaded && state.request_id.is_none() && state.error.is_none(),
        node_mgr,
        camera_focus,
        world_scale,
    );
}

fn render_query_section(
    ui: &mut egui::Ui,
    gis_state: &mut GisPanelState,
    node_mgr: &mut NodeManager,
    verse_mgr: &VerseManager,
    ui_mgr: &mut UiManager,
    camera_focus: &mut CameraFocusTarget,
    petal_id: &str,
    gpx_status: &GpxImportStatus,
    world_scale: f64,
    verse_id: Option<&str>,
) {
    ui.label(
        egui::RichText::new("Query")
            .strong()
            .color(theme::TEXT_SECTION),
    );
    ui.add_space(4.0);

    ui.horizontal(|ui| {
        for (mode, label) in [
            (GisQueryMode::Annotated, "Annotated"),
            (GisQueryMode::PropertyFilter, "Property"),
            (GisQueryMode::Bbox, "Bbox"),
        ] {
            let active = gis_state.mode == mode;
            let btn = egui::Button::new(label).fill(if active {
                theme::BG_BUTTON_ACTIVE
            } else {
                theme::BG_BUTTON
            });
            if ui.add(btn).observed(ui).clicked() {
                gis_state.mode = mode;
            }
        }
    });
    ui.add_space(6.0);

    match gis_state.mode {
        GisQueryMode::Annotated => {
            ui.label(
                egui::RichText::new("Every node in this petal with a saved Annotation title.")
                    .small()
                    .color(theme::TEXT_MUTED),
            );
        }
        GisQueryMode::PropertyFilter => {
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut gis_state.filter_key_buf)
                        .hint_text("key")
                        .desired_width(90.0),
                )
                .named(ui, "Property filter key");
                ui.add(
                    egui::TextEdit::singleline(&mut gis_state.filter_value_buf)
                        .hint_text("value")
                        .desired_width(90.0),
                )
                .named(ui, "Property filter value");
                egui::ComboBox::from_id_salt("gis_filter_value_type")
                    .selected_text(gis_state.filter_value_type_buf.clone())
                    .width(70.0)
                    .show_ui(ui, |ui| {
                        for t in ["string", "number", "bool"] {
                            ui.selectable_value(
                                &mut gis_state.filter_value_type_buf,
                                t.to_string(),
                                t,
                            )
                            .observed(ui);
                        }
                    })
                    .response
                    .named(ui, "Property filter value type");
            });
        }
        GisQueryMode::Bbox => {
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("Min x,z (m)")
                        .small()
                        .color(theme::TEXT_DIM),
                );
                ui.add(egui::TextEdit::singleline(&mut gis_state.bbox_min[0]).desired_width(50.0))
                    .named(ui, "Bounding box minimum X in meters");
                ui.add(egui::TextEdit::singleline(&mut gis_state.bbox_min[1]).desired_width(50.0))
                    .named(ui, "Bounding box minimum Z in meters");
            });
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("Max x,z (m)")
                        .small()
                        .color(theme::TEXT_DIM),
                );
                ui.add(egui::TextEdit::singleline(&mut gis_state.bbox_max[0]).desired_width(50.0))
                    .named(ui, "Bounding box maximum X in meters");
                ui.add(egui::TextEdit::singleline(&mut gis_state.bbox_max[1]).desired_width(50.0))
                    .named(ui, "Bounding box maximum Z in meters");
            });
            ui.horizontal(|ui| {
                if ui
                    .small_button("Center on Selection")
                    .observed(ui)
                    .clicked()
                {
                    let center = node_mgr
                        .selected
                        .as_ref()
                        .and_then(|sel| verse_mgr.all_nodes().find(|n| n.id == sel.node_id))
                        .map(|n| {
                            [
                                world_to_meters(n.position[0], world_scale),
                                world_to_meters(n.position[2], world_scale),
                            ]
                        })
                        .unwrap_or([0.0, 0.0]);
                    gis_state.center_bbox_on(center, DEFAULT_BBOX_HALF_EXTENT);
                }
                if ui.small_button("Center on Origin").observed(ui).clicked() {
                    gis_state.center_bbox_on([0.0, 0.0], DEFAULT_BBOX_HALF_EXTENT);
                }
            });
        }
    }

    ui.add_space(6.0);
    let run_label = if gis_state.query_pending {
        "Running..."
    } else {
        "Run Query"
    };
    let can_run = !gis_state.query_pending;
    if ui
        .add_enabled(can_run, egui::Button::new(run_label).fill(theme::BG_SAVE))
        .observed(ui)
        .clicked()
    {
        run_query(gis_state, verse_mgr, ui_mgr, petal_id, world_scale);
    }

    if let Some(err) = &gis_state.last_error {
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new(err)
                .small()
                .color(theme::STATUS_OFFLINE),
        );
    }

    ui.add_space(6.0);
    ui.separator();
    ui.add_space(4.0);
    render_results(ui, gis_state, node_mgr, camera_focus, world_scale);

    ui.add_space(6.0);
    ui.separator();
    ui.add_space(4.0);
    crate::panels::gpx_import_card::gpx_import_section(ui, ui_mgr, gpx_status, petal_id, verse_id);
}

/// Dispatches the active query mode: DB round-trip (annotated/property) via
/// `UiAction`, or an instant client-side filter over `VerseManager` (bbox —
/// no RawQuery precedent was needed since node positions are already
/// in-memory; see AGENTS.md residual note).
fn run_query(
    gis_state: &mut GisPanelState,
    verse_mgr: &VerseManager,
    ui_mgr: &mut UiManager,
    petal_id: &str,
    world_scale: f64,
) {
    gis_state.results.clear();
    gis_state.query_completed = false;
    gis_state.last_error = None;
    match gis_state.mode {
        GisQueryMode::Annotated => {
            ui_mgr.push_action(UiAction::GisQueryAnnotated {
                petal_id: petal_id.to_string(),
            });
        }
        GisQueryMode::PropertyFilter => {
            let key = gis_state.filter_key_buf.trim().to_string();
            if key.is_empty() {
                gis_state.last_error = Some("Enter a property key".to_string());
                return;
            }
            let value = gis::parse_filter_value(
                &gis_state.filter_value_type_buf,
                gis_state.filter_value_buf.trim(),
            );
            ui_mgr.push_action(UiAction::GisQueryPropertyFilter {
                petal_id: petal_id.to_string(),
                key,
                value,
            });
        }
        GisQueryMode::Bbox => {
            // Bbox fields are meters; node positions are world units.
            let (Some(min_m), Some(max_m)) = (
                gis::parse_bbox_fields(&gis_state.bbox_min),
                gis::parse_bbox_fields(&gis_state.bbox_max),
            ) else {
                gis_state.last_error = Some("Bbox fields must be numbers".to_string());
                return;
            };
            let min = min_m.map(|m| meters_to_world(m, world_scale));
            let max = max_m.map(|m| meters_to_world(m, world_scale));
            let Some(petal) = verse_mgr.find_petal(petal_id) else {
                return;
            };
            gis_state.results = petal
                .nodes
                .iter()
                .filter(|n| gis::bbox_contains(n.position, min, max))
                .map(|n| crate::gis::GisResultRow {
                    node_id: n.id.clone(),
                    name: n.name.clone(),
                    position: n.position,
                    annotation_title: None,
                    annotation_color: None,
                })
                .collect();
            gis_state.query_completed = true;
        }
    }
}

fn render_results(
    ui: &mut egui::Ui,
    gis_state: &GisPanelState,
    node_mgr: &mut NodeManager,
    camera_focus: &mut CameraFocusTarget,
    world_scale: f64,
) {
    render_result_rows(
        ui,
        &gis_state.results,
        gis_state.query_completed && !gis_state.query_pending && gis_state.last_error.is_none(),
        node_mgr,
        camera_focus,
        world_scale,
    );
}

fn render_result_rows(
    ui: &mut egui::Ui,
    rows: &[gis::GisResultRow],
    completed: bool,
    node_mgr: &mut NodeManager,
    camera_focus: &mut CameraFocusTarget,
    world_scale: f64,
) {
    ui.label(
        egui::RichText::new(format!("Results ({})", rows.len()))
            .strong()
            .color(theme::TEXT_SECTION),
    );
    ui.add_space(4.0);

    if rows.is_empty() {
        ui.label(
            egui::RichText::new(if completed {
                "0 results"
            } else {
                "No results yet."
            })
            .small()
            .color(theme::TEXT_MUTED)
            .italics(),
        );
        return;
    }

    let mut clicked_row: Option<(String, [f32; 3])> = None;
    egui::ScrollArea::vertical()
        .max_height(220.0)
        .show(ui, |ui| {
            for row in rows {
                egui::Frame::NONE
                    .fill(theme::BG_PEER_ROW_EVEN)
                    .inner_margin(egui::Margin::symmetric(6, 4))
                    .corner_radius(2.0)
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            if let Some((r, g, b)) = row
                                .annotation_color
                                .as_deref()
                                .and_then(crate::panels::annotation_card::parse_hex_color)
                            {
                                let (rect, _) = ui.allocate_exact_size(
                                    egui::vec2(12.0, 12.0),
                                    egui::Sense::hover(),
                                );
                                ui.painter().rect_filled(
                                    rect,
                                    2.0,
                                    egui::Color32::from_rgb(r, g, b),
                                );
                            }
                            let label = egui::RichText::new(&row.name).color(theme::TEXT_BRIGHT);
                            if ui
                                .add(egui::Label::new(label).sense(egui::Sense::click()))
                                .button_named(
                                    ui,
                                    format_args!("Select and frame data node {}", row.name),
                                )
                                .clicked()
                            {
                                clicked_row = Some((row.node_id.clone(), row.position));
                            }
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.label(
                                        egui::RichText::new(format!(
                                            "({:.1}, {:.1}, {:.1}) m",
                                            world_to_meters(row.position[0], world_scale),
                                            world_to_meters(row.position[1], world_scale),
                                            world_to_meters(row.position[2], world_scale),
                                        ))
                                        .small()
                                        .color(theme::TEXT_DIM),
                                    );
                                },
                            );
                        });
                        if let Some(title) = &row.annotation_title {
                            ui.label(egui::RichText::new(title).small().color(theme::TEXT_MUTED));
                        }
                    });
                ui.add_space(1.0);
            }
        });

    // Reuse the existing sidebar-click selection mechanism (NodeManager's
    // pending_sidebar_select resolves to an Entity next frame) + the
    // existing camera-focus target — see fe-ui/src/panels/sidebar.rs.
    if let Some((node_id, pos)) = clicked_row {
        // camera_focus_clip_20260716 FR-2: carry node_id so apply_camera_focus
        // can prefer the live spawned transform over this cached fallback.
        node_mgr.request_select_and_frame(&node_id, pos, camera_focus);
    }
}
