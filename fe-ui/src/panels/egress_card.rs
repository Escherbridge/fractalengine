//! "Copy for BI" egress card: GIS panel Export tab. Pure string building
//! lives in `crate::gis::egress_strings`; this file is egui-only. See
//! `fe-ui/src/panels/AGENTS.md` §egress-card.

use bevy_egui::egui;

use crate::accessibility::ResponseExt;

use crate::actions::UiManager;
use crate::extensions::builtin::{snippet_rows, SnippetInputs};
use crate::gis::egress_strings::{self as es, EgressSource, ExportFormat};
use crate::gis::GisPanelState;
use crate::node_manager::NodeManager;
use crate::panels::widgets::{copy_row, meters_to_world};
use crate::theme;

pub(crate) fn egress_section(
    ui: &mut egui::Ui,
    gis_state: &mut GisPanelState,
    node_mgr: &NodeManager,
    ui_mgr: &mut UiManager,
    petal_id: &str,
    world_scale: f64,
    api_endpoint: Option<std::net::SocketAddr>,
) {
    let running_api = api_endpoint.map(|address| format!("http://{address}"));
    if gis_state.egress.base_url_buf == es::DEFAULT_API_BASE {
        if let Some(base) = &running_api {
            gis_state.egress.base_url_buf = base.clone();
        }
    }
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
        ui.add(crate::ui_shell::layout_policy::text_input(ui, &mut gis_state.egress.base_url_buf).desired_width(200.0))
            .named(ui, "API base URL");
        if ui
            .add_enabled(running_api.is_some(), egui::Button::new("Use running API"))
            .named(ui, "Use running API endpoint for export")
            .clicked()
        {
            if let Some(base) = &running_api {
                gis_state.egress.base_url_buf = base.clone();
            }
        }
    });
    ui.add_space(6.0);

    let Some(sql) = build_sql(gis_state, petal_id, world_scale) else {
        ui.label(
            egui::RichText::new(source_error(gis_state.egress.source))
                .small()
                .color(theme::STATUS_OFFLINE),
        );
        return;
    };

    let base = gis_state.egress.base_url_buf.clone();
    // The compiled-in connector descriptors (R33) always render first; bound package answers follow, each named
    // `binding:egress.snippet:<key>` and labelled with its package (BND-10c).
    let inputs = SnippetInputs { base: &base, petal_id, sql: &sql };
    if let (EgressSource::Readings, Some(as_of_ms)) =
        (gis_state.egress.source, gis_state.egress.readings_as_of_ms)
    {
        // Pinned snapshot rows replace the generic ones, whose URLs carry no as_of_ms.
        let parquet = es::readings_export_url(&base, petal_id, as_of_ms, ExportFormat::Parquet);
        let csv = es::readings_export_url(&base, petal_id, as_of_ms, ExportFormat::Csv);
        copy_row(ui, ui_mgr, "Readings Parquet URL", &parquet);
        copy_row(ui, ui_mgr, "Readings CSV URL", &csv);
        copy_row(ui, ui_mgr, "DuckDB readings", &es::duckdb_snippet(&parquet));
    } else {
        for row in snippet_rows(&inputs, &[]) {
            copy_row(ui, ui_mgr, &row.label, &row.text);
        }
    }
    for (name, row) in crate::extensions::bound_snippet_rows(ui.ctx(), &inputs) {
        ui.scope(|ui| copy_row(ui, ui_mgr, &row.label, &row.text)).response.named(ui, name);
    }

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
            (EgressSource::Readings, "Readings"),
        ] {
            let active = gis_state.egress.source == source;
            let btn = egui::Button::new(label).fill(theme::tab_fill(active));
            if ui.add(btn).observed(ui).clicked() {
                gis_state.egress.source = source;
                if source == EgressSource::Readings && gis_state.egress.readings_as_of_ms.is_none() {
                    gis_state.egress.readings_as_of_ms = Some(crate::gis::readings::now_unix_ms());
                }
            }
        }
    });
    match gis_state.egress.source {
        EgressSource::Petal => {}
        EgressSource::Readings => {
            ui.horizontal(|ui| {
                if let Some(as_of) = gis_state.egress.readings_as_of_ms {
                    ui.label(
                        egui::RichText::new(snapshot_label(as_of))
                            .small()
                            .color(theme::TEXT_MUTED),
                    );
                }
                if crate::ui_shell::layout_policy::small_button(ui, "Refresh snapshot")
                    .observed(ui)
                    .clicked()
                {
                    gis_state.egress.readings_as_of_ms = Some(crate::gis::readings::now_unix_ms());
                }
            });
        }
        // The node id is panel-local state, filled by an EXPLICIT button click
        // — never read implicitly from `NodeManager.selected` per project
        // memory: track-selection-two-concepts.
        EgressSource::Node => {
            ui.horizontal(|ui| {
                ui.add(
                    crate::ui_shell::layout_policy::text_input(ui, &mut gis_state.egress.node_id_buf)
                        .hint_text("node id")
                        .desired_width(160.0),
                )
                .named(ui, "Export node ID");
                if crate::ui_shell::layout_policy::small_button(ui, "Use viewport selection")
                    .observed(ui)
                    .clicked()
                {
                    if let Some(sel) = node_mgr.primary() {
                        gis_state.egress.node_id_buf = sel.node_id.clone();
                    }
                }
            });
        }
        // Bbox reuses the Query tab's bbox buffers (Center on Selection /
        // Origin shortcuts live there).
        EgressSource::Bbox => {
            ui.label(
                egui::RichText::new(
                    "Uses the Query tab's bbox fields (m; the SQL is in world units).",
                )
                .small()
                .color(theme::TEXT_MUTED),
            );
        }
    }
}

/// Builds the egress SQL for the active source; `None` when its inputs are
/// missing/unparseable (caller renders `source_error`). Bbox fields are
/// meters; stored coordinates (and thus the SQL) are world units.
fn build_sql(gis_state: &GisPanelState, petal_id: &str, world_scale: f64) -> Option<String> {
    match gis_state.egress.source {
        EgressSource::Petal => Some(es::petal_scope_sql(petal_id)),
        EgressSource::Node => {
            let node_id = gis_state.egress.node_id_buf.trim();
            (!node_id.is_empty()).then(|| es::node_selection_sql(petal_id, node_id))
        }
        EgressSource::Readings => gis_state
            .egress
            .readings_as_of_ms
            .map(|as_of_ms| es::readings_sql(petal_id, as_of_ms)),
        EgressSource::Bbox => {
            let min = crate::gis::parse_bbox_fields(&gis_state.bbox_min)?
                .map(|m| meters_to_world(m, world_scale));
            let max = crate::gis::parse_bbox_fields(&gis_state.bbox_max)?
                .map(|m| meters_to_world(m, world_scale));
            Some(es::bbox_sql(petal_id, min, max))
        }
    }
}

/// "Snapshot as of 2026-11-13 22:44:59 UTC"; raw epoch ms only if out of chrono's range.
fn snapshot_label(as_of_ms: i64) -> String {
    match chrono::DateTime::from_timestamp_millis(as_of_ms) {
        Some(time) => format!("Snapshot as of {}", time.format("%Y-%m-%d %H:%M:%S UTC")),
        None => format!("Snapshot as of {as_of_ms} ms"),
    }
}

fn source_error(source: EgressSource) -> &'static str {
    match source {
        EgressSource::Readings => "Choose Refresh snapshot to pin a readings snapshot.",
        EgressSource::Node => "Enter a node id (or click Use viewport selection).",
        EgressSource::Bbox => "Bbox fields must be numbers (see the Query tab).",
        EgressSource::Petal => "No petal context.",
    }
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
        ui.add(crate::ui_shell::layout_policy::text_input(ui, &mut gis_state.egress.ttl_buf).desired_width(70.0))
            .named(ui, "Share link lifetime in seconds");
        ui.label(
            egui::RichText::new("default 3600, max 86400")
                .small()
                .color(theme::TEXT_MUTED),
        );
    });
    match es::parse_share_ttl(&gis_state.egress.ttl_buf) {
        Ok(ttl) => {
            let curl = es::mint_share_curl(base, sql, ExportFormat::Parquet, ttl);
            copy_row(ui, ui_mgr, "Mint via curl", &curl);
        }
        Err(error) => {
            ui.colored_label(theme::STATUS_OFFLINE, error);
        }
    }
    ui.label(
        egui::RichText::new("Run with your API token; the response contains the signed URL.")
            .small()
            .color(theme::TEXT_MUTED),
    );
    ui.label(
        egui::RichText::new("Parquet/CSV links need a petal-scoped token.")
            .small()
            .color(theme::TEXT_MUTED),
    );
}
