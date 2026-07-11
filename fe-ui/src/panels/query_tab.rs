//! Inspector "Query" tab: ad-hoc SurrealQL SELECT/RETURN queries scoped to
//! the current navigation context.

use bevy_egui::egui;

use crate::actions::{UiAction, UiManager};
use crate::navigation_manager::NavigationManager;
use crate::plugin::InspectorFormState;
use crate::theme;

/// Inspector Query tab: a text area for submitting SurrealQL SELECT queries.
/// Results are displayed below as formatted JSON. The query is scoped to the
/// current navigation context (the petal/fractal/verse the user has navigated to).
pub(crate) fn inspector_query_section(
    ui: &mut egui::Ui,
    inspector: &mut InspectorFormState,
    nav: &NavigationManager,
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

            // Show current scope context
            let scope = super::inspector::build_nav_scope(nav);
            if !scope.is_empty() {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Scope:").small().color(theme::TEXT_DIM));
                    ui.label(
                        egui::RichText::new(&scope)
                            .small()
                            .monospace()
                            .color(theme::TEXT_SECTION),
                    );
                });
                ui.add_space(4.0);
            }

            ui.label(egui::RichText::new("Only SELECT and RETURN statements are allowed.").small().color(theme::TEXT_MUTED));
            ui.add_space(4.0);

            // SQL text area
            let response = ui.add(
                egui::TextEdit::multiline(&mut inspector.query_sql_buf)
                    .font(egui::TextStyle::Monospace)
                    .desired_rows(6)
                    .desired_width(ui.available_width())
                    .hint_text("SELECT * FROM node LIMIT 10"),
            );

            ui.add_space(6.0);

            // Submit button
            let can_submit = !inspector.query_sql_buf.trim().is_empty() && !inspector.query_loading;
            ui.horizontal(|ui| {
                let btn = egui::Button::new(if inspector.query_loading { "Running..." } else { "Run Query" })
                    .fill(if can_submit { theme::BG_SAVE } else { theme::BG_BUTTON });
                let submit_clicked = ui.add_enabled(can_submit, btn).clicked();
                let ctrl_enter = response.has_focus()
                    && ui.input(|i| i.key_pressed(egui::Key::Enter) && i.modifiers.ctrl);

                if (submit_clicked || ctrl_enter) && can_submit {
                    inspector.query_loading = true;
                    inspector.query_result = Some("Submitting...".to_string());
                    ui_mgr.push_action(UiAction::SubmitQuery {
                        sql: inspector.query_sql_buf.clone(),
                        scope,
                    });
                }

                if inspector.query_result.is_some() {
                    if ui.add(egui::Button::new("Clear").fill(theme::BG_BUTTON)).clicked() {
                        inspector.query_result = None;
                    }
                }
            });

            ui.add_space(2.0);
            ui.label(egui::RichText::new("Ctrl+Enter to submit").small().color(theme::TEXT_DIM));

            // Results display
            if let Some(ref result) = inspector.query_result {
                ui.add_space(8.0);
                ui.separator();
                ui.add_space(4.0);
                ui.label(egui::RichText::new("Result").small().strong().color(theme::TEXT_SECTION));
                ui.add_space(2.0);

                egui::ScrollArea::vertical()
                    .max_height(300.0)
                    .show(ui, |ui| {
                        ui.add(
                            egui::TextEdit::multiline(&mut result.as_str())
                                .font(egui::TextStyle::Monospace)
                                .desired_width(ui.available_width())
                                .interactive(false),
                        );
                    });
            }
        });
}
