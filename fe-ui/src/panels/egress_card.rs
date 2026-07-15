//! "Copy for BI" egress card: GIS panel Export tab. Pure string building
//! lives in `crate::gis::egress_strings`; this file is egui-only. See
//! `fe-ui/src/panels/AGENTS.md` §egress-card.

use bevy_egui::egui;

use crate::actions::UiManager;
use crate::gis::egress_strings::{self as es, EgressSource, ExportFormat};
use crate::gis::GisPanelState;
use crate::node_manager::NodeManager;
use crate::theme;

pub(crate) fn egress_section(
    ui: &mut egui::Ui,
    gis_state: &mut GisPanelState,
    node_mgr: &NodeManager,
    ui_mgr: &mut UiManager,
    petal_id: &str,
) {
    ui.label(
        egui::RichText::new("Copy for BI")
            .strong()
            .color(theme::TEXT_SECTION),
    );
    ui.label(
        egui::RichText::new("Paste these into PowerBI, DuckDB, or a spreadsheet.")
            .small()
            .color(theme::TEXT_MUTED),
    );
    ui.add_space(6.0);

    render_source_selector(ui, gis_state, node_mgr);
    ui.add_space(4.0);

    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new("API base")
                .small()
                .color(theme::TEXT_DIM),
        );
        ui.add(egui::TextEdit::singleline(&mut gis_state.egress.base_url_buf).desired_width(200.0));
    });
    ui.add_space(6.0);

    let Some(sql) = build_sql(gis_state, petal_id) else {
        ui.label(
            egui::RichText::new(source_error(gis_state.egress.source))
                .small()
                .color(theme::STATUS_OFFLINE),
        );
        return;
    };

    let base = gis_state.egress.base_url_buf.clone();
    let parquet_url = es::export_url(&base, petal_id, &sql, ExportFormat::Parquet);
    let csv_url = es::export_url(&base, petal_id, &sql, ExportFormat::Csv);

    copy_row(ui, ui_mgr, "SQL (POST /api/v1/query)", &sql);
    copy_row(ui, ui_mgr, "Query endpoint", &es::query_post_url(&base));
    copy_row(ui, ui_mgr, "Export URL (parquet)", &parquet_url);
    copy_row(ui, ui_mgr, "Export URL (csv)", &csv_url);
    copy_row(
        ui,
        ui_mgr,
        "DuckDB snippet",
        &es::duckdb_snippet(&parquet_url),
    );

    ui.add_space(6.0);
    ui.separator();
    ui.add_space(4.0);
    render_share_section(ui, gis_state, ui_mgr, &base, &sql);
}

fn render_source_selector(
    ui: &mut egui::Ui,
    gis_state: &mut GisPanelState,
    node_mgr: &NodeManager,
) {
    ui.horizontal(|ui| {
        for (source, label) in [
            (EgressSource::Petal, "Petal"),
            (EgressSource::Node, "Node"),
            (EgressSource::Bbox, "Bbox"),
        ] {
            let active = gis_state.egress.source == source;
            let btn = egui::Button::new(label).fill(if active {
                theme::BG_BUTTON_ACTIVE
            } else {
                theme::BG_BUTTON
            });
            if ui.add(btn).clicked() {
                gis_state.egress.source = source;
            }
        }
    });
    match gis_state.egress.source {
        EgressSource::Petal => {}
        // The node id is panel-local state, filled by an EXPLICIT button click
        // — never read implicitly from `NodeManager.selected` per project
        // memory: track-selection-two-concepts.
        EgressSource::Node => {
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut gis_state.egress.node_id_buf)
                        .hint_text("node id")
                        .desired_width(160.0),
                );
                if ui.small_button("Use viewport selection").clicked() {
                    if let Some(sel) = node_mgr.selected.as_ref() {
                        gis_state.egress.node_id_buf = sel.node_id.clone();
                    }
                }
            });
        }
        // Bbox reuses the Query tab's bbox buffers (Center on Selection /
        // Origin shortcuts live there).
        EgressSource::Bbox => {
            ui.label(
                egui::RichText::new("Uses the Query tab's bbox fields.")
                    .small()
                    .color(theme::TEXT_MUTED),
            );
        }
    }
}

/// Builds the egress SQL for the active source; `None` when its inputs are
/// missing/unparseable (caller renders `source_error`).
fn build_sql(gis_state: &GisPanelState, petal_id: &str) -> Option<String> {
    match gis_state.egress.source {
        EgressSource::Petal => Some(es::petal_scope_sql(petal_id)),
        EgressSource::Node => {
            let node_id = gis_state.egress.node_id_buf.trim();
            (!node_id.is_empty()).then(|| es::node_selection_sql(petal_id, node_id))
        }
        EgressSource::Bbox => {
            let min = crate::gis::parse_bbox_fields(&gis_state.bbox_min)?;
            let max = crate::gis::parse_bbox_fields(&gis_state.bbox_max)?;
            Some(es::bbox_sql(petal_id, min, max))
        }
    }
}

fn source_error(source: EgressSource) -> &'static str {
    match source {
        EgressSource::Node => "Enter a node id (or click Use viewport selection).",
        EgressSource::Bbox => "Bbox fields must be numbers (see the Query tab).",
        EgressSource::Petal => "No petal context.",
    }
}

/// Labeled monospace value with a one-click copy button (egui builtin
/// clipboard via `ctx.copy_text` — no new deps).
fn copy_row(ui: &mut egui::Ui, ui_mgr: &mut UiManager, label: &str, value: &str) {
    egui::Frame::NONE
        .fill(theme::BG_BUTTON)
        .inner_margin(egui::Margin::symmetric(6, 4))
        .corner_radius(3.0)
        .show(ui, |ui| {
            ui.set_max_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(label)
                        .small()
                        .strong()
                        .color(theme::TEXT_SECTION),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add(
                            egui::Button::new("\u{1F4CB}")
                                .fill(egui::Color32::TRANSPARENT)
                                .small(),
                        )
                        .on_hover_text("Copy to clipboard")
                        .clicked()
                    {
                        ui.ctx().copy_text(value.to_string());
                        let now = ui.ctx().input(|i| i.time);
                        ui_mgr.show_toast(format!("Copied {label}"), now);
                    }
                });
            });
            ui.add(
                egui::Label::new(
                    egui::RichText::new(value)
                        .small()
                        .monospace()
                        .color(theme::TEXT_BRIGHT),
                )
                .wrap(),
            );
        });
    ui.add_space(3.0);
}

/// Shareable-link section: fe-ui has no HTTP-client seam to fe-api yet, so
/// minting shows the ready-to-run curl for `POST /api/v1/query/share`
/// (Phase 3 contract). See track open_decisions.
fn render_share_section(
    ui: &mut egui::Ui,
    gis_state: &mut GisPanelState,
    ui_mgr: &mut UiManager,
    base: &str,
    sql: &str,
) {
    ui.label(
        egui::RichText::new("Shareable link")
            .strong()
            .color(theme::TEXT_SECTION),
    );
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new("TTL (s)")
                .small()
                .color(theme::TEXT_DIM),
        );
        ui.add(egui::TextEdit::singleline(&mut gis_state.egress.ttl_buf).desired_width(70.0));
        ui.label(
            egui::RichText::new("default 3600, max 86400")
                .small()
                .color(theme::TEXT_MUTED),
        );
    });
    let ttl = gis_state
        .egress
        .ttl_buf
        .trim()
        .parse::<u64>()
        .unwrap_or(es::DEFAULT_SHARE_TTL_SECS)
        .min(86_400);
    let curl = es::mint_share_curl(base, sql, ExportFormat::Parquet, ttl);
    copy_row(ui, ui_mgr, "Mint via curl", &curl);
    ui.label(
        egui::RichText::new("Run with your API token; the response contains the signed URL.")
            .small()
            .color(theme::TEXT_MUTED),
    );
}
