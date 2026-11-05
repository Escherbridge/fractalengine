//! Property table geometry and display formatting; see panels/AGENTS.md.

use bevy_egui::egui;
use serde_json::Value;

use crate::theme;

const COLUMN_GAP: f32 = 8.0;

pub(super) struct Columns {
    pub widths: [f32; 4],
    pub height: f32,
    compact: bool,
    width: f32,
    action_widths: [f32; 3],
    viewport: egui::Rangef,
}

impl Columns {
    pub fn new(ui: &egui::Ui, available: f32) -> Self {
        let minimum = crate::ui_shell::layout_policy::target_size(ui.ctx());
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
                .max(super::widgets::confirmation_pair_width(ui, "Confirm", COLUMN_GAP)),
        ];
        let actions = action_widths.iter().sum::<f32>() + COLUMN_GAP * 2.0;
        let kind = text_width("number").max(52.0) + 8.0;
        let width = available.max(1.0);
        let compact = width < 96.0 + kind + 120.0 + actions + COLUMN_GAP * 3.0;
        let widths = if compact {
            let key = (width * 0.38).min((width - COLUMN_GAP - height).max(0.0));
            [key, 0.0, (width - key - COLUMN_GAP).max(0.0), 0.0]
        } else {
            let key_max = (width - kind - actions - COLUMN_GAP * 3.0 - 120.0).clamp(96.0, 180.0);
            let key = (width * 0.20).clamp(96.0, key_max);
            [
                key,
                kind,
                width - key - kind - actions - COLUMN_GAP * 3.0,
                actions,
            ]
        };
        Self {
            widths,
            height,
            compact,
            width,
            action_widths,
            viewport: ui
                .available_rect_before_wrap()
                .x_range()
                .intersection(ui.clip_rect().x_range()),
        }
    }

    pub fn width(&self) -> f32 {
        self.width
    }

    pub fn compact(&self) -> bool {
        self.compact
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

    pub fn content_cell<R>(
        &self,
        ui: &mut egui::Ui,
        column: usize,
        body: impl FnOnce(&mut egui::Ui) -> R,
    ) -> R {
        fixed_cell(
            ui,
            ("property-column", column),
            egui::vec2(
                self.widths[column],
                self.height * if self.compact { 2.0 } else { 1.0 },
            ),
            if self.compact && column == 0 {
                egui::Layout::top_down(egui::Align::Min)
            } else {
                egui::Layout::left_to_right(egui::Align::Center)
            },
            body,
        )
    }

    pub fn actions_row(&self, ui: &mut egui::Ui, body: impl FnOnce(&mut egui::Ui)) {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = COLUMN_GAP;
            ui.spacing_mut().interact_size.y = self.height;
            body(ui);
        });
    }

    pub fn action<R>(
        &self,
        ui: &mut egui::Ui,
        slot: usize,
        body: impl FnOnce(&mut egui::Ui) -> R,
    ) -> R {
        if self.compact && self.action_widths[slot] > self.width {
            return ui
                .allocate_ui_with_layout(
                    egui::vec2(self.width, self.height),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.set_width(self.width);
                        body(ui)
                    },
                )
                .inner;
        }
        fixed_cell(
            ui,
            ("property-action", slot),
            egui::vec2(self.action_widths[slot], self.height),
            egui::Layout::top_down(egui::Align::Min),
            body,
        )
    }

    pub fn expanded<R>(&self, ui: &mut egui::Ui, body: impl FnOnce(&mut egui::Ui) -> R) -> R {
        let width = (self.viewport.span() - 2.0).max(1.0);
        let rect = egui::Rect::from_min_size(
            egui::pos2(self.viewport.min + 1.0, ui.next_widget_position().y),
            egui::vec2(width, ui.ctx().viewport_rect().height()),
        );
        ui.scope_builder(
            egui::UiBuilder::new()
                .id_salt("property-expansion")
                .max_rect(rect)
                .layout(egui::Layout::top_down(egui::Align::Min)),
            |ui| {
                ui.set_width(width);
                body(ui)
            },
        )
        .inner
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
