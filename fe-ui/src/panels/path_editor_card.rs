//! Paths tab: list the active petal's track nodes, create/select/delete
//! tracks, and edit the selected track's point list (Pen-tool click-to-place,
//! remove, annotate, export). fe-ui queues intent only — see
//! `crate::path_ops` for the op-queue/status contract, `node_manager/AGENTS.md`
//! §pen-tool for the Pen tool, and `fe-ui/src/AGENTS.md` §path-editor for the
//! end-to-end design.

use bevy_egui::egui;

use crate::actions::{UiAction, UiManager};
use crate::gis::{
    min_neighbor_gap_m, smoothness_readback, CornerKind, PathEditorState, PathPointRow,
};
use crate::node_manager::curve;
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
            egui::RichText::new(format!("Paths ({})", path_state.tracks.len()))
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
    // Two-step Delete confirm — same convention as `entity_settings.rs` Delete.
    let mut arm_delete: Option<String> = None;
    let mut cancel_delete = false;
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
                            let pending = path_state.pending_track_delete.as_deref()
                                == Some(row.node_id.as_str());
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if pending {
                                        if ui.small_button("Cancel").clicked() {
                                            cancel_delete = true;
                                        }
                                        if ui
                                            .add(
                                                egui::Button::new(
                                                    egui::RichText::new("Confirm Delete")
                                                        .small()
                                                        .color(egui::Color32::WHITE),
                                                )
                                                .fill(theme::BG_DANGER),
                                            )
                                            .clicked()
                                        {
                                            to_delete = Some(row.node_id.clone());
                                        }
                                    } else if ui.small_button("Delete").clicked() {
                                        arm_delete = Some(row.node_id.clone());
                                    }
                                },
                            );
                        });
                    });
                ui.add_space(1.0);
            }
        });

    if let Some(node_id) = arm_delete {
        path_state.pending_track_delete = Some(node_id);
    }
    if cancel_delete {
        path_state.pending_track_delete = None;
    }
    if let Some(node_id) = selected {
        path_state.pending_track_delete = None;
        ui_mgr.push_action(UiAction::PathSelectTrack {
            track_node_id: node_id,
        });
    }
    if let Some(node_id) = to_delete {
        path_state.pending_track_delete = None;
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

    // pen_curve_tool_20260722 (FR-6): per-anchor corner settings for the
    // selected vertex — same deferred-push idiom as the style controls above
    // (live buffer edit, persist signal consumed after the borrow ends).
    if let Some(idx) = path_state.selected_point {
        let (to_corner, to_handles) = render_corner_settings(ui, &mut path_state.points, idx);
        if let Some(corner) = to_corner {
            ui_mgr.push_action(UiAction::PathSetAnchorCorner {
                track_node_id: track_id.to_string(),
                index: idx,
                corner,
            });
        }
        if let Some((handle_in, handle_out, smoothness)) = to_handles {
            ui_mgr.push_action(UiAction::PathSetAnchorHandles {
                track_node_id: track_id.to_string(),
                index: idx,
                handle_in,
                handle_out,
                smoothness,
            });
        }
        ui.add_space(4.0);
    }

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
        // Range starts at 0.1 (step 0.1), matching the thin default (petal-local
        // meters). Dial up for wider roads/paths.
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

/// Symmetric-collinear tolerance for the Q5 enable predicate, petal-meters.
const HANDLE_SYMMETRY_EPS_M: f32 = 1e-3;
/// Floor smoothness when toggling a HANDLE-LESS anchor to Smooth/Symmetric —
/// visible default roundness so the classification round-trips (a handle-less
/// Smooth row would decode back to Corner, ratified Q6).
const HANDLELESS_TOGGLE_SMOOTHNESS: f32 = 0.5;

/// Ratified Q5 enable predicate: the smoothness slider is live only while the
/// anchor's handles are symmetric (`handle_in ≈ −handle_out` within epsilon)
/// or both absent; a manually-broken anchor greys it out. Pure.
fn smoothness_slider_enabled(handle_in: Option<[f32; 3]>, handle_out: Option<[f32; 3]>) -> bool {
    match (handle_in, handle_out) {
        (None, None) => true,
        (Some(hin), Some(hout)) => {
            (0..3).all(|k| (hin[k] + hout[k]).abs() <= HANDLE_SYMMETRY_EPS_M)
        }
        _ => false,
    }
}

/// Seed for the ENABLED smoothness slider: geometry readback whenever the
/// anchor has handles (the stored scalar may be stale); the stored scalar only
/// drives handle-less anchors. See `fe-ui/src/AGENTS.md` §path-editor. Pure.
fn slider_seed(
    handle_in: Option<[f32; 3]>,
    handle_out: Option<[f32; 3]>,
    min_gap_m: Option<f32>,
    stored: f32,
) -> f32 {
    if handle_in.is_some() || handle_out.is_some() {
        smoothness_readback(handle_in, handle_out, min_gap_m)
    } else {
        stored
    }
}

/// A persisted zero-handle state on a Smooth/Symmetric anchor reverts the kind
/// to Corner — handle-less Smooth cannot round-trip (ratified Q6 decode). Pure.
fn zero_handle_kind_revert(
    handle_in: Option<[f32; 3]>,
    handle_out: Option<[f32; 3]>,
    kind: CornerKind,
) -> Option<CornerKind> {
    (handle_in.is_none() && handle_out.is_none() && kind != CornerKind::Corner)
        .then_some(CornerKind::Corner)
}

/// Deferred `PathSetAnchorHandles` payload: `(handle_in, handle_out, smoothness)`.
type HandlesPersist = (Option<[f32; 3]>, Option<[f32; 3]>, f32);

/// What a corner-toggle click does to the row buffer and the persist queue.
#[derive(Debug, Clone, Copy, PartialEq)]
struct CornerToggleOutcome {
    /// Kind the row buffer shows after the click (may differ from the clicked
    /// kind when no handles are derivable — see `zero_handle_kind_revert`).
    effective: CornerKind,
    /// `PathSetAnchorCorner` persist signal, when the kind changed.
    corner: Option<CornerKind>,
    /// `PathSetAnchorHandles` persist signal.
    handles: Option<HandlesPersist>,
}

/// Ratified Q5 toggle outcome, NON-destructive: an anchor whose handles are
/// already symmetric-collinear reclassifies only (handles untouched); the
/// re-derive fires ONLY from the broken/asymmetric grey-out state (at the
/// geometry READBACK, not the stored scalar) or on a handle-less anchor (at
/// the 0.5 floor). See `fe-ui/src/AGENTS.md` §path-editor. Pure.
fn corner_toggle_outcome(
    row: &PathPointRow,
    clicked: CornerKind,
    prev: Option<[f32; 3]>,
    next: Option<[f32; 3]>,
) -> CornerToggleOutcome {
    let has_handles = row.handle_in.is_some() || row.handle_out.is_some();
    let rederive = matches!(clicked, CornerKind::Smooth | CornerKind::Symmetric)
        && !(has_handles && smoothness_slider_enabled(row.handle_in, row.handle_out));
    if !rederive {
        return CornerToggleOutcome {
            effective: clicked,
            corner: (clicked != row.corner).then_some(clicked),
            handles: None,
        };
    }
    let s = if has_handles {
        smoothness_readback(
            row.handle_in,
            row.handle_out,
            min_neighbor_gap_m(prev, row.position, next),
        )
    } else {
        row.smoothness.max(HANDLELESS_TOGGLE_SMOOTHNESS)
    };
    let (hin, hout, effective) = match curve::derive_symmetric_handles(prev, row.position, next, s)
    {
        Some((hin, hout)) => (Some(hin), Some(hout), clicked),
        // No derivable handles (no neighbors / zero readback): stay coherent
        // with the Q6 decode guard — a handle-less anchor is a Corner.
        None => (None, None, CornerKind::Corner),
    };
    CornerToggleOutcome {
        effective,
        corner: (effective != row.corner).then_some(effective),
        handles: Some((hin, hout, s)),
    }
}

/// pen_curve_tool_20260722 (FR-6): "Corner settings" sub-card for the selected
/// vertex — live-edits the row, returns deferred persist signals (consumed
/// after the `points` borrow ends). See `fe-ui/src/AGENTS.md` §path-editor.
fn render_corner_settings(
    ui: &mut egui::Ui,
    points: &mut [PathPointRow],
    idx: usize,
) -> (Option<CornerKind>, Option<HandlesPersist>) {
    // Copy the neighbor positions before mutably borrowing the row itself.
    let prev = idx
        .checked_sub(1)
        .and_then(|i| points.get(i))
        .map(|p| p.position);
    let next = points.get(idx + 1).map(|p| p.position);
    let Some(row) = points.get_mut(idx) else {
        return (None, None);
    };

    let mut to_corner: Option<CornerKind> = None;
    let mut to_handles: Option<HandlesPersist> = None;

    egui::Frame::NONE
        .fill(theme::BG_PEER_ROW_EVEN)
        .inner_margin(egui::Margin::same(8))
        .corner_radius(3.0)
        .show(ui, |ui| {
            ui.label(
                egui::RichText::new(format!("Corner settings — vertex {idx}"))
                    .strong()
                    .color(theme::TEXT_SECTION),
            );
            ui.add_space(4.0);

            // Segmented Corner/Smooth/Symmetric toggle. Discrete widget ⇒
            // persist immediately on click; re-derive only from the broken
            // grey-out state (Q5 re-enable route) — never on symmetric handles.
            let mut clicked_kind: Option<CornerKind> = None;
            ui.horizontal(|ui| {
                for kind in [CornerKind::Corner, CornerKind::Smooth, CornerKind::Symmetric] {
                    if ui
                        .selectable_label(row.corner == kind, kind.label())
                        .clicked()
                    {
                        clicked_kind = Some(kind);
                    }
                }
            });
            if let Some(kind) = clicked_kind {
                let out = corner_toggle_outcome(row, kind, prev, next);
                row.corner = out.effective;
                if let Some((hin, hout, s)) = out.handles {
                    row.handle_in = hin;
                    row.handle_out = hout;
                    row.smoothness = s;
                }
                to_corner = out.corner;
                to_handles = out.handles;
            }
            ui.add_space(4.0);

            let min_gap = min_neighbor_gap_m(prev, row.position, next);
            if smoothness_slider_enabled(row.handle_in, row.handle_out) {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("Smoothness")
                            .small()
                            .color(theme::TEXT_DIM),
                    );
                    // Seed from geometry truth when handles exist — the stored
                    // scalar may be stale (drag-created anchors).
                    let mut s = slider_seed(row.handle_in, row.handle_out, min_gap, row.smoothness);
                    let resp = ui.add(egui::Slider::new(&mut s, 0.0..=1.0).step_by(0.01));
                    if resp.changed() {
                        // Live feedback every frame: derive collinear symmetric
                        // handles into the buffer; persist only on release below.
                        row.smoothness = s;
                        let (hin, hout) =
                            match curve::derive_symmetric_handles(prev, row.position, next, s) {
                                Some((hin, hout)) => (Some(hin), Some(hout)),
                                None => (None, None),
                            };
                        row.handle_in = hin;
                        row.handle_out = hout;
                    }
                    if resp.drag_stopped() || (resp.changed() && !resp.dragged()) {
                        // Slider at exactly 0 cleared the handles: revert the
                        // kind to Corner locally AND persisted so UI and disk
                        // agree (Q6 — handle-less Smooth can't round-trip).
                        if let Some(kind) =
                            zero_handle_kind_revert(row.handle_in, row.handle_out, row.corner)
                        {
                            row.corner = kind;
                            to_corner = Some(kind);
                        }
                        to_handles = Some((row.handle_in, row.handle_out, row.smoothness));
                    }
                });
                if let Some(hout) = row.handle_out {
                    let len = (hout[0].powi(2) + hout[1].powi(2) + hout[2].powi(2)).sqrt();
                    ui.label(
                        egui::RichText::new(format!(
                            "Handle length: {}",
                            format_distance_m(len as f64)
                        ))
                        .small()
                        .color(theme::TEXT_MUTED),
                    );
                }
            } else {
                // Q5 grey-out: manually-broken handles have no single scalar
                // smoothness — show the approximate readback, never overwrite.
                let readback = smoothness_readback(row.handle_in, row.handle_out, min_gap);
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("Smoothness")
                            .small()
                            .color(theme::TEXT_DIM),
                    );
                    let mut s = readback;
                    ui.add_enabled(false, egui::Slider::new(&mut s, 0.0..=1.0));
                });
                let fmt_len = |h: Option<[f32; 3]>| {
                    h.map(|v| {
                        format_distance_m(
                            (v[0].powi(2) + v[1].powi(2) + v[2].powi(2)).sqrt() as f64,
                        )
                    })
                    .unwrap_or_else(|| "\u{2014}".to_string())
                };
                ui.label(
                    egui::RichText::new(format!(
                        "Handles edited by hand (\u{2248}{readback:.2}) — in {}, out {}. Select Smooth or Symmetric to re-derive.",
                        fmt_len(row.handle_in),
                        fmt_len(row.handle_out),
                    ))
                    .small()
                    .color(theme::TEXT_MUTED)
                    .italics(),
                );
            }
        });

    (to_corner, to_handles)
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
    use crate::gis::SMOOTHNESS_K;

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

    // ---- corner settings (pen_curve_tool_20260722 FR-6 / ratified Q5) ----

    #[test]
    fn smoothness_slider_enabled_for_symmetric_or_absent_handles() {
        assert!(smoothness_slider_enabled(None, None));
        assert!(smoothness_slider_enabled(
            Some([-1.0, 0.0, -0.5]),
            Some([1.0, 0.0, 0.5])
        ));
        // Within the epsilon tolerance still counts as symmetric.
        assert!(smoothness_slider_enabled(
            Some([-1.0, 0.0, 0.0]),
            Some([1.0005, 0.0, 0.0])
        ));
    }

    #[test]
    fn smoothness_slider_disabled_when_manually_broken() {
        // Asymmetric lengths (a Smooth-style drag) have no single scalar.
        assert!(!smoothness_slider_enabled(
            Some([-3.0, 0.0, 0.0]),
            Some([1.0, 0.0, 0.0])
        ));
        // Non-collinear break.
        assert!(!smoothness_slider_enabled(
            Some([-1.0, 0.0, 0.0]),
            Some([0.0, 0.0, 1.0])
        ));
        // One-sided handle (combination anchor).
        assert!(!smoothness_slider_enabled(None, Some([1.0, 0.0, 0.0])));
        assert!(!smoothness_slider_enabled(Some([1.0, 0.0, 0.0]), None));
    }

    #[test]
    fn min_neighbor_gap_prefers_smaller_and_handles_endpoints() {
        let cur = [0.0, 0.0, 0.0];
        assert_eq!(
            min_neighbor_gap_m(Some([1.0, 0.0, 0.0]), cur, Some([0.0, 0.0, 4.0])),
            Some(1.0)
        );
        assert_eq!(
            min_neighbor_gap_m(None, cur, Some([0.0, 0.0, 4.0])),
            Some(4.0)
        );
        assert_eq!(
            min_neighbor_gap_m(Some([1.0, 0.0, 0.0]), cur, None),
            Some(1.0)
        );
        assert_eq!(min_neighbor_gap_m(None, cur, None), None);
    }

    #[test]
    fn smoothness_readback_round_trips_derive() {
        // Readback is the inverse of the derive length rule at any smoothness.
        let prev = Some([0.0, 0.0, 0.0]);
        let cur = [2.0, 0.0, 0.0];
        let next = Some([2.0, 0.0, 2.0]);
        let gap = min_neighbor_gap_m(prev, cur, next);
        for s in [0.25_f32, 0.5, 1.0] {
            let (hin, hout) = curve::derive_symmetric_handles(prev, cur, next, s).unwrap();
            let rb = smoothness_readback(Some(hin), Some(hout), gap);
            assert!((rb - s).abs() < 1e-4, "smoothness {s} read back as {rb}");
        }
    }

    #[test]
    fn smoothness_zero_is_sharp_and_one_is_round() {
        let prev = Some([0.0, 0.0, 0.0]);
        let cur = [2.0, 0.0, 0.0];
        let next = Some([2.0, 0.0, 2.0]);
        // 0 ⇒ no handles at all (sharp) — and the slider stays enabled.
        assert!(curve::derive_symmetric_handles(prev, cur, next, 0.0).is_none());
        assert!(smoothness_slider_enabled(None, None));
        assert_eq!(
            smoothness_readback(None, None, min_neighbor_gap_m(prev, cur, next)),
            0.0
        );
        // 1 ⇒ full-length handles: |out| = k · min gap (round); readback 1.
        let (_, hout) = curve::derive_symmetric_handles(prev, cur, next, 1.0).unwrap();
        let len = (hout[0].powi(2) + hout[1].powi(2) + hout[2].powi(2)).sqrt();
        assert!((len - SMOOTHNESS_K * 2.0).abs() < 1e-5, "got {len}");
        assert!((smoothness_readback(None, Some(hout), Some(2.0)) - 1.0).abs() < 1e-5);
    }

    #[test]
    fn smoothness_readback_falls_back_to_in_handle_and_clamps() {
        // Out missing ⇒ read the in-handle; over-long handles clamp to 1.
        let rb = smoothness_readback(Some([0.5, 0.0, 0.0]), None, Some(3.0));
        assert!((rb - 0.5).abs() < 1e-5, "0.5 / (1/3 · 3) = 0.5, got {rb}");
        assert_eq!(
            smoothness_readback(None, Some([9.0, 0.0, 0.0]), Some(3.0)),
            1.0
        );
        assert_eq!(
            smoothness_readback(Some([1.0, 0.0, 0.0]), Some([1.0, 0.0, 0.0]), None),
            0.0
        );
    }

    /// Row fixture between prev (0,0,0) and next (2,0,2), at (2,0,0):
    /// both neighbor gaps are 2 m.
    fn toggle_row(
        corner: CornerKind,
        handle_in: Option<[f32; 3]>,
        handle_out: Option<[f32; 3]>,
        smoothness: f32,
    ) -> PathPointRow {
        PathPointRow {
            position: [2.0, 0.0, 0.0],
            handle_in,
            handle_out,
            corner,
            smoothness,
            ..Default::default()
        }
    }
    const TOGGLE_PREV: Option<[f32; 3]> = Some([0.0, 0.0, 0.0]);
    const TOGGLE_NEXT: Option<[f32; 3]> = Some([2.0, 0.0, 2.0]);

    #[test]
    fn corner_toggle_rederive_from_broken_state_uses_readback() {
        // Manually broken handles grey the slider out…
        let row = toggle_row(
            CornerKind::Corner,
            Some([-0.2, 0.0, 0.0]),
            Some([1.0, 0.0, 0.0]),
            0.5,
        );
        assert!(!smoothness_slider_enabled(row.handle_in, row.handle_out));
        // …and clicking Smooth re-derives at the geometry READBACK (|out|=1 /
        // (k·2) = 1.5 → clamp 1.0), NOT at the stale stored 0.5.
        let out = corner_toggle_outcome(&row, CornerKind::Smooth, TOGGLE_PREV, TOGGLE_NEXT);
        assert_eq!(out.effective, CornerKind::Smooth);
        assert_eq!(out.corner, Some(CornerKind::Smooth));
        let (hin, hout, s) = out.handles.expect("broken state re-derives");
        assert_eq!(s, 1.0, "readback value, not the stored scalar");
        assert!(hout.is_some());
        assert!(
            smoothness_slider_enabled(hin, hout),
            "re-enabled after re-derive"
        );
    }

    #[test]
    fn corner_toggle_to_corner_keeps_manual_handles() {
        // Corner click reclassifies only — it never touches the handles.
        let row = toggle_row(
            CornerKind::Symmetric,
            Some([-0.6, 0.0, 0.0]),
            Some([0.6, 0.0, 0.0]),
            1.0,
        );
        let out = corner_toggle_outcome(&row, CornerKind::Corner, TOGGLE_PREV, TOGGLE_NEXT);
        assert_eq!(out.effective, CornerKind::Corner);
        assert_eq!(out.corner, Some(CornerKind::Corner));
        assert!(out.handles.is_none());
    }

    #[test]
    fn corner_toggle_preserves_existing_symmetric_handles() {
        // Symmetric-collinear handles: a same-kind re-click is a full no-op,
        // and a Smooth↔Symmetric switch reclassifies WITHOUT re-deriving —
        // hand-pulled (still symmetric) handles are never wiped.
        let hin = Some([-0.9, 0.0, -0.3]);
        let hout = Some([0.9, 0.0, 0.3]);
        let row = toggle_row(CornerKind::Symmetric, hin, hout, 0.0);
        let reclick = corner_toggle_outcome(&row, CornerKind::Symmetric, TOGGLE_PREV, TOGGLE_NEXT);
        assert_eq!(reclick.effective, CornerKind::Symmetric);
        assert!(reclick.corner.is_none());
        assert!(reclick.handles.is_none(), "re-click never touches handles");
        let switch = corner_toggle_outcome(&row, CornerKind::Smooth, TOGGLE_PREV, TOGGLE_NEXT);
        assert_eq!(switch.corner, Some(CornerKind::Smooth));
        assert!(switch.handles.is_none(), "switch reclassifies only");
    }

    #[test]
    fn corner_toggle_on_sharp_anchor_derives_at_floor_and_persists() {
        // Handle-less anchor at smoothness 0: derive at the 0.5 floor so the
        // handles exist and the Smooth classification survives reload (Q6).
        let row = toggle_row(CornerKind::Corner, None, None, 0.0);
        let out = corner_toggle_outcome(&row, CornerKind::Smooth, TOGGLE_PREV, TOGGLE_NEXT);
        assert_eq!(out.effective, CornerKind::Smooth);
        assert_eq!(out.corner, Some(CornerKind::Smooth));
        let (hin, hout, s) = out.handles.expect("floor derive persists handles");
        assert_eq!(s, HANDLELESS_TOGGLE_SMOOTHNESS);
        assert!(hin.is_some() && hout.is_some(), "handles now exist");
        // A stored smoothness above the floor is kept as-is.
        let row = toggle_row(CornerKind::Corner, None, None, 0.8);
        let out = corner_toggle_outcome(&row, CornerKind::Smooth, TOGGLE_PREV, TOGGLE_NEXT);
        assert_eq!(out.handles.map(|(_, _, s)| s), Some(0.8));
    }

    #[test]
    fn corner_toggle_without_derivable_handles_stays_corner() {
        // No neighbors at all: nothing to derive — the outcome stays Corner
        // (coherent with the Q6 decode guard) instead of persisting a
        // handle-less Smooth that would silently revert on reload.
        let row = toggle_row(CornerKind::Corner, None, None, 0.0);
        let out = corner_toggle_outcome(&row, CornerKind::Smooth, None, None);
        assert_eq!(out.effective, CornerKind::Corner);
        assert!(out.corner.is_none(), "kind never left Corner");
        assert_eq!(
            out.handles,
            Some((None, None, HANDLELESS_TOGGLE_SMOOTHNESS))
        );
    }

    #[test]
    fn slider_seed_reads_geometry_truth_when_handles_exist() {
        // Drag-created anchor: stored 0.0 but real symmetric handles — the
        // slider must seed from the readback, not show 0.00 on a round anchor.
        let gap = Some(3.0);
        let seed = slider_seed(Some([-0.5, 0.0, 0.0]), Some([0.5, 0.0, 0.0]), gap, 0.0);
        assert!((seed - 0.5).abs() < 1e-5, "0.5 / (k·3) = 0.5, got {seed}");
        // Handle-less anchor: the stored scalar drives the seed.
        assert_eq!(slider_seed(None, None, gap, 0.7), 0.7);
    }

    #[test]
    fn zero_handle_revert_fires_only_on_handleless_non_corner() {
        assert_eq!(
            zero_handle_kind_revert(None, None, CornerKind::Smooth),
            Some(CornerKind::Corner)
        );
        assert_eq!(
            zero_handle_kind_revert(None, None, CornerKind::Symmetric),
            Some(CornerKind::Corner)
        );
        assert_eq!(
            zero_handle_kind_revert(None, None, CornerKind::Corner),
            None
        );
        assert_eq!(
            zero_handle_kind_revert(Some([1.0, 0.0, 0.0]), None, CornerKind::Smooth),
            None
        );
    }

    #[test]
    fn corner_settings_are_authority_b_only() {
        // Split compliance (ui_ux.md §5): the whole corner-settings decision is
        // computable from PathEditorState data alone — no NodeManager anywhere
        // in this module (compile-enforced) — and maps 1:1 onto the queued
        // Authority-B `PathSetAnchorCorner`/`PathSetAnchorHandles` actions.
        let mut state = PathEditorState {
            editing_track_id: Some("track:1".to_string()),
            points: vec![
                PathPointRow {
                    position: [0.0, 0.0, 0.0],
                    ..Default::default()
                },
                PathPointRow {
                    position: [3.0, 0.0, 0.0],
                    smoothness: 1.0,
                    ..Default::default()
                },
            ],
            selected_point: Some(1),
            ..Default::default()
        };

        let idx = state.selected_point.unwrap();
        let prev = idx
            .checked_sub(1)
            .and_then(|i| state.points.get(i))
            .map(|p| p.position);
        let next = state.points.get(idx + 1).map(|p| p.position);
        let out = corner_toggle_outcome(&state.points[idx], CornerKind::Smooth, prev, next);

        // Apply exactly as the card does: mutate the local buffer…
        let (hin, hout, s) = out.handles.expect("Smooth click derives");
        {
            let row = &mut state.points[idx];
            row.corner = out.effective;
            row.handle_in = hin;
            row.handle_out = hout;
            row.smoothness = s;
        }
        // …and queue Authority-B actions only.
        let actions = [
            UiAction::PathSetAnchorCorner {
                track_node_id: "track:1".to_string(),
                index: idx,
                corner: out.corner.expect("kind changed"),
            },
            UiAction::PathSetAnchorHandles {
                track_node_id: "track:1".to_string(),
                index: idx,
                handle_in: hin,
                handle_out: hout,
                smoothness: s,
            },
        ];
        assert!(matches!(
            actions[0],
            UiAction::PathSetAnchorCorner { index: 1, .. }
        ));
        assert!(matches!(
            actions[1],
            UiAction::PathSetAnchorHandles { index: 1, .. }
        ));
        assert!(state.points[1].handle_out.is_some(), "buffer echoed live");
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
