//! Shared hierarchy row and contextual routing; see panels/AGENTS.md.

use super::tree_appearance::{AppearanceDraft, EntryKind};
use crate::accessibility::ResponseExt;
use crate::actions::UiManager;
use crate::dialogs::{ActiveDialog, EntitySettingsType, SettingsTab};
use crate::plugin::LocalUserRole;
use crate::settings::AppSettings;
use crate::theme;
use bevy_egui::egui;

#[derive(Clone, Copy)]
pub(crate) struct Target<'a> {
    pub kind: EntryKind,
    pub id: &'a str,
    pub name: &'a str,
    pub verse_id: &'a str,
    pub fractal_id: Option<&'a str>,
    pub url: Option<&'a str>,
}

#[derive(Clone, Copy)]
enum Command {
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
        ActiveDialog::EntitySettings {
            entity_type: match self.kind {
                EntryKind::Verse => EntitySettingsType::Verse,
                EntryKind::Fractal => EntitySettingsType::Fractal,
                _ => EntitySettingsType::Petal,
            },
            entity_id: self.id.into(),
            entity_name: self.name.into(),
            parent_verse_id: self.verse_id.into(),
            parent_fractal_id: self.fractal_id.map(str::to_owned),
            active_tab: SettingsTab::General,
            name_buf: self.name.into(),
            default_access_buf: None,
            description_buf: None,
            peer_roles: vec![],
            roles_loading: false,
            invite_role_buf: "viewer".into(),
            invite_expiry_buf: 24,
            generated_invite_link: None,
            pending_delete: false,
            pending_claim_ownership: false,
            api_tokens: vec![],
            api_tokens_loading: false,
            api_token_scope_buf: String::new(),
            api_token_role_buf: "viewer".into(),
            api_token_expiry_buf: 24,
            api_token_client_buf: String::new(),
            generated_api_token: None,
            generated_api_token_client: None,
            scoped_api_tokens: vec![],
            scoped_tokens_loading: false,
        }
    }
}

fn menu(ui: &mut egui::Ui, target: Target<'_>, can_edit: bool, command: &mut Option<Command>) {
    ui.spacing_mut().interact_size.y = 24.0_f32.max(20.0 / ui.ctx().pixels_per_point());
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
        ui.ctx().copy_text(target.id.to_owned());
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
                "Open this entry's verse with an Editor role (Manager for hierarchy settings).",
            )
            .clicked()
        {
            *command = Some(Command::Options);
            ui.close();
        }
    }
    if target.kind == EntryKind::Node {
        if ui
            .add_enabled(can_edit, egui::Button::new("Delete…"))
            .named(ui, format_args!("Delete node {}", target.name))
            .on_hover_text("Open Node Options, then Delete Node and confirm the subtree deletion.")
            .on_disabled_hover_text("Editor role in this entry's verse is required.")
            .clicked()
        {
            *command = Some(Command::Options);
            ui.close();
        }
    } else {
        ui.add_enabled(false, egui::Button::new("Delete…")).named(ui, format_args!("Delete {} {} unavailable", target.kind.label(), target.name))
            .on_disabled_hover_text("Deleting a whole hierarchy branch is not supported yet. Individual nodes can be deleted from their options.");
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn row(
    ui: &mut egui::Ui,
    target: Target<'_>,
    selected: bool,
    on_path: bool,
    enabled: bool,
    active_verse: Option<&str>,
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
    let can_edit = active_verse == Some(target.verse_id)
        && if target.kind == EntryKind::Node {
            role.can_edit()
        } else {
            role.can_manage()
        };
    let mut command = None;
    let mut eye_clicked = false;
    let fill = if selected {
        theme::TREE_SELECTED_BG
    } else if on_path {
        theme::BG_BUTTON_ALT
    } else {
        egui::Color32::TRANSPARENT
    };
    let inner = ui
        .push_id(("tree_entry", target.kind.label(), target.id), |ui| {
            egui::Frame::NONE
                .fill(fill)
                .inner_margin(egui::Margin::symmetric(2, 1))
                .show(ui, |ui| {
                    ui.set_min_height(26.0_f32.max(20.0 / ui.ctx().pixels_per_point()));
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 2.0;
                        let target_size = 24.0_f32.max(20.0 / ui.ctx().pixels_per_point());
                        ui.spacing_mut().interact_size = egui::vec2(target_size, target_size);
                        let name_width = (ui.available_width() - target_size * 3.0 - 8.0).max(8.0);
                        let (icon_rect, _) = ui.allocate_exact_size(
                            egui::vec2(target_size, target_size),
                            egui::Sense::hover(),
                        );
                        appearance
                            .icon
                            .paint(ui.painter(), icon_rect, appearance.color32());
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
                                ui.menu_button("…", |ui| {
                                    menu(ui, target, can_edit, &mut command);
                                    extra_menu(ui);
                                })
                                .response
                                .named(
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
                })
                .inner
        })
        .inner;
    if let Some(command) = command {
        let dialog = match command {
            Command::Appearance => ActiveDialog::TreeAppearance {
                draft: AppearanceDraft::new(target.kind, target.id, target.name, settings),
            },
            Command::Options => target.options(),
        };
        ui_mgr.open_dialog(dialog);
    }
    (inner, eye_clicked)
}
