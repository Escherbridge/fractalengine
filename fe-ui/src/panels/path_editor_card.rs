//! Paths tab: list the active petal's track nodes, create/select/delete
//! tracks, and edit the selected track's point list (Pen-tool click-to-place,
//! remove, annotate, export). fe-ui queues intent only — see
//! `crate::path_ops` for the op-queue/status contract, `node_manager/AGENTS.md`
//! §pen-tool for the Pen tool, and `fe-ui/src/AGENTS.md` §path-editor for the
//! end-to-end design.

use bevy_egui::egui;

use crate::actions::{UiAction, UiManager};
use crate::gis::PathEditorState;
use crate::path_ops::PathEditStatus;
use crate::plugin::ViewportCursorWorld;
use crate::theme;

// FR-1 (data_icons_20260713): recolorable geometric glyphs for the Paths tab.
// Plain `\u{25xx}`/`\u{27xx}` codepoints that egui recolors reliably (color
// emoji do not) — see `sidebar.rs:306` precedent. `type_glyph` maps a
// `gpx_type` discriminant to its glyph for the 3D overlay (FR-3) too.
/// Route/path glyph for a track row (`\u{29BF}`, circled dot in a ring).
const GLYPH_TRACK: &str = "\u{29BF}";
/// Filled circle for a timestamped/plain path point (`\u{25CF}`).
const GLYPH_POINT: &str = "\u{25CF}";
/// Hollow circle for a point with no timestamp (`\u{25CB}`) — authored via the
/// Pen tool with no GPX time, vs. an imported trackpoint that carries one.
const GLYPH_POINT_UNTIMED: &str = "\u{25CB}";
/// Diamond for a waypoint (`\u{25C6}`) — matches the annotated-node marker feel.
const GLYPH_WAYPOINT: &str = "\u{25C6}";

/// Map a `gpx_type` property value to its type glyph. Pure so it's shared by
/// the panel rows (FR-1) and the 3D overlay labels (FR-3) and unit-testable
/// without egui. Unknown types fall back to the point glyph.
pub(crate) fn type_glyph(gpx_type: &str) -> &'static str {
    match gpx_type {
        "track" => GLYPH_TRACK,
        "waypoint" => GLYPH_WAYPOINT,
        "trackpoint" => GLYPH_POINT,
        _ => GLYPH_POINT,
    }
}

/// path_interaction_20260716 (FR-3): human-readable distance label — m below
/// 1 km, km above, cm-ish precision below 1 m. fe-ui-local twin of
/// `fe_terrain::ruler::format_distance_m` (fe-ui must not depend on fe-terrain).
pub(crate) fn format_distance_m(meters: f64) -> String {
    if !meters.is_finite() || meters <= 0.0 {
        return "0 m".to_string();
    }
    if meters >= 1000.0 {
        let km = meters / 1000.0;
        if km >= 10.0 || km.fract().abs() < 1e-9 {
            format!("{km:.0} km")
        } else {
            format!("{km:.1} km")
        }
    } else if meters >= 1.0 {
        if meters.fract().abs() < 1e-9 {
            format!("{meters:.0} m")
        } else {
            format!("{meters:.1} m")
        }
    } else {
        format!("{meters:.2} m")
    }
}

pub(crate) fn path_editor_section(
    ui: &mut egui::Ui,
    path_state: &mut PathEditorState,
    path_status: &PathEditStatus,
    ui_mgr: &mut UiManager,
    cursor_world: &ViewportCursorWorld,
    petal_id: &str,
) {
    if let Some(track_id) = path_state.editing_track_id.clone() {
        render_edit_view(
            ui,
            path_state,
            path_status,
            ui_mgr,
            cursor_world,
            &track_id,
            petal_id,
        );
    } else {
        render_track_list(ui, path_state, ui_mgr, petal_id);
    }
}

fn render_track_list(
    ui: &mut egui::Ui,
    path_state: &mut PathEditorState,
    ui_mgr: &mut UiManager,
    petal_id: &str,
) {
    ui.label(
        egui::RichText::new("Paths")
            .strong()
            .color(theme::TEXT_SECTION),
    );
    ui.add_space(4.0);

    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut path_state.new_track_name_buf)
                .hint_text("New path name")
                .desired_width(180.0),
        );
        if ui
            .add(egui::Button::new("New Path").fill(theme::BG_SAVE))
            .clicked()
        {
            let name = std::mem::take(&mut path_state.new_track_name_buf);
            // Manual create: correlation_id None — this isn't a pen auto-create,
            // so the bridge generates its own id and nothing flushes a pen point.
            ui_mgr.push_action(UiAction::PathCreateTrack {
                petal_id: petal_id.to_string(),
                name,
                correlation_id: None,
            });
        }
    });

    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(format!("Tracks ({})", path_state.tracks.len()))
                .strong()
                .color(theme::TEXT_SECTION),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            // FR-3: this button is now a manual override, not the only sync
            // path — `db_results::apply_db_results` auto re-runs the query
            // on NodeCreated/NodeDeleted/`gis.track.name` property changes.
            let label = if path_state.tracks_pending {
                "Refreshing..."
            } else {
                "Refresh"
            };
            if ui
                .add_enabled(
                    !path_state.tracks_pending,
                    egui::Button::new(label).fill(theme::BG_BUTTON),
                )
                .clicked()
            {
                ui_mgr.push_action(UiAction::PathQueryTracks {
                    petal_id: petal_id.to_string(),
                });
            }
        });
    });
    ui.add_space(4.0);

    if let Some(err) = &path_state.last_error {
        ui.label(
            egui::RichText::new(err)
                .small()
                .color(theme::STATUS_OFFLINE),
        );
        ui.add_space(4.0);
    }

    if path_state.tracks.is_empty() {
        ui.label(
            egui::RichText::new("No paths yet.")
                .small()
                .color(theme::TEXT_MUTED)
                .italics(),
        );
        return;
    }

    let mut selected: Option<String> = None;
    let mut to_delete: Option<String> = None;
    egui::ScrollArea::vertical()
        .max_height(220.0)
        .show(ui, |ui| {
            for row in &path_state.tracks {
                egui::Frame::NONE
                    .fill(theme::BG_PEER_ROW_EVEN)
                    .inner_margin(egui::Margin::symmetric(6, 4))
                    .corner_radius(2.0)
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            // FR-1: type glyph before the track name (recolored, small).
                            ui.label(
                                egui::RichText::new(GLYPH_TRACK)
                                    .small()
                                    .color(theme::ICON_TRACK),
                            );
                            let label = egui::RichText::new(
                                row.annotation_title.as_deref().unwrap_or(&row.name),
                            )
                            .color(theme::TEXT_BRIGHT);
                            if ui
                                .add(egui::Label::new(label).sense(egui::Sense::click()))
                                .clicked()
                            {
                                selected = Some(row.node_id.clone());
                            }
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui.small_button("Delete").clicked() {
                                        to_delete = Some(row.node_id.clone());
                                    }
                                },
                            );
                        });
                    });
                ui.add_space(1.0);
            }
        });

    if let Some(node_id) = selected {
        ui_mgr.push_action(UiAction::PathSelectTrack {
            track_node_id: node_id,
        });
    }
    if let Some(node_id) = to_delete {
        ui_mgr.push_action(UiAction::PathDeleteTrack {
            track_node_id: node_id,
        });
    }
}

fn render_edit_view(
    ui: &mut egui::Ui,
    path_state: &mut PathEditorState,
    path_status: &PathEditStatus,
    ui_mgr: &mut UiManager,
    // Kept for signature parity with the panel-caller contract (mod.rs); no
    // longer read here now that append is Pen-tool-driven, not cursor-driven.
    _cursor_world: &ViewportCursorWorld,
    track_id: &str,
    petal_id: &str,
) {
    ui.horizontal(|ui| {
        if ui.small_button("\u{2190} Back").clicked() {
            path_state.stop_editing();
            ui_mgr.push_action(UiAction::PathQueryTracks {
                petal_id: petal_id.to_string(),
            });
            return;
        }
        ui.label(
            egui::RichText::new("Editing path")
                .strong()
                .color(theme::TEXT_SECTION),
        );
    });
    if path_state.editing_track_id.is_none() {
        return;
    }
    ui.add_space(4.0);

    ui.label(
        egui::RichText::new("Select the Pen tool (P) and click the viewport to draw the path.")
            .small()
            .color(theme::TEXT_MUTED)
            .italics(),
    );
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        if ui
            .add(egui::Button::new("Export GPX").fill(theme::BG_SAVE))
            .clicked()
        {
            ui_mgr.push_action(UiAction::PathExportGpx {
                track_node_id: track_id.to_string(),
            });
        }
    });

    // track_styling_20260713: per-track style controls (color / thickness /
    // visibility). Bound to `edited_track_style` (seeded from the track's
    // `gis.track.*` props on select); each change emits a focused `PathSetStyle`
    // via the deferred-push idiom so the borrow on `path_state` ends first.
    let to_style = render_style_controls(ui, &mut path_state.edited_track_style);
    if let Some((color, width, visible)) = to_style {
        ui_mgr.push_action(UiAction::PathSetStyle {
            track_node_id: track_id.to_string(),
            color,
            width,
            visible,
        });
    }

    if path_status.track_node_id.as_deref() == Some(track_id) {
        ui.add_space(4.0);
        if let Some(err) = &path_status.error {
            ui.label(
                egui::RichText::new(format!("\u{2717} {err}"))
                    .small()
                    .color(theme::STATUS_OFFLINE),
            );
        } else if let Some(msg) = &path_status.message {
            ui.label(
                egui::RichText::new(format!("\u{2713} {msg}"))
                    .small()
                    .color(theme::STATUS_ONLINE),
            );
        }
    }

    ui.add_space(6.0);
    ui.separator();
    ui.add_space(4.0);
    ui.label(
        egui::RichText::new(format!("Points ({})", path_state.points.len()))
            .strong()
            .color(theme::TEXT_SECTION),
    );
    ui.add_space(4.0);

    if path_state.points.is_empty() {
        ui.label(
            egui::RichText::new("No points yet — select the Pen tool (P) and click the viewport.")
                .small()
                .color(theme::TEXT_MUTED)
                .italics(),
        );
        return;
    }

    // FR-3 (path_interaction_20260716): real-world measurements, computed by
    // `node_manager::sync_path_measurements` (the panel can't reach
    // `world_scale`). Total length is always shown; a selected segment (or
    // vertex) shows its own readout below it.
    if let Some(total_m) = path_state.total_length_m {
        ui.label(
            egui::RichText::new(format!("Total length: {}", format_distance_m(total_m)))
                .small()
                .color(theme::TEXT_SECTION),
        );
    }
    if let Some(seg) = path_state.selected_segment {
        let len = path_state
            .selected_segment_length_m
            .map(format_distance_m)
            .unwrap_or_else(|| "\u{2014}".to_string());
        ui.label(
            egui::RichText::new(format!("Segment {}\u{2013}{}: {}", seg, seg + 1, len))
                .small()
                .color(theme::STATUS_ONLINE),
        );
    } else if let Some(pt) = path_state.selected_point {
        ui.label(
            egui::RichText::new(format!("Vertex {pt} selected"))
                .small()
                .color(theme::STATUS_ONLINE),
        );
    }
    ui.add_space(4.0);

    ui.label(
        egui::RichText::new("Pen tool: click terrain to add \u{2022} click a marker to select \u{2022} drag markers to move \u{2022} Ctrl+drag a marker to raise/lower height \u{2022} click the ribbon to select a segment \u{2022} Shift/Alt+click a marker to annotate")
            .small()
            .color(theme::TEXT_MUTED)
            .italics(),
    );
    ui.add_space(4.0);

    // FR-2/FR-3: mirror the viewport highlight in the list — the selected vertex
    // or the two ends of the selected segment get an active row fill.
    let sel_point = path_state.selected_point;
    let sel_seg = path_state.selected_segment;
    let mut to_remove: Option<usize> = None;
    let mut to_annotate: Option<usize> = None;
    // FR-1b: a numeric Height (Y, the user's "z-axis") edit on a point row.
    // Reuses `UiAction::PathMovePoint` (no new action) — it flows through
    // `PathOp::MovePoint` generically over all 3 position components.
    let mut to_move: Option<(usize, [f32; 3])> = None;
    egui::ScrollArea::vertical()
        .max_height(240.0)
        .show(ui, |ui| {
            for (i, point) in path_state.points.iter().enumerate() {
                let highlighted =
                    sel_point == Some(i) || sel_seg.is_some_and(|s| i == s || i == s + 1);
                let row_fill = if highlighted {
                    theme::BG_BUTTON_ACTIVE
                } else {
                    theme::BG_PEER_ROW_EVEN
                };
                egui::Frame::NONE
                    .fill(row_fill)
                    .inner_margin(egui::Margin::symmetric(6, 4))
                    .corner_radius(2.0)
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            // FR-1: point glyph — filled when the point carries a GPX
                            // timestamp, hollow when authored via the Pen (no time).
                            let glyph = if point.time_seconds.is_some() {
                                GLYPH_POINT
                            } else {
                                GLYPH_POINT_UNTIMED
                            };
                            ui.label(egui::RichText::new(glyph).small().color(theme::ICON_POINT));
                            ui.label(
                                egui::RichText::new(format!(
                                    "{i}: ({:.1}, {:.1}, {:.1})",
                                    point.position[0], point.position[1], point.position[2]
                                ))
                                .color(theme::TEXT_BRIGHT),
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui.small_button("Remove").clicked() {
                                        to_remove = Some(i);
                                    }
                                    if ui.small_button("Annotate").clicked() {
                                        to_annotate = Some(i);
                                    }
                                    // Numeric height (Bevy Y) field for this point.
                                    let mut new_y = point.position[1];
                                    let resp = ui.add(
                                        egui::DragValue::new(&mut new_y).speed(0.05).prefix("Y "),
                                    );
                                    if resp.changed() {
                                        to_move = Some((
                                            i,
                                            [point.position[0], new_y, point.position[2]],
                                        ));
                                    }
                                },
                            );
                        });
                    });
                ui.add_space(1.0);
            }
        });

    if let Some(index) = to_remove {
        ui_mgr.push_action(UiAction::PathRemovePoint {
            track_node_id: track_id.to_string(),
            index,
        });
        if path_state.annotating_index == Some(index) {
            path_state.close_annotate_form();
        }
    }
    if let Some((index, position)) = to_move {
        ui_mgr.push_action(UiAction::PathMovePoint {
            track_node_id: track_id.to_string(),
            index,
            position,
        });
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

/// track_styling_20260713: renders the color / thickness / visibility controls
/// for the edited track, mutating `style` in place for immediate UI feedback.
/// Returns `Some((color?, width?, visible?))` with ONLY the field that changed
/// set, so the caller emits a focused `PathSetStyle` (untouched props aren't
/// clobbered). `None` when nothing needs persisting this frame.
///
/// MEDIUM-2 (persist on release, not per-frame): a color-picker/slider drag
/// fires `.changed()` every frame, and each `PathSetStyle` triggers a
/// `SetNodeProperty` → refetch → despawn/respawn/ribbon-rebuild round-trip in
/// the gpx bridge — dozens of full mesh rebuilds per drag. So `style` is still
/// mutated live every frame (the picker/slider shows the value immediately),
/// but the returned persist signal only fires when the widget's drag *ends*
/// (`drag_stopped`) or a settled non-drag change lands (keyboard / step click /
/// checkbox toggle). Live visual feedback is preserved; the DB write happens
/// once at release. See `fe-ui/src/AGENTS.md` §path-editor.
#[allow(clippy::type_complexity)]
fn render_style_controls(
    ui: &mut egui::Ui,
    style: &mut crate::gis::TrackStyleFields,
) -> Option<(Option<[f32; 4]>, Option<f32>, Option<bool>)> {
    let mut changed_color: Option<[f32; 4]> = None;
    let mut changed_width: Option<f32> = None;
    let mut changed_visible: Option<bool> = None;

    ui.add_space(6.0);
    ui.separator();
    ui.add_space(4.0);
    ui.label(
        egui::RichText::new("Style")
            .strong()
            .color(theme::TEXT_SECTION),
    );
    ui.add_space(4.0);

    ui.horizontal(|ui| {
        ui.label("Color");
        // egui's picker works in sRGB `Color32` (unmultiplied). Convert to/from
        // our `[f32; 4]` sRGB representation.
        let mut rgba = egui::Color32::from_rgba_unmultiplied(
            (style.color[0].clamp(0.0, 1.0) * 255.0).round() as u8,
            (style.color[1].clamp(0.0, 1.0) * 255.0).round() as u8,
            (style.color[2].clamp(0.0, 1.0) * 255.0).round() as u8,
            (style.color[3].clamp(0.0, 1.0) * 255.0).round() as u8,
        );
        let resp = egui::color_picker::color_edit_button_srgba(
            ui,
            &mut rgba,
            egui::color_picker::Alpha::Opaque,
        );
        if resp.changed() {
            // Live feedback: mirror the picked value into `style` every frame so
            // the swatch/preview tracks the drag.
            style.color = [
                rgba.r() as f32 / 255.0,
                rgba.g() as f32 / 255.0,
                rgba.b() as f32 / 255.0,
                rgba.a() as f32 / 255.0,
            ];
        }
        // Persist only when the color change has settled — i.e. a `.changed()`
        // observed while the primary pointer is NOT held down. The button
        // response doesn't reflect the popup's internal slider drag, so we can't
        // use `drag_stopped()` here; keying off "changed while pointer released"
        // suppresses every mid-drag frame (pointer down) and fires on the commit
        // frame after the user lets go (pointer up). Keyboard/hex edits (no
        // pointer) also land here immediately, which is fine — they're discrete.
        if resp.changed() && ui.input(|i| !i.pointer.primary_down()) {
            changed_color = Some(style.color);
        }
    });

    ui.horizontal(|ui| {
        ui.label("Thickness");
        let mut w = style.width;
        // FR-5 (path_interaction_20260716): range starts at 0.1 (step 0.1) so
        // the new 0.5 default can be dialed even thinner.
        let resp = ui.add(egui::Slider::new(&mut w, 0.1..=20.0).step_by(0.1));
        if resp.changed() {
            // Live feedback every frame; persist only on release below.
            style.width = w;
        }
        // Persist on drag-release, or on a settled non-drag change (keyboard /
        // single step click that never entered a drag).
        if resp.drag_stopped() || (resp.changed() && !resp.dragged()) {
            changed_width = Some(style.width);
        }
    });

    ui.horizontal(|ui| {
        let mut v = style.visible;
        // A checkbox is a single click — no per-frame drag churn — so persist
        // immediately on change.
        if ui.add(egui::Checkbox::new(&mut v, "Visible")).changed() {
            style.visible = v;
            changed_visible = Some(v);
        }
    });

    if changed_color.is_some() || changed_width.is_some() || changed_visible.is_some() {
        Some((changed_color, changed_width, changed_visible))
    } else {
        None
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
                ui.add(
                    egui::TextEdit::singleline(&mut path_state.annotate_title_buf)
                        .desired_width(200.0),
                );
            });
            ui.horizontal(|ui| {
                ui.label("Body ");
                ui.add(
                    egui::TextEdit::singleline(&mut path_state.annotate_body_buf)
                        .desired_width(200.0),
                );
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
                if ui
                    .add(egui::Button::new("Save").fill(theme::BG_SAVE))
                    .clicked()
                {
                    ui_mgr.push_action(UiAction::PathAnnotatePoint {
                        track_node_id: track_id.to_string(),
                        index,
                        title: path_state.annotate_title_buf.clone(),
                        body: path_state.annotate_body_buf.clone(),
                        color: path_state.annotate_color_buf.clone(),
                    });
                    path_state.close_annotate_form();
                }
                if ui
                    .add(egui::Button::new("Cancel").fill(theme::BG_BUTTON))
                    .clicked()
                {
                    path_state.close_annotate_form();
                }
            });
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn type_glyph_maps_known_types() {
        assert_eq!(type_glyph("track"), GLYPH_TRACK);
        assert_eq!(type_glyph("waypoint"), GLYPH_WAYPOINT);
        assert_eq!(type_glyph("trackpoint"), GLYPH_POINT);
    }

    #[test]
    fn type_glyph_unknown_falls_back_to_point() {
        assert_eq!(type_glyph("mystery"), GLYPH_POINT);
        assert_eq!(type_glyph(""), GLYPH_POINT);
    }

    #[test]
    fn format_distance_meters_and_kilometers() {
        // Mirrors the fe-terrain ruler twin so the two stay in lock-step.
        assert_eq!(format_distance_m(500.0), "500 m");
        assert_eq!(format_distance_m(2.5), "2.5 m");
        assert_eq!(format_distance_m(0.5), "0.50 m");
        assert_eq!(format_distance_m(1000.0), "1 km");
        assert_eq!(format_distance_m(1500.0), "1.5 km");
        assert_eq!(format_distance_m(50_000.0), "50 km");
    }

    #[test]
    fn format_distance_bad_input_is_zero() {
        assert_eq!(format_distance_m(0.0), "0 m");
        assert_eq!(format_distance_m(-5.0), "0 m");
        assert_eq!(format_distance_m(f64::NAN), "0 m");
    }

    #[test]
    fn glyphs_are_single_recolorable_codepoints() {
        // Each glyph is exactly one Unicode scalar in the geometric-shapes /
        // dingbats range egui recolors reliably (no color-emoji fallback).
        for g in [
            GLYPH_TRACK,
            GLYPH_POINT,
            GLYPH_POINT_UNTIMED,
            GLYPH_WAYPOINT,
        ] {
            assert_eq!(g.chars().count(), 1, "glyph {g:?} must be one codepoint");
        }
    }
}
