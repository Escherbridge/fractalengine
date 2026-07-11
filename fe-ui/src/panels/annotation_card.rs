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
    let Some(sel) = node_mgr.selected.as_ref() else { return };
    let node_id = sel.node_id.clone();

    egui::CollapsingHeader::new(
        egui::RichText::new("Annotation").strong().color(theme::TEXT_SECTION),
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
        ui.label(egui::RichText::new("Color (hex)").small().color(theme::TEXT_DIM));
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut inspector.annotation_color_buf)
                    .hint_text("#RRGGBB")
                    .desired_width(100.0),
            );
            match parse_hex_color(&inspector.annotation_color_buf) {
                Some((r, g, b)) => {
                    let (rect, _) =
                        ui.allocate_exact_size(egui::vec2(20.0, 20.0), egui::Sense::hover());
                    ui.painter().rect_filled(rect, 2.0, egui::Color32::from_rgb(r, g, b));
                }
                None if !inspector.annotation_color_buf.trim().is_empty() => {
                    ui.label(
                        egui::RichText::new("invalid hex")
                            .small()
                            .color(theme::STATUS_OFFLINE),
                    );
                }
                None => {}
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
            for (key, buf) in [
                (ANNOTATION_TITLE_KEY, inspector.annotation_title_buf.clone()),
                (ANNOTATION_BODY_KEY, inspector.annotation_body_buf.clone()),
                (ANNOTATION_COLOR_KEY, inspector.annotation_color_buf.clone()),
            ] {
                let trimmed = buf.trim();
                if trimmed.is_empty() {
                    ui_mgr.push_action(UiAction::DeleteNodeProperty {
                        node_id: node_id.clone(),
                        key: key.to_string(),
                    });
                } else {
                    ui_mgr.push_action(UiAction::SetNodeProperty {
                        node_id: node_id.clone(),
                        key: key.to_string(),
                        value: serde_json::Value::String(trimmed.to_string()),
                    });
                }
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
}
