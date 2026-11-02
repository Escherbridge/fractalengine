//! Per-entry display preferences and appearance editor; see panels/AGENTS.md.

use bevy_egui::egui::{self, ecolor::Hsva};

use crate::accessibility::ResponseExt;
use crate::actions::UiManager;
use crate::dialogs::ActiveDialog;
use crate::settings::{AppSettings, EntryAppearance, TreeIcon};
use crate::theme;
use crate::verse_manager::VerseManager;

impl TreeIcon {
    const ALL: [(Self, &'static str); 8] = [
        (Self::Circle, "Circle"),
        (Self::Diamond, "Diamond"),
        (Self::Square, "Square"),
        (Self::Triangle, "Triangle"),
        (Self::Star, "Star"),
        (Self::Hexagon, "Hexagon"),
        (Self::Cross, "Cross"),
        (Self::Ring, "Ring"),
    ];

    pub(crate) fn paint(self, painter: &egui::Painter, rect: egui::Rect, color: egui::Color32) {
        let center = rect.center();
        let radius = rect.width().min(rect.height()) * 0.32;
        let stroke = egui::Stroke::new(1.5, color);
        let outline = egui::Stroke::new(3.5, theme::TEXT_MUTED);
        match self {
            Self::Circle => {
                painter.circle_stroke(center, radius + 1.0, outline);
                painter.circle_filled(center, radius, color);
            }
            Self::Ring => {
                painter.circle_stroke(center, radius, outline);
                painter.circle_stroke(center, radius, stroke);
            }
            Self::Square => {
                painter.rect_stroke(
                    egui::Rect::from_center_size(center, egui::vec2(radius * 1.6, radius * 1.6)),
                    1.0,
                    outline,
                    egui::StrokeKind::Outside,
                );
                painter.rect_filled(
                    egui::Rect::from_center_size(center, egui::vec2(radius * 1.6, radius * 1.6)),
                    1.0,
                    color,
                );
            }
            Self::Cross => {
                for angle in [0.0_f32, std::f32::consts::FRAC_PI_2] {
                    let arm = egui::vec2(angle.cos(), angle.sin()) * radius;
                    painter.line_segment(
                        [center - arm, center + arm],
                        egui::Stroke::new(5.0, theme::TEXT_MUTED),
                    );
                    painter
                        .line_segment([center - arm, center + arm], egui::Stroke::new(3.0, color));
                }
            }
            _ => {
                let (sides, start) = match self {
                    Self::Diamond => (4, -std::f32::consts::FRAC_PI_2),
                    Self::Triangle => (3, -std::f32::consts::FRAC_PI_2),
                    Self::Star => (10, -std::f32::consts::FRAC_PI_2),
                    _ => (6, 0.0),
                };
                let point = |i: usize| {
                    let angle = start + i as f32 * std::f32::consts::TAU / sides as f32;
                    let r = if self == Self::Star && i % 2 == 1 {
                        radius * 0.45
                    } else {
                        radius
                    };
                    center + egui::vec2(angle.cos(), angle.sin()) * r
                };
                for i in 0..sides {
                    let line = [point(i), point((i + 1) % sides)];
                    painter.line_segment(line, outline);
                    painter.line_segment(line, stroke);
                }
            }
        }
    }
}

impl EntryAppearance {
    pub(crate) fn color32(self) -> egui::Color32 {
        egui::Color32::from_rgb(self.color[0], self.color[1], self.color[2])
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    Verse,
    Fractal,
    Petal,
    Node,
}

impl EntryKind {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Verse => "verse",
            Self::Fractal => "fractal",
            Self::Petal => "petal",
            Self::Node => "node",
        }
    }

    pub(crate) fn key(self, id: &str) -> String {
        format!("{}:{id}", self.label())
    }

    pub(crate) fn default_appearance(self) -> EntryAppearance {
        EntryAppearance {
            icon: match self {
                Self::Verse => TreeIcon::Ring,
                Self::Fractal => TreeIcon::Hexagon,
                Self::Petal => TreeIcon::Diamond,
                Self::Node => TreeIcon::Circle,
            },
            color: [
                theme::TREE_NODE_ICON.r(),
                theme::TREE_NODE_ICON.g(),
                theme::TREE_NODE_ICON.b(),
            ],
        }
    }

    pub(crate) fn exists(self, id: &str, hierarchy: &VerseManager) -> bool {
        match self {
            Self::Verse => hierarchy.verses.iter().any(|v| v.id == id),
            Self::Fractal => hierarchy
                .verses
                .iter()
                .flat_map(|v| &v.fractals)
                .any(|f| f.id == id),
            Self::Petal => hierarchy.find_petal(id).is_some(),
            Self::Node => hierarchy.all_nodes().any(|n| n.id == id),
        }
    }
}

#[derive(Debug, Clone)]
pub struct AppearanceDraft {
    pub kind: EntryKind,
    pub id: String,
    pub name: String,
    appearance: EntryAppearance,
    hsva: Hsva,
    hex: String,
}

impl AppearanceDraft {
    pub(crate) fn new(kind: EntryKind, id: &str, name: &str, settings: &AppSettings) -> Self {
        let appearance = settings
            .tree_appearance
            .get(&kind.key(id))
            .copied()
            .unwrap_or_else(|| kind.default_appearance());
        Self {
            kind,
            id: id.into(),
            name: name.into(),
            hsva: Hsva::from(appearance.color32()),
            hex: format_hex(appearance.color),
            appearance,
        }
    }
}

pub(crate) fn format_hex(rgb: [u8; 3]) -> String {
    format!("#{:02X}{:02X}{:02X}", rgb[0], rgb[1], rgb[2])
}

pub(crate) fn parse_hex(raw: &str) -> Option<[u8; 3]> {
    let hex = raw.strip_prefix('#')?;
    if hex.len() != 6 || !hex.is_ascii() {
        return None;
    }
    Some([
        u8::from_str_radix(&hex[0..2], 16).ok()?,
        u8::from_str_radix(&hex[2..4], 16).ok()?,
        u8::from_str_radix(&hex[4..6], 16).ok()?,
    ])
}

fn wheel(ui: &mut egui::Ui, hsva: &mut Hsva) -> bool {
    let size = ui.available_width().clamp(64.0, 180.0);
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::click_and_drag());
    let response = response.named(ui, "Appearance hue and saturation color wheel");
    let radius = size * 0.46;
    let center = rect.center();
    let mut mesh = egui::Mesh::default();
    const SEGMENTS: u32 = 64;
    const RINGS: u32 = 8;
    for ring in 0..=RINGS {
        let saturation = ring as f32 / RINGS as f32;
        for segment in 0..=SEGMENTS {
            let hue = segment as f32 / SEGMENTS as f32;
            let angle = hue * std::f32::consts::TAU;
            mesh.colored_vertex(
                center + egui::vec2(angle.cos(), angle.sin()) * radius * saturation,
                egui::Color32::from(Hsva::new(hue, saturation, hsva.v, 1.0)),
            );
        }
    }
    for ring in 0..RINGS {
        for segment in 0..SEGMENTS {
            let i = ring * (SEGMENTS + 1) + segment;
            mesh.add_triangle(i, i + 1, i + SEGMENTS + 1);
            mesh.add_triangle(i + 1, i + SEGMENTS + 2, i + SEGMENTS + 1);
        }
    }
    ui.painter().add(egui::Shape::mesh(mesh));
    let mut changed = false;
    if let Some(pos) = response
        .interact_pointer_pos()
        .filter(|_| response.clicked() || response.dragged())
    {
        let delta = pos - center;
        hsva.h = delta.y.atan2(delta.x).rem_euclid(std::f32::consts::TAU) / std::f32::consts::TAU;
        hsva.s = (delta.length() / radius).clamp(0.0, 1.0);
        changed = true;
    }
    let angle = hsva.h * std::f32::consts::TAU;
    let marker = center + egui::vec2(angle.cos(), angle.sin()) * hsva.s * radius;
    ui.painter()
        .circle_stroke(marker, 5.0, egui::Stroke::new(3.0, egui::Color32::BLACK));
    ui.painter()
        .circle_stroke(marker, 5.0, egui::Stroke::new(1.5, egui::Color32::WHITE));
    changed
}

pub(crate) fn render(
    ctx: &egui::Context,
    ui_mgr: &mut UiManager,
    settings: &mut AppSettings,
    hierarchy: &VerseManager,
) {
    let ActiveDialog::TreeAppearance { draft } = &mut ui_mgr.active_dialog else {
        return;
    };
    let exists = draft.kind.exists(&draft.id, hierarchy);
    let mut open = true;
    let mut save = false;
    let mut cancel = false;
    let mut reset = false;
    crate::ui_shell::layout_policy::bounded_window(ctx, format!("Appearance — {}", draft.name))
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .default_width(460.0)
        .show(ctx, |ui| {
            ui.spacing_mut().interact_size.y = 24.0_f32.max(20.0 / ctx.pixels_per_point());
            ui.label("Display preference on this device");
            ui.horizontal(|ui| {
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(32.0, 32.0), egui::Sense::hover());
                draft
                    .appearance
                    .icon
                    .paint(ui.painter(), rect, draft.appearance.color32());
                ui.label(egui::RichText::new(&draft.name).strong());
            });
            ui.separator();
            let color_width = (ui.available_width() * 0.53).max(100.0);
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    ui.set_width(color_width);
                    ui.set_max_width(color_width);
                    ui.spacing_mut().slider_width = (color_width - 72.0).max(36.0);
                    ui.label("Color");
                    let mut changed = wheel(ui, &mut draft.hsva);
                    changed |= ui
                        .add(egui::Slider::new(&mut draft.hsva.v, 0.0..=1.0).text("Brightness"))
                        .named(ui, "Appearance color brightness")
                        .changed();
                    // Numeric alternatives keep the wheel keyboard-editable.
                    changed |= ui
                        .add(egui::Slider::new(&mut draft.hsva.h, 0.0..=1.0).text("Hue"))
                        .named(ui, "Appearance color hue")
                        .changed();
                    changed |= ui
                        .add(egui::Slider::new(&mut draft.hsva.s, 0.0..=1.0).text("Saturation"))
                        .named(ui, "Appearance color saturation")
                        .changed();
                    if changed {
                        let [r, g, b, _] = egui::Color32::from(draft.hsva).to_array();
                        draft.appearance.color = [r, g, b];
                        draft.hex = format_hex(draft.appearance.color);
                    }
                    let label = ui.label("Hex color (#RRGGBB)");
                    if ui
                        .add(
                            egui::TextEdit::singleline(&mut draft.hex)
                                .desired_width(color_width)
                                .char_limit(7),
                        )
                        .labelled(ui, &label)
                        .changed()
                    {
                        if let Some(rgb) = parse_hex(&draft.hex) {
                            draft.appearance.color = rgb;
                            draft.hsva = Hsva::from(draft.appearance.color32());
                        }
                    }
                    if parse_hex(&draft.hex).is_none() {
                        ui.colored_label(
                            theme::STATUS_OFFLINE,
                            "Enter # and six hexadecimal digits.",
                        );
                    }
                });
                ui.separator();
                ui.vertical(|ui| {
                    ui.label("Icon palette");
                    egui::Grid::new("tree_icon_palette")
                        .num_columns(2)
                        .spacing([4.0, 4.0])
                        .show(ui, |ui| {
                            for (index, (icon, label)) in TreeIcon::ALL.into_iter().enumerate() {
                                let response = ui
                                    .add(
                                        egui::Button::new("")
                                            .min_size(egui::vec2(40.0, 36.0))
                                            .selected(icon == draft.appearance.icon),
                                    )
                                    .named(ui, format_args!("Choose {label} appearance icon"))
                                    .on_hover_text(label);
                                icon.paint(ui.painter(), response.rect, draft.appearance.color32());
                                if response.clicked() {
                                    draft.appearance.icon = icon;
                                }
                                if index % 2 == 1 {
                                    ui.end_row();
                                }
                            }
                        });
                });
            });
            ui.separator();
            if !exists {
                ui.colored_label(
                    theme::STATUS_OFFLINE,
                    "This entry no longer exists. Close this editor.",
                );
            }
            ui.horizontal_wrapped(|ui| {
                save = ui
                    .add_enabled(
                        exists && parse_hex(&draft.hex).is_some(),
                        egui::Button::new("Save").min_size(egui::vec2(64.0, 28.0)),
                    )
                    .named(ui, "Save tree appearance")
                    .clicked();
                cancel = ui
                    .add(egui::Button::new("Cancel").min_size(egui::vec2(64.0, 28.0)))
                    .named(ui, "Cancel tree appearance")
                    .clicked();
                reset = ui
                    .add_enabled(
                        exists,
                        egui::Button::new("Reset to default").min_size(egui::vec2(0.0, 28.0)),
                    )
                    .named(ui, "Reset tree appearance draft to default")
                    .clicked();
            });
        });
    if reset {
        let appearance = draft.kind.default_appearance();
        draft.appearance = appearance;
        draft.hsva = Hsva::from(appearance.color32());
        draft.hex = format_hex(appearance.color);
    }
    if save {
        let key = draft.kind.key(&draft.id);
        if draft.appearance == draft.kind.default_appearance() {
            settings.tree_appearance.remove(&key);
        } else {
            settings.tree_appearance.insert(key, draft.appearance);
        }
    }
    if save || cancel || !open || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        ui_mgr.close_dialog();
    }
}
