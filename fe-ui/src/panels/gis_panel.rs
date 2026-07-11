//! GIS Query panel: a floating window with three query modes (nodes with
//! annotations, property key/value filter, local-coords bbox) plus an
//! embedded Layer Manager (`layer_manager_card`) for the active petal's
//! terrain layer stack. See `fe-ui/src/AGENTS.md` §gis-query-ui.

use bevy_egui::egui;

use crate::actions::{UiAction, UiManager};
use crate::gis::{self, GisPanelState, GisQueryMode};
use crate::navigation_manager::NavigationManager;
use crate::node_manager::NodeManager;
use crate::plugin::CameraFocusTarget;
use crate::terrain_map::PetalMapState;
use crate::theme;
use crate::verse_manager::VerseManager;

/// Default half-extent (world units) for the "Center on Selection"/"Center
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
) {
    if !gis_state.open {
        return;
    }

    let mut still_open = true;
    egui::Window::new("GIS Query & Layers")
        .open(&mut still_open)
        .resizable(true)
        .default_width(360.0)
        .min_width(300.0)
        .frame(
            egui::Frame::NONE
                .fill(theme::BG_DIALOG)
                .inner_margin(egui::Margin::same(12))
                .corner_radius(6.0)
                .stroke(egui::Stroke::new(1.0, theme::TEXT_DIM)),
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

            render_query_section(ui, gis_state, node_mgr, verse_mgr, ui_mgr, camera_focus, &petal_id);

            ui.add_space(8.0);
            ui.separator();
            ui.add_space(4.0);

            crate::panels::layer_manager_card::layer_manager_section(ui, petal_map, ui_mgr, &petal_id);
        });

    if !still_open {
        gis_state.open = false;
    }
}

fn render_query_section(
    ui: &mut egui::Ui,
    gis_state: &mut GisPanelState,
    node_mgr: &mut NodeManager,
    verse_mgr: &VerseManager,
    ui_mgr: &mut UiManager,
    camera_focus: &mut CameraFocusTarget,
    petal_id: &str,
) {
    ui.label(egui::RichText::new("Query").strong().color(theme::TEXT_SECTION));
    ui.add_space(4.0);

    ui.horizontal(|ui| {
        for (mode, label) in [
            (GisQueryMode::Annotated, "Annotated"),
            (GisQueryMode::PropertyFilter, "Property"),
            (GisQueryMode::Bbox, "Bbox"),
        ] {
            let active = gis_state.mode == mode;
            let btn = egui::Button::new(label)
                .fill(if active { theme::BG_BUTTON_ACTIVE } else { theme::BG_BUTTON });
            if ui.add(btn).clicked() {
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
                );
                ui.add(
                    egui::TextEdit::singleline(&mut gis_state.filter_value_buf)
                        .hint_text("value")
                        .desired_width(90.0),
                );
                egui::ComboBox::from_id_salt("gis_filter_value_type")
                    .selected_text(gis_state.filter_value_type_buf.clone())
                    .width(70.0)
                    .show_ui(ui, |ui| {
                        for t in ["string", "number", "bool"] {
                            ui.selectable_value(&mut gis_state.filter_value_type_buf, t.to_string(), t);
                        }
                    });
            });
        }
        GisQueryMode::Bbox => {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Min (x,z)").small().color(theme::TEXT_DIM));
                ui.add(egui::TextEdit::singleline(&mut gis_state.bbox_min[0]).desired_width(50.0));
                ui.add(egui::TextEdit::singleline(&mut gis_state.bbox_min[1]).desired_width(50.0));
            });
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Max (x,z)").small().color(theme::TEXT_DIM));
                ui.add(egui::TextEdit::singleline(&mut gis_state.bbox_max[0]).desired_width(50.0));
                ui.add(egui::TextEdit::singleline(&mut gis_state.bbox_max[1]).desired_width(50.0));
            });
            ui.horizontal(|ui| {
                if ui.small_button("Center on Selection").clicked() {
                    let center = node_mgr
                        .selected
                        .as_ref()
                        .and_then(|sel| verse_mgr.all_nodes().find(|n| n.id == sel.node_id))
                        .map(|n| [n.position[0], n.position[2]])
                        .unwrap_or([0.0, 0.0]);
                    gis_state.center_bbox_on(center, DEFAULT_BBOX_HALF_EXTENT);
                }
                if ui.small_button("Center on Origin").clicked() {
                    gis_state.center_bbox_on([0.0, 0.0], DEFAULT_BBOX_HALF_EXTENT);
                }
            });
        }
    }

    ui.add_space(6.0);
    let run_label = if gis_state.query_pending { "Running..." } else { "Run Query" };
    let can_run = !gis_state.query_pending;
    if ui
        .add_enabled(can_run, egui::Button::new(run_label).fill(theme::BG_SAVE))
        .clicked()
    {
        run_query(gis_state, verse_mgr, ui_mgr, petal_id);
    }

    if let Some(err) = &gis_state.last_error {
        ui.add_space(4.0);
        ui.label(egui::RichText::new(err).small().color(theme::STATUS_OFFLINE));
    }

    ui.add_space(6.0);
    ui.separator();
    ui.add_space(4.0);
    render_results(ui, gis_state, node_mgr, camera_focus);
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
) {
    gis_state.results.clear();
    gis_state.last_error = None;
    match gis_state.mode {
        GisQueryMode::Annotated => {
            ui_mgr.push_action(UiAction::GisQueryAnnotated { petal_id: petal_id.to_string() });
        }
        GisQueryMode::PropertyFilter => {
            let key = gis_state.filter_key_buf.trim().to_string();
            if key.is_empty() {
                gis_state.last_error = Some("Enter a property key".to_string());
                return;
            }
            let value = gis::parse_filter_value(&gis_state.filter_value_type_buf, gis_state.filter_value_buf.trim());
            ui_mgr.push_action(UiAction::GisQueryPropertyFilter {
                petal_id: petal_id.to_string(),
                key,
                value,
            });
        }
        GisQueryMode::Bbox => {
            let (Some(min), Some(max)) =
                (gis::parse_bbox_fields(&gis_state.bbox_min), gis::parse_bbox_fields(&gis_state.bbox_max))
            else {
                gis_state.last_error = Some("Bbox fields must be numbers".to_string());
                return;
            };
            let Some(petal) = verse_mgr.find_petal(petal_id) else { return };
            gis_state.results = petal
                .nodes
                .iter()
                .filter(|n| gis::bbox_contains(n.position, min, max))
                .map(|n| crate::gis::GisResultRow {
                    node_id: n.id.clone(),
                    name: n.name.clone(),
                    position: n.position,
                    annotation_title: None,
                })
                .collect();
        }
    }
}

fn render_results(
    ui: &mut egui::Ui,
    gis_state: &GisPanelState,
    node_mgr: &mut NodeManager,
    camera_focus: &mut CameraFocusTarget,
) {
    ui.label(
        egui::RichText::new(format!("Results ({})", gis_state.results.len()))
            .strong()
            .color(theme::TEXT_SECTION),
    );
    ui.add_space(4.0);

    if gis_state.results.is_empty() {
        ui.label(egui::RichText::new("No results yet.").small().color(theme::TEXT_MUTED).italics());
        return;
    }

    let mut clicked_row: Option<(String, [f32; 3])> = None;
    egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
        for row in &gis_state.results {
            egui::Frame::NONE
                .fill(theme::BG_PEER_ROW_EVEN)
                .inner_margin(egui::Margin::symmetric(6, 4))
                .corner_radius(2.0)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let label = egui::RichText::new(&row.name).color(theme::TEXT_BRIGHT);
                        if ui.add(egui::Label::new(label).sense(egui::Sense::click())).clicked() {
                            clicked_row = Some((row.node_id.clone(), row.position));
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(
                                egui::RichText::new(format!(
                                    "({:.1}, {:.1}, {:.1})",
                                    row.position[0], row.position[1], row.position[2]
                                ))
                                .small()
                                .color(theme::TEXT_DIM),
                            );
                        });
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
        node_mgr.pending_sidebar_select = Some(node_id);
        camera_focus.target = Some(pos);
    }
}
