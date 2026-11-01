//! Query view/run model and virtualized dock; see AGENTS.md §query-results.

use crate::{
    actions::UiManager, navigation_manager::NavigationManager, node_manager::NodeManager,
    plugin::CameraFocusTarget, verse_manager::VerseManager,
};
use bevy_egui::egui;
use serde_json::Value;
use std::{
    collections::{BTreeSet, HashSet},
    time::Instant,
};

#[derive(Default)]
pub struct ResultsDockState {
    pub open: bool,
    pub request_id: Option<String>,
    pub rows: Vec<Value>,
    pub error: Option<String>,
    pub submitted_scope: String,
    pub elapsed_ms: Option<u128>,
    columns: Vec<String>,
    cells: Vec<Vec<String>>,
    node_ids: HashSet<String>,
    order: Vec<usize>,
    widths: Vec<f32>,
    sort: Option<(usize, bool)>,
    selected_row: Option<usize>,
    started: Option<Instant>,
}

impl ResultsDockState {
    pub fn pending(&self) -> bool {
        self.request_id.is_some()
    }

    pub fn summary(&self) -> Value {
        serde_json::json!({"open":self.open,"pending":self.pending(),"row_count":self.rows.len(),
            "elapsed_ms":self.elapsed_ms,"failed":self.error.is_some(),"selected_row":self.selected_row,
            "sort":self.sort.map(|(column,ascending)| serde_json::json!({"column":self.columns[column],"ascending":ascending}))})
    }

    pub(crate) fn display_order(&self) -> &[usize] {
        &self.order
    }

    pub fn begin(&mut self, id: String, scope: String) {
        *self = Self {
            open: true,
            request_id: Some(id),
            submitted_scope: scope,
            started: Some(Instant::now()),
            ..Default::default()
        };
    }

    pub fn complete(&mut self, rows: &[Value]) {
        self.finish();
        self.error = None;
        self.rows = rows.to_vec();
        self.node_ids = rows
            .iter()
            .filter_map(|row| {
                row.get("node_id")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .collect();
        let mut columns = BTreeSet::new();
        for row in rows {
            if let Some(object) = row.as_object() {
                columns.extend(object.keys().cloned());
            } else {
                columns.insert("value".to_owned());
            }
        }
        self.columns = columns.into_iter().collect();
        self.cells = rows
            .iter()
            .map(|row| {
                self.columns
                    .iter()
                    .map(|column| {
                        let value = row
                            .as_object()
                            .and_then(|o| o.get(column))
                            .unwrap_or(if column == "value" { row } else { &Value::Null });
                        value
                            .as_str()
                            .map(str::to_owned)
                            .unwrap_or_else(|| value.to_string())
                    })
                    .collect()
            })
            .collect();
        self.order = (0..rows.len()).collect();
        self.widths = vec![140.0; self.columns.len()];
    }

    pub fn fail(&mut self, error: &str) {
        self.finish();
        self.error = Some(error.to_owned());
    }

    fn finish(&mut self) {
        self.request_id = None;
        self.elapsed_ms = self.started.take().map(|start| start.elapsed().as_millis());
    }

    pub fn invalidate(&mut self, active_verse: Option<&str>) {
        let submitted_verse = self
            .submitted_scope
            .strip_prefix("VERSE#")
            .map(|s| s.split("-FRACTAL#").next().unwrap_or(s));
        if submitted_verse.is_some() && submitted_verse != active_verse {
            *self = Self::default();
        }
    }

    pub(crate) fn sort_by(&mut self, column: usize) {
        let ascending = self
            .sort
            .map(|(c, ascending)| c != column || !ascending)
            .unwrap_or(true);
        let key = &self.columns[column];
        self.order.sort_by(|&a, &b| {
            let numbers = self.rows[a]
                .get(key)
                .and_then(Value::as_f64)
                .zip(self.rows[b].get(key).and_then(Value::as_f64));
            let order = numbers
                .and_then(|(a, b)| a.partial_cmp(&b))
                .unwrap_or_else(|| self.cells[a][column].cmp(&self.cells[b][column]));
            let order = if ascending { order } else { order.reverse() };
            order.then(a.cmp(&b))
        });
        self.sort = Some((column, ascending));
    }

    pub fn contains_node(&self, id: &str) -> bool {
        self.open && self.node_ids.contains(id)
    }
}

fn row_node<'a>(
    row: &Value,
    nav: &NavigationManager,
    hierarchy: &'a VerseManager,
) -> Option<&'a crate::verse_manager::NodeEntry> {
    let id = row.get("node_id")?.as_str()?;
    hierarchy
        .verses
        .iter()
        .filter(|v| Some(v.id.as_str()) == nav.active_verse_id.as_deref())
        .flat_map(|v| &v.fractals)
        .filter(|f| Some(f.id.as_str()) == nav.active_fractal_id.as_deref())
        .flat_map(|f| &f.petals)
        .filter(|p| Some(p.id.as_str()) == nav.active_petal_id.as_deref())
        .flat_map(|p| &p.nodes)
        .find(|n| n.id == id)
}

pub fn render(
    ctx: &egui::Context,
    results: &mut ResultsDockState,
    nav: &NavigationManager,
    hierarchy: &VerseManager,
    nodes: &mut NodeManager,
    focus: &mut CameraFocusTarget,
    ui_mgr: &mut UiManager,
) {
    results.invalidate(nav.active_verse_id.as_deref());
    if !results.open {
        return;
    }
    let height = ctx.available_rect().height();
    let max_height = (height * 0.35).min((height - 220.0).max(72.0)).max(72.0);
    egui::TopBottomPanel::bottom("query_results_dock")
        .resizable(true)
        .default_height(180.0)
        .height_range(72.0..=max_height)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.strong("Results");
                ui.label(format!(
                    "{} rows · {} ms",
                    results.rows.len(),
                    results.elapsed_ms.unwrap_or(0)
                ));
                if results.pending() {
                    ui.spinner();
                }
                if let Some(row) = results.selected_row {
                    if ui.button("Copy row").clicked() {
                        ctx.copy_text(results.cells[row].join("\t"));
                        ui_mgr.show_toast("Copied result row", ctx.input(|i| i.time));
                    }
                }
                if ui.button("Close Results").clicked() {
                    results.open = false;
                }
            });
            if let Some(error) = &results.error {
                ui.colored_label(crate::theme::STATUS_OFFLINE, error);
                return;
            }
            if results.rows.is_empty() {
                ui.label(if results.pending() {
                    "Running query…"
                } else {
                    "No rows matched this query."
                });
                return;
            }
            let row_height = 24.0;
            let total_width =
                results.widths.iter().sum::<f32>() + results.widths.len() as f32 * 8.0;
            egui::ScrollArea::both()
                .id_salt("query_result_table")
                .auto_shrink([false, false])
                .show_rows(ui, row_height, results.display_order().len() + 1, |ui, range| {
                    ui.set_min_width(total_width);
                    for display_row in range {
                        if display_row == 0 {
                            ui.horizontal(|ui| {
                                for col in 0..results.columns.len() {
                                    let label = match results.sort {
                                        Some((c, true)) if c == col => {
                                            format!("{} ↑", results.columns[col])
                                        }
                                        Some((c, false)) if c == col => {
                                            format!("{} ↓", results.columns[col])
                                        }
                                        _ => results.columns[col].clone(),
                                    };
                                    let response = ui.add_sized(
                                        [results.widths[col] - 6.0, row_height],
                                        egui::Button::new(label).truncate(),
                                    );
                                    super::layout_policy::record_response(ui, &response);
                                    if response.clicked() {
                                        results.sort_by(col);
                                    }
                                    let (rect, response) = ui.allocate_exact_size(
                                        egui::vec2(6.0, row_height),
                                        egui::Sense::drag(),
                                    );
                                    ui.painter().vline(
                                        rect.center().x,
                                        rect.y_range(),
                                        egui::Stroke::new(1.0, crate::theme::TEXT_DIM),
                                    );
                                    if response.dragged() {
                                        results.widths[col] = (results.widths[col]
                                            + response.drag_delta().x)
                                            .clamp(60.0, 600.0);
                                    }
                                }
                            });
                            continue;
                        }
                        let row = results.order[display_row - 1];
                        ui.push_id(row, |ui| {
                            ui.horizontal(|ui| {
                                let node = row_node(&results.rows[row], nav, hierarchy);
                                for col in 0..results.columns.len() {
                                    let text = &results.cells[row][col];
                                    let response = ui
                                        .add_sized(
                                            [results.widths[col], row_height],
                                            egui::Button::selectable(
                                                results.selected_row == Some(row),
                                                text,
                                            )
                                            .truncate(),
                                        )
                                        .on_hover_text(text);
                                    super::layout_policy::record_response(ui, &response);
                                    if response.clicked() {
                                        results.selected_row = Some(row);
                                        if let Some(node) = node {
                                            nodes.request_select_and_frame(
                                                &node.id,
                                                node.position,
                                                focus,
                                            );
                                        }
                                    }
                                    response.context_menu(|ui| {
                                        if ui.button("Copy cell").clicked() {
                                            ctx.copy_text(text.clone());
                                            ui.close();
                                        }
                                        if ui.button("Copy row").clicked() {
                                            ctx.copy_text(results.cells[row].join("\t"));
                                            ui.close();
                                        }
                                    });
                                }
                            })
                        });
                    }
                });
        });
}

pub(crate) fn draw_matches(
    inspector: bevy::prelude::Res<crate::plugin::InspectorFormState>,
    nav: bevy::prelude::Res<NavigationManager>,
    transforms: bevy::prelude::Query<(
        &bevy::prelude::GlobalTransform,
        &crate::plugin::SpawnedNodeMarker,
        &bevy::prelude::ViewVisibility,
    )>,
    mut gizmos: bevy::prelude::Gizmos,
) {
    let results = &inspector.query_results;
    if !results.open
        || results
            .submitted_scope
            .strip_prefix("VERSE#")
            .map(|s| s.split("-FRACTAL#").next().unwrap_or(s))
            != nav.active_verse_id.as_deref()
    {
        return;
    }
    for (transform, marker, visibility) in &transforms {
        if visibility.get()
            && Some(marker.petal_id.as_str()) == nav.active_petal_id.as_deref()
            && results.contains_node(&marker.node_id)
        {
            gizmos.sphere(
                transform.translation(),
                0.25,
                bevy::prelude::Color::srgb(0.2, 0.8, 1.0),
            );
        }
    }
}
