//! Shared hierarchy row and contextual routing; see panels/AGENTS.md.

use super::tree_appearance::{AppearanceDraft, EntryKind};
use crate::accessibility::ResponseExt;
use crate::actions::UiManager;
use crate::dialogs::{ActiveDialog, EntitySettingsType};
use crate::navigation_manager::NavigationManager;
use crate::plugin::LocalUserRole;
use crate::settings::AppSettings;
use crate::theme;
use bevy_egui::egui;

/// Presentation projected from the selection authority, independently of navigation.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RowSelection {
    #[default]
    None,
    Primary,
    Selected,
    Partial,
}

impl RowSelection {
    pub(crate) fn from_primary(selected: bool) -> Self {
        if selected {
            Self::Primary
        } else {
            Self::None
        }
    }

    fn is_selected(self) -> bool {
        matches!(self, Self::Primary | Self::Selected)
    }

    fn description(self) -> &'static str {
        match self {
            Self::None => "Not selected",
            Self::Primary => "Primary selection",
            Self::Selected => "Selected",
            Self::Partial => "Partially selected group",
        }
    }

    pub(crate) fn fill(self, on_path: bool) -> egui::Color32 {
        match self {
            Self::Primary => theme::TREE_PRIMARY_BG,
            Self::Selected => theme::TREE_SELECTED_BG,
            Self::Partial => theme::TREE_PARTIAL_BG,
            Self::None if on_path => theme::BG_BUTTON_ALT,
            Self::None => egui::Color32::TRANSPARENT,
        }
    }

    pub(crate) fn paint(self, painter: &egui::Painter, rect: egui::Rect) {
        let (width, color) = match self {
            Self::Primary => (3.0, theme::TREE_PRIMARY_ACCENT),
            Self::Selected => (1.5, theme::TREE_SELECTED_ACCENT),
            Self::Partial => (1.5, theme::TREE_SELECTED_ACCENT),
            Self::None => return,
        };
        let x = rect.left() + width * 0.5;
        let top = rect.top() + 3.0;
        let bottom = rect.bottom() - 3.0;
        if self == Self::Partial {
            let segment = (bottom - top) / 5.0;
            for index in [0, 2, 4] {
                let y = top + index as f32 * segment;
                painter.line_segment(
                    [egui::pos2(x, y), egui::pos2(x, y + segment)],
                    egui::Stroke::new(width, color),
                );
            }
        } else {
            painter.line_segment(
                [egui::pos2(x, top), egui::pos2(x, bottom)],
                egui::Stroke::new(width, color),
            );
        }
        if self == Self::Primary {
            painter.rect_stroke(
                rect,
                2.0,
                egui::Stroke::new(1.0, color),
                egui::StrokeKind::Inside,
            );
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct Target<'a> {
    pub kind: EntryKind,
    pub id: &'a str,
    pub name: &'a str,
    pub verse_id: &'a str,
    pub fractal_id: Option<&'a str>,
    pub petal_id: Option<&'a str>,
    pub url: Option<&'a str>,
}

#[derive(Clone, Copy)]
enum Command {
    CopyId,
    Appearance,
    Options,
}

impl Target<'_> {
    fn options(self) -> ActiveDialog {
        if self.kind == EntryKind::Node {
            return ActiveDialog::NodeOptions {
                node_id: self.id.into(),
                node_name_buf: self.name.into(),
                webpage_url_buf: self.url.unwrap_or_default().into(),
                pending_delete: false,
                descendant_count: None,
            };
        }
        crate::dialogs::entity_details::dialog(
            match self.kind {
                EntryKind::Verse => EntitySettingsType::Verse,
                EntryKind::Fractal => EntitySettingsType::Fractal,
                _ => EntitySettingsType::Petal,
            },
            self.id,
            self.name,
            self.verse_id,
            self.fractal_id,
        )
    }
}

fn menu(ui: &mut egui::Ui, target: Target<'_>, can_edit: bool, command: &mut Option<Command>) {
    crate::native_accessibility::surface::popup(ui.ctx());
    ui.spacing_mut().interact_size.y = crate::ui_shell::layout_policy::target_size(ui.ctx());
    ui.add(egui::Label::new(egui::RichText::new(target.name).strong()).truncate())
        .on_hover_text(target.name);
    if ui
        .button("Appearance…")
        .named(
            ui,
            format_args!("Appearance for {} {}", target.kind.label(), target.name),
        )
        .clicked()
    {
        *command = Some(Command::Appearance);
        ui.close();
    }
    if ui
        .button("Copy ID")
        .named(
            ui,
            format_args!("Copy ID for {} {}", target.kind.label(), target.name),
        )
        .clicked()
    {
        *command = Some(Command::CopyId);
        ui.close();
    }
    for label in ["Rename…", "Options…"] {
        if ui
            .add_enabled(can_edit, egui::Button::new(label))
            .named(
                ui,
                format_args!("{label} {} {}", target.kind.label(), target.name),
            )
            .on_disabled_hover_text(
                "Enter this entry's scope before editing it: Editor for nodes, Manager for hierarchy settings.",
            )
            .clicked()
        {
            *command = Some(Command::Options);
            ui.close();
        }
    }
    if ui
        .add_enabled(can_edit, egui::Button::new("Delete..."))
        .named(
            ui,
            format_args!("Delete {} {}", target.kind.label(), target.name),
        )
        .on_hover_text(
            "Open this entry's settings, then choose Delete and confirm removal of its contents.",
        )
        .on_disabled_hover_text(
            "Enter this entry's scope with the required edit role before deleting it.",
        )
        .clicked()
    {
        *command = Some(Command::Options);
        ui.close();
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn row(
    ui: &mut egui::Ui,
    target: Target<'_>,
    selection: RowSelection,
    on_path: bool,
    enabled: bool,
    navigation: &NavigationManager,
    role: &LocalUserRole,
    settings: &AppSettings,
    ui_mgr: &mut UiManager,
    eye: impl FnOnce(&mut egui::Ui) -> bool,
    mut extra_menu: impl FnMut(&mut egui::Ui),
) -> (egui::Response, bool) {
    let appearance = settings
        .tree_appearance
        .get(&target.kind.key(target.id))
        .copied()
        .unwrap_or_else(|| target.kind.default_appearance());
    let scope_matches = navigation.active_verse_id.as_deref() == Some(target.verse_id)
        && match target.kind {
            EntryKind::Verse => {
                navigation.active_fractal_id.is_none() && navigation.active_petal_id.is_none()
            }
            EntryKind::Fractal => {
                navigation.active_fractal_id.as_deref() == Some(target.id)
                    && navigation.active_petal_id.is_none()
            }
            EntryKind::Petal => {
                navigation.active_fractal_id.as_deref() == target.fractal_id
                    && navigation.active_petal_id.as_deref() == Some(target.id)
            }
            EntryKind::Node => {
                navigation.active_fractal_id.as_deref() == target.fractal_id
                    && navigation.active_petal_id.as_deref() == target.petal_id
            }
        };
    let can_edit = scope_matches
        && if target.kind == EntryKind::Node {
            role.can_edit()
        } else {
            role.can_manage()
        };
    let mut command = None;
    let mut eye_clicked = false;
    let selected = selection.is_selected();
    let inner = ui
        .push_id(("tree_entry", target.kind.label(), target.id), |ui| {
            let frame = egui::Frame::NONE
                .fill(selection.fill(on_path))
                .inner_margin(egui::Margin::symmetric(2, 1))
                .show(ui, |ui| {
                    let target_size = crate::ui_shell::layout_policy::target_size(ui.ctx());
                    let gap = 2.0;
                    let row_width = ui.available_width().max(4.0 * target_size + 3.0 * gap);
                    let row_layout = egui::Layout::left_to_right(egui::Align::Center);
                    ui.allocate_ui_with_layout(egui::vec2(row_width, target_size), row_layout, |ui| {
                        ui.spacing_mut().item_spacing.x = gap;
                        ui.spacing_mut().interact_size = egui::vec2(target_size, target_size);
                        let name_width = row_width - target_size * 3.0 - 3.0 * gap;
                        let (icon_rect, _) = ui.allocate_exact_size(
                            egui::vec2(target_size, target_size),
                            egui::Sense::hover(),
                        );
                        let glyph_size = egui::TextStyle::Button
                            .resolve(ui.style())
                            .size
                            .min(icon_rect.width())
                            .min(icon_rect.height());
                        appearance.icon.paint(
                            ui.painter(),
                            egui::Rect::from_center_size(
                                icon_rect.center(),
                                egui::Vec2::splat(glyph_size),
                            ),
                            appearance.color32(),
                        );
                        let mut text = egui::RichText::new(target.name).color(if !enabled {
                            theme::TEXT_MUTED
                        } else if selected {
                            theme::TEXT_BRIGHT
                        } else {
                            theme::TEXT_SECTION
                        });
                        if selected {
                            text = text.strong();
                        }
                        let response = ui
                            .add_sized(
                                [name_width, target_size],
                                egui::Button::new(text)
                                    .min_size(egui::Vec2::splat(target_size))
                                    .selected(selected)
                                    .frame(false)
                                    .sense(egui::Sense::click())
                                    .truncate(),
                            )
                            .named(
                                ui,
                                format_args!(
                                    "{} {} {}",
                                    if target.kind == EntryKind::Node {
                                        "Select"
                                    } else {
                                        "Open"
                                    },
                                    target.kind.label(),
                                    target.name
                                ),
                            )
                            .on_hover_text(target.name);
                        ui.ctx().accesskit_node_builder(response.id, |node| {
                            node.set_selected(selected);
                            node.set_description(selection.description());
                        });
                        response.context_menu(|ui| {
                            menu(ui, target, can_edit, &mut command);
                            extra_menu(ui);
                        });
                        ui.allocate_ui_with_layout(
                            egui::vec2(target_size, target_size),
                            egui::Layout::left_to_right(egui::Align::Center),
                            |ui| {
                                eye_clicked = eye(ui);
                            },
                        );
                        ui.allocate_ui_with_layout(
                            egui::vec2(target_size, target_size),
                            egui::Layout::left_to_right(egui::Align::Center),
                            |ui| {
                                let (response, _) =
                                    egui::containers::menu::MenuButton::from_button(
                                        egui::Button::new("\u{2026}")
                                            .min_size(egui::Vec2::splat(target_size)),
                                    )
                                    .ui(ui, |ui| {
                                        menu(ui, target, can_edit, &mut command);
                                        extra_menu(ui);
                                    });
                                response.named(
                                    ui,
                                    format_args!(
                                        "Options menu for {} {}",
                                        target.kind.label(),
                                        target.name
                                    ),
                                );
                            },
                        );
                        response
                    })
                    .inner
                });
            selection.paint(ui.painter(), frame.response.rect);
            frame.inner
        })
        .inner;
    match command {
        Some(Command::CopyId) => {
            super::widgets::copy_with_feedback(ui, ui_mgr, target.id, "Copied entry ID")
        }
        Some(Command::Appearance) => ui_mgr.open_dialog(ActiveDialog::TreeAppearance {
            draft: AppearanceDraft::new(target.kind, target.id, target.name, settings),
        }),
        Some(Command::Options) => ui_mgr.open_dialog(target.options()),
        None => {}
    }
    (inner, eye_clicked)
}
