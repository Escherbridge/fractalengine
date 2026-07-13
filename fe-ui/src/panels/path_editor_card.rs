//! Paths tab: list the active petal's track nodes, create/select/delete
//! tracks, and edit the selected track's point list (append from cursor,
//! remove, annotate, export). fe-ui queues intent only — see
//! `crate::path_ops` for the op-queue/status contract and
//! `fe-ui/src/AGENTS.md` §path-editor for the end-to-end design.

use bevy_egui::egui;

use crate::actions::{UiAction, UiManager};
use crate::gis::PathEditorState;
use crate::path_ops::PathEditStatus;
use crate::plugin::ViewportCursorWorld;
use crate::theme;

pub(crate) fn path_editor_section(
    ui: &mut egui::Ui,
    path_state: &mut PathEditorState,
    path_status: &PathEditStatus,
    ui_mgr: &mut UiManager,
    cursor_world: &ViewportCursorWorld,
    petal_id: &str,
) {
    if let Some(track_id) = path_state.editing_track_id.clone() {
        render_edit_view(ui, path_state, path_status, ui_mgr, cursor_world, &track_id, petal_id);
    } else {
        render_track_list(ui, path_state, ui_mgr, petal_id);
    }
}

fn render_track_list(ui: &mut egui::Ui, path_state: &mut PathEditorState, ui_mgr: &mut UiManager, petal_id: &str) {
    ui.label(egui::RichText::new("Paths").strong().color(theme::TEXT_SECTION));
    ui.add_space(4.0);

    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut path_state.new_track_name_buf)
                .hint_text("New path name")
                .desired_width(180.0),
        );
        if ui.add(egui::Button::new("New Path").fill(theme::BG_SAVE)).clicked() {
            let name = std::mem::take(&mut path_state.new_track_name_buf);
            ui_mgr.push_action(UiAction::PathCreateTrack { petal_id: petal_id.to_string(), name });
        }
    });

    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(format!("Tracks ({})", path_state.tracks.len())).strong().color(theme::TEXT_SECTION));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            // FR-3: this button is now a manual override, not the only sync
            // path — `db_results::apply_db_results` auto re-runs the query
            // on NodeCreated/NodeDeleted/`gis.track.name` property changes.
            let label = if path_state.tracks_pending { "Refreshing..." } else { "Refresh" };
            if ui
                .add_enabled(!path_state.tracks_pending, egui::Button::new(label).fill(theme::BG_BUTTON))
                .clicked()
            {
                ui_mgr.push_action(UiAction::PathQueryTracks { petal_id: petal_id.to_string() });
            }
        });
    });
    ui.add_space(4.0);

    if let Some(err) = &path_state.last_error {
        ui.label(egui::RichText::new(err).small().color(theme::STATUS_OFFLINE));
        ui.add_space(4.0);
    }

    if path_state.tracks.is_empty() {
        ui.label(egui::RichText::new("No paths yet.").small().color(theme::TEXT_MUTED).italics());
        return;
    }

    let mut selected: Option<String> = None;
    let mut to_delete: Option<String> = None;
    egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
        for row in &path_state.tracks {
            egui::Frame::NONE
                .fill(theme::BG_PEER_ROW_EVEN)
                .inner_margin(egui::Margin::symmetric(6, 4))
                .corner_radius(2.0)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let label = egui::RichText::new(row.annotation_title.as_deref().unwrap_or(&row.name))
                            .color(theme::TEXT_BRIGHT);
                        if ui.add(egui::Label::new(label).sense(egui::Sense::click())).clicked() {
                            selected = Some(row.node_id.clone());
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.small_button("Delete").clicked() {
                                to_delete = Some(row.node_id.clone());
                            }
                        });
                    });
                });
            ui.add_space(1.0);
        }
    });

    if let Some(node_id) = selected {
        path_state.start_editing(node_id);
    }
    if let Some(node_id) = to_delete {
        ui_mgr.push_action(UiAction::PathDeleteTrack { track_node_id: node_id });
    }
}

fn render_edit_view(
    ui: &mut egui::Ui,
    path_state: &mut PathEditorState,
    path_status: &PathEditStatus,
    ui_mgr: &mut UiManager,
    cursor_world: &ViewportCursorWorld,
    track_id: &str,
    petal_id: &str,
) {
    ui.horizontal(|ui| {
        if ui.small_button("\u{2190} Back").clicked() {
            path_state.stop_editing();
            ui_mgr.push_action(UiAction::PathQueryTracks { petal_id: petal_id.to_string() });
            return;
        }
        ui.label(egui::RichText::new("Editing path").strong().color(theme::TEXT_SECTION));
    });
    if path_state.editing_track_id.is_none() {
        return;
    }
    ui.add_space(4.0);

    ui.horizontal(|ui| {
        let has_cursor = cursor_world.pos.is_some();
        if ui
            .add_enabled(has_cursor, egui::Button::new("Append from cursor").fill(theme::BG_BUTTON))
            .clicked()
        {
            if let Some(pos) = cursor_world.pos {
                ui_mgr.push_action(UiAction::PathAppendPoint { track_node_id: track_id.to_string(), position: pos });
            }
        }
        if ui.add(egui::Button::new("Export GPX").fill(theme::BG_SAVE)).clicked() {
            ui_mgr.push_action(UiAction::PathExportGpx { track_node_id: track_id.to_string() });
        }
    });

    if path_status.track_node_id.as_deref() == Some(track_id) {
        ui.add_space(4.0);
        if let Some(err) = &path_status.error {
            ui.label(egui::RichText::new(format!("\u{2717} {err}")).small().color(theme::STATUS_OFFLINE));
        } else if let Some(msg) = &path_status.message {
            ui.label(egui::RichText::new(format!("\u{2713} {msg}")).small().color(theme::STATUS_ONLINE));
        }
    }

    ui.add_space(6.0);
    ui.separator();
    ui.add_space(4.0);
    ui.label(egui::RichText::new(format!("Points ({})", path_state.points.len())).strong().color(theme::TEXT_SECTION));
    ui.add_space(4.0);

    if path_state.points.is_empty() {
        ui.label(egui::RichText::new("No points yet — append one from the 3D cursor.").small().color(theme::TEXT_MUTED).italics());
        return;
    }

    ui.label(
        egui::RichText::new("Click terrain to add \u{2022} drag markers to move \u{2022} Shift/Alt+click a marker to annotate")
            .small()
            .color(theme::TEXT_MUTED)
            .italics(),
    );
    ui.add_space(4.0);

    let mut to_remove: Option<usize> = None;
    let mut to_annotate: Option<usize> = None;
    egui::ScrollArea::vertical().max_height(240.0).show(ui, |ui| {
        for (i, point) in path_state.points.iter().enumerate() {
            egui::Frame::NONE
                .fill(theme::BG_PEER_ROW_EVEN)
                .inner_margin(egui::Margin::symmetric(6, 4))
                .corner_radius(2.0)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(format!(
                                "{i}: ({:.1}, {:.1}, {:.1})",
                                point.position[0], point.position[1], point.position[2]
                            ))
                            .color(theme::TEXT_BRIGHT),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.small_button("Remove").clicked() {
                                to_remove = Some(i);
                            }
                            if ui.small_button("Annotate").clicked() {
                                to_annotate = Some(i);
                            }
                        });
                    });
                });
            ui.add_space(1.0);
        }
    });

    if let Some(index) = to_remove {
        ui_mgr.push_action(UiAction::PathRemovePoint { track_node_id: track_id.to_string(), index });
        if path_state.annotating_index == Some(index) {
            path_state.close_annotate_form();
        }
    }
    if let Some(index) = to_annotate {
        path_state.open_annotate_form(index);
    }

    // Inline annotation form for the point set by a list "Annotate" click or a
    // Shift/Alt+click on the point's viewport marker. Reuses the
    // `gis.annotation.*` contract once the bridge creates the waypoint node.
    if let Some(index) = path_state.annotating_index {
        render_annotate_form(ui, path_state, ui_mgr, track_id, index);
    }
}

/// Inline title/body/color form for annotating point `index`. Emits a
/// `PathAnnotatePoint` on Save and closes the form on Save/Cancel.
fn render_annotate_form(
    ui: &mut egui::Ui,
    path_state: &mut PathEditorState,
    ui_mgr: &mut UiManager,
    track_id: &str,
    index: usize,
) {
    ui.add_space(6.0);
    egui::Frame::NONE
        .fill(theme::BG_PEER_ROW_EVEN)
        .inner_margin(egui::Margin::same(8))
        .corner_radius(3.0)
        .show(ui, |ui| {
            ui.label(
                egui::RichText::new(format!("Annotate point {index}"))
                    .strong()
                    .color(theme::TEXT_SECTION),
            );
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label("Title");
                ui.add(egui::TextEdit::singleline(&mut path_state.annotate_title_buf).desired_width(200.0));
            });
            ui.horizontal(|ui| {
                ui.label("Body ");
                ui.add(egui::TextEdit::singleline(&mut path_state.annotate_body_buf).desired_width(200.0));
            });
            ui.horizontal(|ui| {
                ui.label("Color");
                ui.add(
                    egui::TextEdit::singleline(&mut path_state.annotate_color_buf)
                        .hint_text("#00ff00")
                        .desired_width(120.0),
                );
            });
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui.add(egui::Button::new("Save").fill(theme::BG_SAVE)).clicked() {
                    ui_mgr.push_action(UiAction::PathAnnotatePoint {
                        track_node_id: track_id.to_string(),
                        index,
                        title: path_state.annotate_title_buf.clone(),
                        body: path_state.annotate_body_buf.clone(),
                        color: path_state.annotate_color_buf.clone(),
                    });
                    path_state.close_annotate_form();
                }
                if ui.add(egui::Button::new("Cancel").fill(theme::BG_BUTTON)).clicked() {
                    path_state.close_annotate_form();
                }
            });
        });
}
