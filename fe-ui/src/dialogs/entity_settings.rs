//! Entity Settings dialog: General / Access / API Access tabs for Verse,
//! Fractal, and Petal entities.

use crate::ui_shell::layout_policy::WindowTargets;
use bevy_egui::egui;

use crate::accessibility::ResponseExt;

use super::{ActiveDialog, EntitySettingsType, SettingsTab};
use crate::actions::UiManager;
use crate::theme;
use fe_runtime::messages::{DbCommand, EntityType};

pub fn render_entity_settings_dialog(
    ctx: &egui::Context,
    ui_mgr: &mut UiManager,
    db_tx: &crossbeam::channel::Sender<DbCommand>,
    _local_role: &crate::plugin::LocalUserRole,
    known_peers: Option<&fe_runtime::PeerRegistry>,
    api_endpoint: Option<std::net::SocketAddr>,
) {
    super::entity_details::request_if_needed(ui_mgr, db_tx, ctx.input(|input| input.time));
    super::entity_access::service(ui_mgr, db_tx, known_peers);
    // Early-return guard: only render when EntitySettings is active.
    let ActiveDialog::EntitySettings {
        ref entity_type,
        ref entity_id,
        ref entity_name,
        ref mut details,
        ref mut active_tab,
        ref mut name_buf,
        ref mut default_access_buf,
        ref mut description_buf,
        ref mut access,
        ref mut invite_role_buf,
        ref mut invite_expiry_buf,
        ref mut pending_delete,
        ref mut pending_claim_ownership,
        ref mut api_token_role_buf,
        ref mut api_token_expiry_buf,
        ref mut api_token_client_buf,
        ..
    } = ui_mgr.active_dialog
    else {
        return;
    };

    let type_label = match entity_type {
        EntitySettingsType::Verse => "Verse",
        EntitySettingsType::Fractal => "Fractal",
        EntitySettingsType::Petal => "Petal",
    };
    let title = format!("{} Settings — {}", type_label, entity_name);
    let current_tab = *active_tab;
    let can_write = details.can_write();
    let can_edit_draft = details
        .loaded
        .as_ref()
        .is_some_and(|stored| fe_database::RoleLevel::from(stored.role.as_str()).can_manage());

    let mut close = false;
    let mut still_open = true;
    let mut send_failure = None;
    let mut access_action = None;
    let mut copied_access = false;

    crate::ui_shell::layout_policy::bounded_window(ctx, title)
        .open(&mut still_open)
        .collapsible(false)
        .resizable(true)
        .default_width(500.0)
        .min_width(crate::ui_shell::layout_policy::dialog_width(ctx,400.0))
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .frame(
            egui::Frame::NONE
                .fill(theme::BG_DIALOG)
                .inner_margin(egui::Margin::same(16))
                .corner_radius(6.0)
                .stroke(egui::Stroke::new(1.0_f32, theme::TEXT_DIM)),
        )
        .show_with_targets(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                if details.loading() { ui.spinner(); ui.label("Loading stored settings..."); }
                if ui.button(if details.error.is_some() { "Retry loading settings" } else { "Refresh stored settings" }).named(ui, format_args!("Refresh settings for {entity_name}")).clicked() { details.refresh(); }
                if details.loaded.is_some() && ui.button("Discard local draft").named(ui, format_args!("Discard settings draft for {entity_name}")).clicked() {
                    details.restore(name_buf, default_access_buf, description_buf);
                }
            });
            if let Some(error) = &details.error { ui.colored_label(theme::STATUS_OFFLINE, error); }
            if details.conflict { ui.colored_label(theme::STATUS_OFFLINE, "Stored settings changed. Your draft is retained; discard it before saving a replacement."); }
            if let Some(stored) = &details.loaded {
                ui.label(format!("Stored role at this entry: {}", stored.role));
                if !can_edit_draft { ui.label("Manager role at this entry is required to edit hierarchy settings."); }
            }
            // Tab bar
            ui.horizontal(|ui| {
                let gen_fill = if current_tab == SettingsTab::General {
                    theme::BG_TAB_ACTIVE
                } else {
                    theme::BG_TAB_INACTIVE
                };
                if ui
                    .add(
                        egui::Button::new(egui::RichText::new("General").color(theme::TEXT_BRIGHT))
                            .fill(gen_fill),
                    ).observed(ui)
                    .clicked()
                {
                    super::entity_access::select_tab(active_tab,access,SettingsTab::General);
                }

                let acc_fill = if current_tab == SettingsTab::Access {
                    theme::BG_TAB_ACTIVE
                } else {
                    theme::BG_TAB_INACTIVE
                };
                if ui
                    .add_enabled(can_edit_draft && !details.loading() && details.error.is_none(),
                        egui::Button::new(egui::RichText::new("Access").color(theme::TEXT_BRIGHT))
                            .fill(acc_fill),
                    ).observed(ui).on_disabled_hover_text("Load this entry's stored Manager role to manage access.")
                    .clicked()
                {
                    super::entity_access::select_tab(active_tab,access,SettingsTab::Access);
                }

                let api_fill = if current_tab == SettingsTab::ApiAccess {
                    theme::BG_TAB_ACTIVE
                } else {
                    theme::BG_TAB_INACTIVE
                };
                if ui
                    .add_enabled(can_edit_draft && !details.loading() && details.error.is_none(),
                        egui::Button::new(egui::RichText::new("API").color(theme::TEXT_BRIGHT))
                            .fill(api_fill),
                    ).observed(ui).on_disabled_hover_text("Load this entry's stored Manager role to manage API access.")
                    .clicked()
                {
                    super::entity_access::select_tab(active_tab,access,SettingsTab::ApiAccess);

                }
            });

            ui.add_space(12.0);
            ui.separator();
            ui.add_space(8.0);

            // Tab content
            egui::ScrollArea::vertical().id_salt(("entity_settings_body",entity_id.as_str(),current_tab as u8)).show(ui,|ui| {
            match current_tab {
                SettingsTab::General => {
                    // --- Rename ---
                    let name_label = ui.label(egui::RichText::new("Name").color(theme::TEXT_SECTION).strong());
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.add_enabled(can_edit_draft, crate::ui_shell::layout_policy::text_input(ui, name_buf).desired_width(300.0)).labelled(ui, &name_label);
                        if ui.add_enabled(can_write, egui::Button::new("Rename").fill(theme::BG_SAVE)).named(ui, format_args!("Rename {type_label} {entity_name}")).clicked() {
                            let new_name = name_buf.trim().to_string();
                            if !new_name.is_empty() {
                                let etype = match entity_type {
                                    EntitySettingsType::Verse => EntityType::Verse,
                                    EntitySettingsType::Fractal => EntityType::Fractal,
                                    EntitySettingsType::Petal => EntityType::Petal,
                                };
                                if let Err(error) = db_tx.send(DbCommand::RenameEntity {
                                        entity_type: etype,
                                        entity_id: entity_id.clone(),
                                        new_name,
                                    }) {
                                    bevy::log::warn!(%error, "RenameEntity channel closed");
                                    send_failure = Some("Rename could not be sent. The database is unavailable.");
                                }
                            }
                        }
                    });

                    // --- Default Access (Verse only) ---
                    if *entity_type == EntitySettingsType::Verse {
                        ui.add_space(12.0);
                        ui.label(
                            egui::RichText::new("Default Access")
                                .color(theme::TEXT_SECTION)
                                .strong(),
                        );
                        ui.add_space(4.0);
                        if let Some(stored) = &details.loaded { ui.label(format!("Stored default access: {}", stored.default_access.as_deref().unwrap_or("not set"))); }
                        ui.add_enabled_ui(can_write, |ui| {
                        if let Some(ref mut da) = default_access_buf {
                            let mut is_viewer = da == "viewer";
                            if ui
                                .radio_value(&mut is_viewer, true, "Public (viewer) — anyone can see").observed(ui)
                                .clicked()
                            {
                                *da = "viewer".to_string();
                                if db_tx.send(DbCommand::SetVerseDefaultAccess {
                                        verse_id: entity_id.clone(),
                                        default_access: "viewer".to_string(),
                                    }).is_err() {
                                    bevy::log::warn!("SetVerseDefaultAccess channel closed");
                                    send_failure = Some("Default access could not be sent. The database is unavailable.");
                                }
                            }
                            if ui
                                .radio_value(&mut is_viewer, false, "Private (none) — explicit grants only").observed(ui)
                                .clicked()
                            {
                                *da = "none".to_string();
                                if db_tx.send(DbCommand::SetVerseDefaultAccess {
                                        verse_id: entity_id.clone(),
                                        default_access: "none".to_string(),
                                    }).is_err() {
                                    bevy::log::warn!("SetVerseDefaultAccess channel closed");
                                    send_failure = Some("Default access could not be sent. The database is unavailable.");
                                }
                            }
                        } else if details.loaded.is_some() {
                            ui.label("No default access is stored. Choose an explicit setting.");
                            for (label, value) in [("Set public (viewer)", "viewer"), ("Set private (none)", "none")] {
                                if ui.button(label).named(ui, format_args!("{label} for {entity_name}")).clicked()
                                    && db_tx.send(DbCommand::SetVerseDefaultAccess { verse_id: entity_id.clone(), default_access: value.into() }).is_err() {
                                        bevy::log::warn!("SetVerseDefaultAccess channel closed");
                                        send_failure = Some("Default access could not be sent. The database is unavailable.");
                                    }
                            }
                        } else { ui.label("Waiting for the stored default access setting."); }
                        });
                    }

                    // --- Description (Fractal only) ---
                    if *entity_type == EntitySettingsType::Fractal {
                        ui.add_space(12.0);
                        let description_label = ui.label(
                            egui::RichText::new("Description").color(theme::TEXT_SECTION).strong(),
                        );
                        ui.add_space(4.0);
                        if let Some(ref mut desc) = description_buf {
                            ui.add_enabled(can_edit_draft,
                                egui::TextEdit::multiline(desc)
                                    .desired_rows(3)
                                    .desired_width(f32::INFINITY),
                            )
                            .labelled(ui, &description_label);
                            if ui
                                .add_enabled(can_write, egui::Button::new("Save Description").fill(theme::BG_SAVE)).named(ui, format_args!("Save description for {entity_name}"))
                                .clicked()
                            {
                                if let Err(error) = db_tx.send(DbCommand::UpdateFractalDescription {
                                        fractal_id: entity_id.clone(),
                                        description: desc.clone(),
                                    }) {
                                    bevy::log::warn!(%error, "UpdateFractalDescription channel closed");
                                    send_failure = Some("Description could not be saved. The database is unavailable.");
                                }
                            }
                        }
                    }

                    // --- Claim Ownership (F6, Verse only) ---
                    // Shown whenever this node is not already Owner here;
                    // the DB handler (`ClaimOrphanedVerse`) enforces the
                    // real eligibility guard (never-shared, did:key-orphaned
                    // creator) and refuses with a typed reason surfaced as a
                    // toast — a click on an ineligible verse is a no-op,
                    // never a silent one. OF-4 stays dead: this is explicit,
                    // user-confirmed, never automatic.
                    if *entity_type == EntitySettingsType::Verse
                        && !details.loaded.as_ref().is_some_and(|stored| fe_database::RoleLevel::from(stored.role.as_str()) == fe_database::RoleLevel::Owner)
                    {
                        ui.add_space(20.0);
                        ui.separator();
                        ui.add_space(8.0);
                        ui.label(
                            egui::RichText::new("Claim Ownership")
                                .color(theme::TEXT_SECTION)
                                .strong(),
                        );
                        ui.add_space(4.0);
                        ui.label(
                            egui::RichText::new(
                                "If this verse was created on this device under a previous \
                                 identity and has never been shared with anyone else, you \
                                 can reclaim ownership of it.",
                            )
                            .small()
                            .color(theme::TEXT_DIM),
                        );
                        ui.add_space(4.0);
                        if !*pending_claim_ownership {
                            if ui
                                .add(egui::Button::new("Claim Ownership").fill(theme::BG_BUTTON)).observed(ui)
                                .clicked()
                            {
                                *pending_claim_ownership = true;
                            }
                        } else {
                            ui.label(
                                egui::RichText::new(
                                    "Confirm — this only succeeds for a verse never shared \
                                     with anyone.",
                                )
                                .color(theme::STATUS_OFFLINE),
                            );
                            ui.horizontal(|ui| {
                                if ui
                                    .add(egui::Button::new("Confirm Claim").fill(theme::BG_SAVE)).observed(ui)
                                    .clicked()
                                {
                                    db_tx
                                        .send(DbCommand::ClaimOrphanedVerse {
                                            verse_id: entity_id.clone(),
                                            correlation_id: None,
                                        })
                                        .ok();
                                    *pending_claim_ownership = false;
                                }
                                if ui.add(egui::Button::new("Cancel").fill(theme::BG_BUTTON)).observed(ui).clicked()
                                {
                                    *pending_claim_ownership = false;
                                }
                            });
                        }
                    }

                    // --- Delete ---
                    ui.add_space(20.0);
                    ui.separator();
                    ui.add_space(8.0);
                    ui.add_enabled_ui(can_write, |ui| {
                    if !*pending_delete {
                        if ui
                            .add(
                                egui::Button::new(egui::RichText::new("Delete").color(egui::Color32::WHITE))
                                    .fill(theme::BG_DANGER),
                            ).observed(ui)
                            .clicked()
                        {
                            *pending_delete = true;
                        }
                    } else {
                        ui.label(
                            egui::RichText::new("Are you sure? This cannot be undone.")
                                .color(theme::STATUS_OFFLINE),
                        );
                        ui.horizontal(|ui| {
                            if ui
                                .add(
                                    egui::Button::new(
                                        egui::RichText::new("Confirm Delete").color(egui::Color32::WHITE),
                                    )
                                    .fill(theme::BG_DANGER),
                                ).observed(ui)
                                .clicked()
                            {
                                let etype = match entity_type {
                                    EntitySettingsType::Verse => EntityType::Verse,
                                    EntitySettingsType::Fractal => EntityType::Fractal,
                                    EntitySettingsType::Petal => EntityType::Petal,
                                };
                                match db_tx.send(DbCommand::DeleteEntity {
                                        entity_type: etype,
                                        entity_id: entity_id.clone(),
                                    }) {
                                    Ok(()) => close = true,
                                    Err(error) => {
                                        bevy::log::warn!(%error, "DeleteEntity channel closed");
                                        send_failure = Some("Delete could not be sent. The database is unavailable.");
                                    }
                                }
                            }
                            if ui.add(egui::Button::new("Cancel").fill(theme::BG_BUTTON)).observed(ui).clicked() {
                                *pending_delete = false;
                            }
                        });
                    }
                    });
                }
                SettingsTab::Access => {
                    ui.label(egui::RichText::new("Peer Access").strong());
                    let verse_scope = *entity_type == EntitySettingsType::Verse;
                    if let Some(preset) = super::entity_access::preset_controls(ui, access, default_access_buf.as_deref(), verse_scope, can_write) {
                        use super::entity_access::AccessPreset;
                        let stored_default = match preset {
                            AccessPreset::Public => Some("viewer"),
                            AccessPreset::MembersOnly => Some("none"),
                            AccessPreset::ManagersOnly => verse_scope.then_some("none"),
                        };
                        if let Some(value) = stored_default.filter(|_| verse_scope) {
                            if db_tx.send(DbCommand::SetVerseDefaultAccess { verse_id: entity_id.clone(), default_access: value.to_string() }).is_err() {
                                bevy::log::warn!("SetVerseDefaultAccess channel closed");
                                send_failure = Some("Default access could not be sent. The database is unavailable.");
                            } else if let Some(buffer) = default_access_buf.as_mut() {
                                *buffer = value.to_string();
                            }
                        }
                        if preset == AccessPreset::ManagersOnly {
                            access.start_managers_only();
                        }
                    }
                    access_action=super::entity_access::role_controls(ui,access,known_peers);
                    let copied=super::entity_access::invite_controls(ui,access,invite_role_buf,invite_expiry_buf,known_peers);
                    copied_access |= copied;
                }
                SettingsTab::ApiAccess => {
                    ui.label(egui::RichText::new("API Access").strong());
                    let (action,copied)=super::entity_access::api_controls(ui,access,api_token_role_buf,api_token_expiry_buf,api_token_client_buf,api_endpoint);
                    access_action=action;copied_access|=copied;
                }
            }
            });
        });

    if let Some(action) = access_action {
        super::entity_access::submit(ui_mgr, action, db_tx);
    }
    if copied_access {
        ui_mgr.show_toast("Copied access value", ctx.input(|input| input.time));
    }
    if let Some(message) = send_failure {
        ui_mgr.show_error(message, ctx.input(|input| input.time));
    }
    if !still_open || close {
        ui_mgr.close_dialog();
    }
}
