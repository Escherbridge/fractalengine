//! Inspector "Annotation" card: editor for the reserved `gis.annotation.*`
//! node properties (title/body/color). Reuses the Phase 5 custom-property
//! `SetNodeProperty`/`DeleteNodeProperty` action path exactly — see
//! `fe-ui/src/AGENTS.md` §gis-query-ui.

use bevy_egui::egui;

use crate::actions::node_props::{ANNOTATION_BODY_KEY, ANNOTATION_COLOR_KEY, ANNOTATION_TITLE_KEY};
use crate::actions::{UiAction, UiManager};
use crate::node_manager::NodeManager;
use crate::plugin::InspectorFormState;
use crate::theme;

pub(crate) fn annotation_card_section(
    ui: &mut egui::Ui,
    inspector: &mut InspectorFormState,
    node_mgr: &NodeManager,
    ui_mgr: &mut UiManager,
) {
    let Some(sel) = node_mgr.selected.as_ref() else {
        return;
    };
    let node_id = sel.node_id.clone();

    egui::CollapsingHeader::new(
        egui::RichText::new("Annotation")
            .strong()
            .color(theme::TEXT_SECTION),
    )
    .default_open(false)
    .show(ui, |ui| {
        ui.add_space(4.0);

        ui.label(egui::RichText::new("Title").small().color(theme::TEXT_DIM));
        ui.add(
            egui::TextEdit::singleline(&mut inspector.annotation_title_buf)
                .hint_text("Untitled")
                .desired_width(f32::INFINITY),
        );

        ui.add_space(4.0);
        ui.label(egui::RichText::new("Body").small().color(theme::TEXT_DIM));
        ui.add(
            egui::TextEdit::multiline(&mut inspector.annotation_body_buf)
                .desired_rows(3)
                .desired_width(f32::INFINITY),
        );

        ui.add_space(4.0);
        ui.label(egui::RichText::new("Color").small().color(theme::TEXT_DIM));
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut inspector.annotation_color_buf)
                    .hint_text("#RRGGBB")
                    .desired_width(100.0),
            );
            // Interactive swatch: egui's own color picker, bound to the hex
            // buffer via round-trip parse/format so the stored value stays a
            // hex string per the gis.annotation.color contract (see
            // `fe-ui/src/AGENTS.md` §gis-query-ui).
            let mut rgb = parse_hex_color(&inspector.annotation_color_buf)
                .map(|(r, g, b)| [r, g, b])
                .unwrap_or([255, 255, 255]);
            if egui::widgets::color_picker::color_edit_button_srgb(ui, &mut rgb).changed() {
                inspector.annotation_color_buf = rgb_to_hex(rgb[0], rgb[1], rgb[2]);
            }
            if parse_hex_color(&inspector.annotation_color_buf).is_none()
                && !inspector.annotation_color_buf.trim().is_empty()
            {
                ui.label(
                    egui::RichText::new("invalid hex")
                        .small()
                        .color(theme::STATUS_OFFLINE),
                );
            }
        });

        ui.add_space(6.0);
        if ui
            .add(
                egui::Button::new("\u{1F4BE} Save Annotation")
                    .fill(theme::BG_SAVE)
                    .min_size(egui::vec2(ui.available_width(), 26.0)),
            )
            .clicked()
        {
            for action in annotation_save_actions(
                &node_id,
                &inspector.annotation_title_buf,
                &inspector.annotation_body_buf,
                &inspector.annotation_color_buf,
            ) {
                ui_mgr.push_action(action);
            }
        }
        ui.label(
            egui::RichText::new("Saving an empty field clears that annotation property.")
                .small()
                .color(theme::TEXT_MUTED)
                .italics(),
        );

        ui.add_space(4.0);
    });
}

/// Parses a `#RGB`/`#RRGGBB` hex color string into `(r, g, b)` bytes; `None`
/// when the string isn't a valid hex color (missing `#`, wrong length, or
/// non-hex digits).
pub(crate) fn parse_hex_color(s: &str) -> Option<(u8, u8, u8)> {
    let s = s.trim().strip_prefix('#')?;
    match s.len() {
        6 => {
            let r = u8::from_str_radix(&s[0..2], 16).ok()?;
            let g = u8::from_str_radix(&s[2..4], 16).ok()?;
            let b = u8::from_str_radix(&s[4..6], 16).ok()?;
            Some((r, g, b))
        }
        3 => {
            let expand = |c: char| c.to_digit(16).map(|d| (d * 16 + d) as u8);
            let mut chars = s.chars();
            let r = expand(chars.next()?)?;
            let g = expand(chars.next()?)?;
            let b = expand(chars.next()?)?;
            Some((r, g, b))
        }
        _ => None,
    }
}

/// Formats `(r, g, b)` bytes back into a lowercase `#rrggbb` hex string —
/// the inverse of `parse_hex_color`, used when the color picker swatch
/// changes.
pub(crate) fn rgb_to_hex(r: u8, g: u8, b: u8) -> String {
    format!("#{r:02x}{g:02x}{b:02x}")
}

/// Computes the Set/Delete actions for a "Save Annotation" click: one action
/// per field (title/body/color, in that order) — a non-empty (post-trim)
/// buffer becomes `SetNodeProperty` with the trimmed value, an empty buffer
/// becomes `DeleteNodeProperty`. Pure so the action-emission logic is
/// testable without an egui context — see `fe-ui/src/AGENTS.md`
/// §gis-query-ui.
pub(crate) fn annotation_save_actions(
    node_id: &str,
    title: &str,
    body: &str,
    color: &str,
) -> Vec<UiAction> {
    [
        (ANNOTATION_TITLE_KEY, title),
        (ANNOTATION_BODY_KEY, body),
        (ANNOTATION_COLOR_KEY, color),
    ]
    .into_iter()
    .map(|(key, buf)| {
        let trimmed = buf.trim();
        if trimmed.is_empty() {
            UiAction::DeleteNodeProperty {
                node_id: node_id.to_string(),
                key: key.to_string(),
            }
        } else {
            UiAction::SetNodeProperty {
                node_id: node_id.to_string(),
                key: key.to_string(),
                value: serde_json::Value::String(trimmed.to_string()),
            }
        }
    })
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_six_digit_hex() {
        assert_eq!(parse_hex_color("#ff0000"), Some((255, 0, 0)));
    }

    #[test]
    fn parses_three_digit_hex() {
        assert_eq!(parse_hex_color("#f00"), Some((255, 0, 0)));
    }

    #[test]
    fn parses_mixed_case_hex() {
        assert_eq!(parse_hex_color("#Ff8800"), Some((255, 136, 0)));
    }

    #[test]
    fn rejects_missing_hash() {
        assert_eq!(parse_hex_color("ff0000"), None);
    }

    #[test]
    fn rejects_bad_length() {
        assert_eq!(parse_hex_color("#ff00"), None);
    }

    #[test]
    fn rejects_non_hex_chars() {
        assert_eq!(parse_hex_color("#zzzzzz"), None);
    }

    #[test]
    fn trims_surrounding_whitespace() {
        assert_eq!(parse_hex_color("  #00ff00  "), Some((0, 255, 0)));
    }

    #[test]
    fn rgb_to_hex_round_trips_through_parse_hex_color() {
        let hex = rgb_to_hex(255, 136, 0);
        assert_eq!(hex, "#ff8800");
        assert_eq!(parse_hex_color(&hex), Some((255, 136, 0)));
    }

    #[test]
    fn save_actions_set_for_non_empty_fields() {
        let actions = annotation_save_actions("node-1", "Trailhead", "Start here", "#ff0000");
        assert_eq!(actions.len(), 3);
        match &actions[0] {
            UiAction::SetNodeProperty {
                node_id,
                key,
                value,
            } => {
                assert_eq!(node_id, "node-1");
                assert_eq!(key, ANNOTATION_TITLE_KEY);
                assert_eq!(value, &serde_json::json!("Trailhead"));
            }
            other => panic!("expected SetNodeProperty, got {other:?}"),
        }
    }

    #[test]
    fn save_actions_delete_for_empty_fields() {
        let actions = annotation_save_actions("node-1", "", "", "");
        assert_eq!(actions.len(), 3);
        for action in &actions {
            match action {
                UiAction::DeleteNodeProperty { node_id, .. } => assert_eq!(node_id, "node-1"),
                other => panic!("expected DeleteNodeProperty, got {other:?}"),
            }
        }
    }

    #[test]
    fn save_actions_trims_before_deciding_set_vs_delete() {
        let actions = annotation_save_actions("node-1", "  ", "Body", "  ");
        match &actions[0] {
            UiAction::DeleteNodeProperty { key, .. } => assert_eq!(key, ANNOTATION_TITLE_KEY),
            other => panic!("expected DeleteNodeProperty for whitespace-only title, got {other:?}"),
        }
        match &actions[1] {
            UiAction::SetNodeProperty { value, .. } => {
                assert_eq!(value, &serde_json::json!("Body"))
            }
            other => panic!("expected SetNodeProperty for body, got {other:?}"),
        }
    }

    #[test]
    fn save_actions_mixed_set_and_delete_are_independent() {
        // Title set, body/color empty — the historical bug: sibling Delete
        // results must not stomp the sibling Set buffer. This test only
        // covers action *emission*; see db_results.rs tests for the
        // buffer-stomping fix itself.
        let actions = annotation_save_actions("node-1", "Only Title", "", "");
        assert!(matches!(actions[0], UiAction::SetNodeProperty { .. }));
        assert!(matches!(actions[1], UiAction::DeleteNodeProperty { .. }));
        assert!(matches!(actions[2], UiAction::DeleteNodeProperty { .. }));
    }
}
