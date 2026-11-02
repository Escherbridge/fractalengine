//! Property table geometry and display formatting; see panels/AGENTS.md.

use bevy_egui::egui;
use serde_json::Value;

use crate::theme;

const COLUMN_GAP: f32 = 8.0;

pub(super) struct Columns {
    pub widths: [f32; 4],
    pub height: f32,
}

impl Columns {
    pub fn new(ui: &egui::Ui, available: f32) -> Self {
        let minimum = (20.0 / ui.ctx().pixels_per_point()).max(24.0);
        let font = egui::TextStyle::Button.resolve(ui.style());
        let text_width = |text: &str| {
            ui.fonts_mut(|fonts| {
                fonts
                    .layout_no_wrap(text.to_owned(), font.clone(), theme::TEXT_BRIGHT)
                    .size()
                    .x
            })
        };
        let button_width =
            |text: &str| (text_width(text) + ui.spacing().button_padding.x * 2.0).max(minimum);
        let actions = ["Edit", "Copy", "Confirm", "Cancel"]
            .map(button_width)
            .iter()
            .sum::<f32>()
            + COLUMN_GAP * 3.0;
        let kind = text_width("number").max(52.0) + 8.0;
        let width = available.max(96.0 + kind + 120.0 + actions + COLUMN_GAP * 3.0);
        let key_max = (width - kind - actions - COLUMN_GAP * 3.0 - 120.0).clamp(96.0, 180.0);
        let key = (width * 0.20).clamp(96.0, key_max);
        Self {
            widths: [
                key,
                kind,
                width - key - kind - actions - COLUMN_GAP * 3.0,
                actions,
            ],
            height: minimum.max(ui.spacing().interact_size.y),
        }
    }

    pub fn width(&self) -> f32 {
        self.widths.iter().sum::<f32>() + COLUMN_GAP * 3.0
    }

    pub fn row(&self, ui: &mut egui::Ui, body: impl FnOnce(&mut egui::Ui)) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = COLUMN_GAP;
            ui.spacing_mut().interact_size.y = self.height;
            body(ui);
        });
    }

    pub fn cell<R>(
        &self,
        ui: &mut egui::Ui,
        column: usize,
        body: impl FnOnce(&mut egui::Ui) -> R,
    ) -> R {
        let width = self.widths[column];
        ui.allocate_ui_with_layout(
            egui::vec2(width, self.height),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.set_min_size(egui::vec2(width, self.height));
                ui.set_max_width(width);
                body(ui)
            },
        )
        .inner
    }
}

pub(super) fn preview(value: &Value) -> String {
    match value {
        Value::Object(fields) => format!("{{{} fields}}", fields.len()),
        Value::Array(items) => format!("[{} items]", items.len()),
        Value::String(text) => {
            let mut preview = String::new();
            for character in text.chars().take(120) {
                match character {
                    '\n' => preview.push_str("\\n"),
                    '\r' => preview.push_str("\\r"),
                    '\t' => preview.push_str("\\t"),
                    other => preview.push(other),
                }
            }
            if text.chars().nth(120).is_some() {
                preview.push('…');
            }
            if preview.is_empty() {
                preview.push_str("(empty string)");
            }
            preview
        }
        other => other.to_string(),
    }
}

pub(super) fn clipboard_value(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}
