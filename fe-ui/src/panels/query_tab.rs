//! Inspector "Query" tab: ad-hoc SurrealQL SELECT queries. Offers one-click
//! templates built from the real schema (F3) and lays the editor/results out
//! top-down so Results fills whatever height remains (F2). See
//! `fe-ui/src/AGENTS.md` §inspector.

use bevy_egui::egui;

use crate::accessibility::ResponseExt;

use crate::actions::{UiAction, UiManager};
use crate::navigation_manager::NavigationManager;
use crate::node_manager::NodeManager;
use crate::panels::widgets::clipped_value;
use crate::plugin::InspectorFormState;
use crate::theme;
use crate::verse_manager::VerseManager;

/// Reasonable minimum rows for the SQL editor regardless of panel height.
const QUERY_EDITOR_MIN_ROWS: usize = 4;
/// Floor for the Result scroll area so a short panel still shows something
/// rather than collapsing to near-zero (F2).

/// Escapes a value for literal interpolation into a template SQL string.
/// `actions::query::submit` always sends an empty `vars` map (the GUI tab has
/// no bind-variable plumbing — fe-ui/src/actions/query.rs), so templates
/// cannot use `$id`-style binds; this is the cheap defence against a quote in
/// an id/DID making it onto the wire. See `fe-ui/src/AGENTS.md` §inspector.
fn sql_literal(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\'', "\\'")
}

/// One-click SELECT templates (F3), built from the real tables/keys in
/// `fe-database/src/schema.rs`: `node` is keyed by `node_id` and carries a
/// `petal_id` column (no scope field), `iot_reading` is keyed by `node_id`.
/// Always offers a petal-scoped "all nodes" template; adds node-specific
/// templates only when something is selected.
fn query_templates(nav: &NavigationManager, node_mgr: &NodeManager) -> Vec<(&'static str, String)> {
    let mut templates = Vec::new();
    if nav.active_petal_id.is_none() {
        return templates;
    }
    if let Some(sel) = node_mgr.selected.as_ref() {
        let id = sql_literal(&sel.node_id);
        templates.push((
            "This node",
            format!("SELECT * FROM node WHERE node_id = '{id}'"),
        ));
        templates.push((
            "This node's device data",
            format!("SELECT * FROM iot_reading WHERE node_id = '{id}'"),
        ));
    }
    match nav.active_petal_id.as_deref() {
        Some(petal_id) => templates.push((
            "All nodes in this petal",
            format!(
                "SELECT * FROM node WHERE petal_id = '{}'",
                sql_literal(petal_id)
            ),
        )),
        None => {}
    }
    templates
}

/// Inspector Query tab: templates + a text area for submitting SurrealQL
/// SELECT queries, with results below.
pub(crate) fn inspector_query_section(
    ui: &mut egui::Ui,
    inspector: &mut InspectorFormState,
    nav: &NavigationManager,
    node_mgr: &NodeManager,
    hierarchy: &VerseManager,
    ui_mgr: &mut UiManager,
) {
    egui::Frame::NONE
        .inner_margin(egui::Margin::symmetric(10, 4))
        .show(ui, |ui| {
            ui.set_max_width(ui.available_width());

            ui.label(
                egui::RichText::new("SurrealQL Query")
                    .strong()
                    .color(theme::TEXT_SECTION),
            );
            ui.add_space(4.0);

            let scope = super::inspector::build_nav_scope(nav);
            let petal = nav
                .active_petal_id
                .as_deref()
                .and_then(|id| hierarchy.find_petal(id));
            let scoped = petal.is_some()
                && fe_database::parse_scope(&scope)
                    .ok()
                    .is_some_and(|scope| scope.petal_id.is_some());
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("Query petal:")
                        .small()
                        .color(theme::TEXT_DIM),
                );
                if let Some(petal) = petal {
                    clipped_value(ui, &petal.name, theme::TEXT_SECTION).on_hover_text(&petal.id);
                } else {
                    ui.label("Open a petal to query it");
                }
            });
            ui.add_space(4.0);

            // One-click templates (F3): fills the buffer, Run still required.
            ui.horizontal_wrapped(|ui| {
                for (label, sql) in query_templates(nav, node_mgr) {
                    if ui
                        .add(
                            egui::Button::new(egui::RichText::new(label).small())
                                .fill(theme::BG_BUTTON),
                        ).observed(ui)
                        .clicked()
                    {
                        inspector.query_sql_buf = sql;
                    }
                }
            });
            ui.add_space(4.0);

            ui.label(
                egui::RichText::new("Only a single SELECT statement is allowed.")
                    .small()
                    .color(theme::TEXT_MUTED),
            );
            ui.add_space(4.0);

            // SQL text area — reasonable min height; the hint lists the real
            // tables instead of a single wrong-table example (F3).
            let editor_height = crate::ui_shell::layout_policy::editor_height(ui.ctx());
            let response = crate::ui_shell::layout_policy::scroll(
                ui,
                "SQL editor",
                [false, true],
                egui::ScrollArea::vertical()
                    .id_salt("sql_editor")
                    .max_height(editor_height),
                |ui| {
                    ui.add_sized(
                        [ui.available_width(), editor_height],
                        egui::TextEdit::multiline(&mut inspector.query_sql_buf)
                            .font(egui::TextStyle::Monospace)
                            .desired_rows(QUERY_EDITOR_MIN_ROWS)
                            .desired_width(ui.available_width())
                            .hint_text(
                                "Petal tables: node, iot_reading, device_payload, petal, room, model, crate_registry\n\
                         e.g. SELECT * FROM node",
                            ),
                    )
                    .named(ui, "SQL query")
                },
            )
            .inner;

            ui.add_space(6.0);

            // Submit button
            let can_submit = scoped
                && !inspector.query_sql_buf.trim().is_empty()
                && !inspector.query_results.pending();
            ui.horizontal(|ui| {
                let btn = egui::Button::new(if inspector.query_results.pending() {
                    "Running..."
                } else {
                    "Run Query"
                })
                .fill(if can_submit {
                    theme::BG_SAVE
                } else {
                    theme::BG_BUTTON
                });
                let submit = ui.add_enabled(can_submit, btn).named(ui, "Run Query");
                let submit_clicked = submit.clicked();
                let ctrl_enter = response.has_focus()
                    && ui.input(|i| i.key_pressed(egui::Key::Enter) && i.modifiers.command);

                if (submit_clicked || ctrl_enter) && can_submit {
                    ui_mgr.push_action(UiAction::SubmitQuery {
                        sql: inspector.query_sql_buf.clone(),
                        scope: scope.clone(),
                    });
                }
            });

            ui.add_space(2.0);
            ui.label(
                egui::RichText::new("Ctrl+Enter to submit")
                    .small()
                    .color(theme::TEXT_DIM),
            );

            ui.label("Results open in the dock below the viewport.");
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sql_literal_escapes_quotes_and_backslashes() {
        assert_eq!(sql_literal("abc"), "abc");
        for (value, limit, expected) in [
            ("abc", 8, "abc"),
            (
                "\u{03b1}\u{03b2}\u{03b3}\u{03b4}\u{03b5}\u{03b6}\u{03b7}\u{03b8}",
                5,
                "\u{03b1}\u{2026}\u{03b6}\u{03b7}\u{03b8}",
            ),
            ("abc", 0, "\u{2026}"),
        ] {
            assert_eq!(
                crate::ui_shell::layout_policy::middle_ellipsis(value, limit),
                expected
            );
        }
        assert_eq!(sql_literal("o'brien"), "o\\'brien");
        assert_eq!(sql_literal("a\\b"), "a\\\\b");
    }

    #[test]
    fn templates_require_an_open_petal() {
        let nav = NavigationManager::default();
        let node_mgr = NodeManager::default();
        let templates = query_templates(&nav, &node_mgr);
        assert!(templates.is_empty());
    }

    #[test]
    fn templates_scope_to_active_petal_when_present() {
        let nav = NavigationManager {
            active_petal_id: Some("petal-123".to_string()),
            ..Default::default()
        };
        let node_mgr = NodeManager::default();
        let templates = query_templates(&nav, &node_mgr);
        assert_eq!(templates.len(), 1);
        assert_eq!(templates[0].0, "All nodes in this petal");
        assert_eq!(
            templates[0].1,
            "SELECT * FROM node WHERE petal_id = 'petal-123'"
        );
    }

    #[test]
    fn templates_add_node_scoped_queries_when_a_node_is_selected() {
        let nav = NavigationManager::default();
        let mut node_mgr = NodeManager::default();
        node_mgr.select(bevy::prelude::Entity::from_bits(1), "node-42");
        let templates = query_templates(&nav, &node_mgr);
        assert_eq!(templates.len(), 3);
        assert_eq!(templates[0].0, "This node");
        assert_eq!(
            templates[0].1,
            "SELECT * FROM node WHERE node_id = 'node-42'"
        );
        assert_eq!(templates[1].0, "This node's device data");
        assert_eq!(
            templates[1].1,
            "SELECT * FROM iot_reading WHERE node_id = 'node-42'"
        );
    }
}
