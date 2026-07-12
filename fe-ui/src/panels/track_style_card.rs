//! FR-10 per-track line style controls (color / line style / visibility) for
//! the Paths tab edit view. Persists flat `gis.track.*` properties via the
//! existing `SetNodeProperty` action path — fe-ui does no persistence I/O
//! itself. Reuses `annotation_card`'s hex color picker. See
//! `fe-ui/src/AGENTS.md` §path-editor.

use bevy_egui::egui;

use crate::actions::node_props::{TRACK_COLOR_KEY, TRACK_LINE_STYLE_KEY, TRACK_VISIBLE_KEY};
use crate::actions::{UiAction, UiManager};
use crate::gis::PathEditorState;
use crate::panels::annotation_card::{parse_hex_color, rgb_to_hex};
use crate::theme;

/// Renders the color picker, line-style selector, and visibility toggle for
/// the currently-edited track, queueing `SetNodeProperty` actions on change.
pub(crate) fn track_style_section(
    ui: &mut egui::Ui,
    path_state: &mut PathEditorState,
    ui_mgr: &mut UiManager,
    track_id: &str,
) {
    ui.add_space(6.0);
    ui.separator();
    ui.add_space(4.0);
    ui.label(egui::RichText::new("Line style").strong().color(theme::TEXT_SECTION));
    ui.add_space(4.0);

    // Color: hex text + swatch picker, mirroring annotation_card.
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Color").small().color(theme::TEXT_DIM));
        let mut rgb = parse_hex_color(&path_state.style_color_buf)
            .map(|(r, g, b)| [r, g, b])
            .unwrap_or([0, 204, 255]);
        if egui::widgets::color_picker::color_edit_button_srgb(ui, &mut rgb).changed() {
            path_state.style_color_buf = rgb_to_hex(rgb[0], rgb[1], rgb[2]);
            ui_mgr.push_action(UiAction::SetNodeProperty {
                node_id: track_id.to_string(),
                key: TRACK_COLOR_KEY.to_string(),
                value: serde_json::Value::String(path_state.style_color_buf.clone()),
            });
        }
        ui.add(
            egui::TextEdit::singleline(&mut path_state.style_color_buf)
                .hint_text("#RRGGBB")
                .desired_width(90.0),
        );
    });

    // Line style: solid / dashed (dashed no-ops to solid rendering in phase 2).
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Style").small().color(theme::TEXT_DIM));
        for style in ["solid", "dashed"] {
            let active = path_state.style_line_style == style;
            let btn = egui::Button::new(style)
                .fill(if active { theme::BG_BUTTON_ACTIVE } else { theme::BG_BUTTON });
            if ui.add(btn).clicked() && !active {
                path_state.style_line_style = style.to_string();
                ui_mgr.push_action(UiAction::SetNodeProperty {
                    node_id: track_id.to_string(),
                    key: TRACK_LINE_STYLE_KEY.to_string(),
                    value: serde_json::Value::String(style.to_string()),
                });
            }
        }
    });

    // Visibility toggle, mirroring layer_manager_card's visible checkbox.
    if ui.checkbox(&mut path_state.style_visible, "Visible").changed() {
        ui_mgr.push_action(UiAction::SetNodeProperty {
            node_id: track_id.to_string(),
            key: TRACK_VISIBLE_KEY.to_string(),
            value: serde_json::Value::Bool(path_state.style_visible),
        });
    }
    ui.add_space(4.0);
}

/// Pure builder for the actions a full style save would emit — one
/// `SetNodeProperty` per field (color/line_style/visible), in that order.
/// Kept testable without an egui context (mirrors `annotation_save_actions`).
pub(crate) fn track_style_actions(
    node_id: &str,
    color: &str,
    line_style: &str,
    visible: bool,
) -> Vec<UiAction> {
    vec![
        UiAction::SetNodeProperty {
            node_id: node_id.to_string(),
            key: TRACK_COLOR_KEY.to_string(),
            value: serde_json::Value::String(color.to_string()),
        },
        UiAction::SetNodeProperty {
            node_id: node_id.to_string(),
            key: TRACK_LINE_STYLE_KEY.to_string(),
            value: serde_json::Value::String(line_style.to_string()),
        },
        UiAction::SetNodeProperty {
            node_id: node_id.to_string(),
            key: TRACK_VISIBLE_KEY.to_string(),
            value: serde_json::Value::Bool(visible),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn style_actions_emit_three_set_props_in_order() {
        let actions = track_style_actions("track-1", "#ff8800", "dashed", false);
        assert_eq!(actions.len(), 3);
        match &actions[0] {
            UiAction::SetNodeProperty { node_id, key, value } => {
                assert_eq!(node_id, "track-1");
                assert_eq!(key, TRACK_COLOR_KEY);
                assert_eq!(value, &serde_json::json!("#ff8800"));
            }
            other => panic!("expected color SetNodeProperty, got {other:?}"),
        }
        match &actions[1] {
            UiAction::SetNodeProperty { key, value, .. } => {
                assert_eq!(key, TRACK_LINE_STYLE_KEY);
                assert_eq!(value, &serde_json::json!("dashed"));
            }
            other => panic!("expected line_style SetNodeProperty, got {other:?}"),
        }
        match &actions[2] {
            UiAction::SetNodeProperty { key, value, .. } => {
                assert_eq!(key, TRACK_VISIBLE_KEY);
                assert_eq!(value, &serde_json::json!(false));
            }
            other => panic!("expected visible SetNodeProperty, got {other:?}"),
        }
    }
}
