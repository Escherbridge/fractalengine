//! Property table geometry and display formatting; see panels/AGENTS.md.

use bevy_egui::egui;
use serde_json::Value;

use crate::theme;

const COLUMN_GAP: f32 = 8.0;

pub(super) struct Columns {
    pub widths: [f32; 4],
    pub height: f32,
    action_widths: [f32; 3],
}

impl Columns {
    pub fn new(ui: &egui::Ui, available: f32) -> Self {
        let minimum = (20.0 / ui.ctx().pixels_per_point()).max(24.0);
        let height = minimum.max(ui.spacing().interact_size.y);
        let font = egui::TextStyle::Button.resolve(ui.style());
        let text_width = |text: &str| {
            ui.fonts_mut(|fonts| {
                fonts
                    .layout_no_wrap(text.to_owned(), font.clone(), theme::TEXT_BRIGHT)
                    .size()
                    .x
            })
        };
        let button_width = |text: &str| {
            (text_width(text) + ui.spacing().button_padding.x * 2.0)
                .max(height)
                .ceil()
        };
        let action_widths = [
            button_width("Edit"),
            button_width("Copy"),
            button_width("Delete")
                .max(button_width("Confirm") + COLUMN_GAP + button_width("Cancel")),
        ];
        let actions = action_widths.iter().sum::<f32>() + COLUMN_GAP * 2.0;
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
            height,
            action_widths,
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
        fixed_cell(
            ui,
            ("property-column", column),
            egui::vec2(self.widths[column], self.height),
            egui::Layout::left_to_right(egui::Align::Center),
            body,
        )
    }

    pub fn action<R>(
        &self,
        ui: &mut egui::Ui,
        slot: usize,
        body: impl FnOnce(&mut egui::Ui) -> R,
    ) -> R {
        fixed_cell(
            ui,
            ("property-action", slot),
            egui::vec2(self.action_widths[slot], self.height),
            egui::Layout::top_down(egui::Align::Min),
            body,
        )
    }
}

fn fixed_cell<R>(
    ui: &mut egui::Ui,
    id: impl std::hash::Hash,
    size: egui::Vec2,
    layout: egui::Layout,
    body: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .id_salt(id)
            .max_rect(rect)
            .layout(layout),
    );
    child.set_clip_rect(rect.intersect(ui.clip_rect()));
    body(&mut child)
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
