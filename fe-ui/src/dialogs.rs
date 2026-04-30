use bevy_egui::egui;

use crate::navigation_manager::NavigationManager;
use crate::plugin::{ActiveDialog, CreateKind, EntitySettingsType, SettingsTab, UiManager};
use crate::theme;
use crate::verse_manager::VerseManager;
use fe_runtime::messages::{DbCommand, EntityType};

/// Build a hierarchical scope string from entity settings dialog context.
fn build_entity_scope(
    entity_type: &EntitySettingsType,
    entity_id: &str,
    parent_verse_id: &str,
    parent_fractal_id: &Option<String>,
) -> String {
    match entity_type {
        EntitySettingsType::Verse => fe_database::build_scope(entity_id, None, None),
        EntitySettingsType::Fractal => {
            fe_database::build_scope(parent_verse_id, Some(entity_id), None)
        }
        EntitySettingsType::Petal => {
            let fid = parent_fractal_id.as_deref().unwrap_or("");
            fe_database::build_scope(parent_verse_id, Some(fid), Some(entity_id))
        }
    }
}

// ---------------------------------------------------------------------------
// Context menu (viewport right-click)
// ---------------------------------------------------------------------------

pub fn render_context_menu(
    ctx: &egui::Context,
    ui_mgr: &mut UiManager,
) {
    let ActiveDialog::ContextMenu { screen_pos, world_pos } = ui_mgr.active_dialog else {
        return;
    };

    let pos = egui::pos2(screen_pos[0], screen_pos[1]);
    let world = world_pos;

    let mut next_dialog: Option<ActiveDialog> = None;
    let mut close = false;

    let area_response = egui::Area::new(egui::Id::new("viewport_context_menu"))
        .fixed_pos(pos)
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            egui::Frame::NONE
                .fill(theme::BG_CONTEXT_MENU)
                .inner_margin(egui::Margin::same(4))
                .corner_radius(4.0)
                .stroke(egui::Stroke::new(1.0, theme::TEXT_DIM))
                .show(ui, |ui| {
                    ui.set_min_width(160.0);

                    if ui
                        .add(egui::Button::new("Add GLTF Model").fill(egui::Color32::TRANSPARENT))
                        .clicked()
                    {
                        next_dialog = Some(ActiveDialog::GltfImport {
                            file_path_buf: String::new(),
                            name_buf: String::new(),
                            position: world,
                        });
                    }

                    if ui
                        .add(egui::Button::new("Add Empty Node").fill(egui::Color32::TRANSPARENT))
                        .clicked()
                    {
                        // TODO(tracked): CreateEmptyNode at cursor world pos
                        close = true;
                    }
                });
        });

    // Close on click elsewhere — use the actual rendered rect rather than a
    // hardcoded size so all items are accounted for regardless of content.
    if ctx.input(|i| i.pointer.any_pressed()) {
        let ptr = ctx.input(|i| i.pointer.interact_pos());
        if let Some(ptr_pos) = ptr {
            let menu_rect = area_response.response.rect;
            if !menu_rect.contains(ptr_pos) {
                close = true;
            }
        }
    }

    if let Some(dialog) = next_dialog {
        ui_mgr.open_dialog(dialog);
    } else if close {
        ui_mgr.close_dialog();
    }
}

// ---------------------------------------------------------------------------
// Create dialog (Verse / Fractal / Petal / Node)
// ---------------------------------------------------------------------------

pub fn render_create_dialog(
    ctx: &egui::Context,
    ui_mgr: &mut UiManager,
    hierarchy: &mut VerseManager,
    nav: &mut NavigationManager,
    db_tx: &crossbeam::channel::Sender<DbCommand>,
) {
    let ActiveDialog::CreateEntity { ref mut kind, ref mut parent_id, ref mut name_buf } =
        ui_mgr.active_dialog
    else {
        return;
    };

    let title = match *kind {
        CreateKind::Verse => "Create Verse",
        CreateKind::Fractal => "Create Fractal",
        CreateKind::Petal => "Create Petal",
        CreateKind::Node => "Create Node",
    };

    let current_kind = *kind;
    let current_parent = parent_id.clone();
    let mut close = false;

    let mut still_open = true;
    egui::Window::new(title)
        .open(&mut still_open)
        .collapsible(false)
        .resizable(false)
        .default_width(280.0)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .frame(
            egui::Frame::NONE
                .fill(theme::BG_DIALOG)
                .inner_margin(egui::Margin::same(12))
                .corner_radius(6.0)
                .stroke(egui::Stroke::new(1.0, theme::TEXT_DIM)),
        )
        .show(ctx, |ui| {
            ui.label(egui::RichText::new("Name:").small().color(theme::TEXT_DIM));
            ui.add(
                egui::TextEdit::singleline(name_buf)
                    .hint_text("Enter name\u{2026}")
                    .desired_width(f32::INFINITY),
            );

            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui
                    .add(egui::Button::new("Create").fill(theme::BG_SAVE))
                    .clicked()
                {
                    let name = name_buf.trim().to_string();
                    if !name.is_empty() {
                        apply_create(
                            hierarchy,
                            nav,
                            current_kind,
                            &current_parent,
                            &name,
                            db_tx,
                        );
                        close = true;
                    }
                }
                if ui
                    .add(egui::Button::new("Cancel").fill(theme::BG_BUTTON))
                    .clicked()
                {
                    close = true;
                }
            });
        });

    if !still_open || close {
        ui_mgr.close_dialog();
    }
}

/// Send a DB command to create the entity. The hierarchy will update when
/// the DbResult comes back -- no local push to avoid duplicates.
pub fn apply_create(
    _hierarchy: &mut VerseManager,
    _nav: &mut NavigationManager,
    kind: CreateKind,
    parent_id: &str,
    name: &str,
    db_tx: &crossbeam::channel::Sender<DbCommand>,
) {
    match kind {
        CreateKind::Verse => {
            db_tx
                .send(DbCommand::CreateVerse {
                    name: name.to_string(),
                })
                .ok();
        }
        CreateKind::Fractal => {
            db_tx
                .send(DbCommand::CreateFractal {
                    verse_id: parent_id.to_string(),
                    name: name.to_string(),
                })
                .ok();
        }
        CreateKind::Petal => {
            db_tx
                .send(DbCommand::CreatePetal {
                    fractal_id: parent_id.to_string(),
                    name: name.to_string(),
                })
                .ok();
        }
        CreateKind::Node => {
            db_tx
                .send(DbCommand::CreateNode {
                    petal_id: parent_id.to_string(),
                    name: name.to_string(),
                    position: [0.0, 0.0, 0.0],
                })
                .ok();
        }
    }
}

// ---------------------------------------------------------------------------
// GLTF import dialog
// ---------------------------------------------------------------------------

pub fn render_gltf_import_dialog(
    ctx: &egui::Context,
    ui_mgr: &mut UiManager,
    nav: &NavigationManager,
    db_tx: &crossbeam::channel::Sender<DbCommand>,
) {
    let ActiveDialog::GltfImport {
        ref mut file_path_buf,
        ref mut name_buf,
        ref mut position,
    } = ui_mgr.active_dialog
    else {
        return;
    };

    let mut close = false;

    let mut still_open = true;
    egui::Window::new("Import GLTF Model")
        .open(&mut still_open)
        .collapsible(false)
        .resizable(false)
        .default_width(340.0)
        .max_width(400.0)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .frame(
            egui::Frame::NONE
                .fill(theme::BG_DIALOG)
                .inner_margin(egui::Margin::same(12))
                .corner_radius(6.0)
                .stroke(egui::Stroke::new(1.0, theme::TEXT_DIM)),
        )
        .show(ctx, |ui| {
            ui.label(
                egui::RichText::new("File Path:")
                    .small()
                    .color(theme::TEXT_DIM),
            );
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(file_path_buf)
                        .hint_text("path/to/model.glb")
                        .desired_width(ui.available_width() - 70.0),
                );
                if ui
                    .add(egui::Button::new("Browse...").fill(theme::BG_BUTTON))
                    .clicked()
                {
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("GLTF Models", &["glb", "gltf"])
                        .add_filter("All Files", &["*"])
                        .pick_file()
                    {
                        *file_path_buf = path.display().to_string();
                        if name_buf.is_empty() {
                            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                                *name_buf = stem.to_string();
                            }
                        }
                    }
                }
            });

            ui.add_space(4.0);
            ui.label(
                egui::RichText::new("Display Name:")
                    .small()
                    .color(theme::TEXT_DIM),
            );
            ui.add(
                egui::TextEdit::singleline(name_buf)
                    .hint_text("My Model")
                    .desired_width(f32::INFINITY),
            );

            ui.add_space(4.0);
            ui.label(
                egui::RichText::new("Position:")
                    .small()
                    .color(theme::TEXT_DIM),
            );
            ui.horizontal(|ui| {
                for (axis, val) in ["X", "Y", "Z"].iter().zip(position.iter()) {
                    ui.label(egui::RichText::new(*axis).small().color(theme::TEXT_AXIS));
                    ui.label(
                        egui::RichText::new(format!("{:.2}", val))
                            .monospace()
                            .small(),
                    );
                }
            });

            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui
                    .add(egui::Button::new("Import").fill(theme::BG_SAVE))
                    .clicked()
                {
                    let file_path = file_path_buf.trim().to_string();
                    let name = name_buf.trim().to_string();
                    let petal_id = nav.active_petal_id.clone().unwrap_or_default();
                    if !file_path.is_empty() && !petal_id.is_empty() {
                        db_tx
                            .send(DbCommand::ImportGltf {
                                petal_id,
                                name: if name.is_empty() {
                                    "Imported Model".to_string()
                                } else {
                                    name
                                },
                                file_path,
                                position: *position,
                            })
                            .ok();
                    }
                    close = true;
                }
                if ui
                    .add(egui::Button::new("Cancel").fill(theme::BG_BUTTON))
                    .clicked()
                {
                    close = true;
                }
            });
        });

    if !still_open || close {
        ui_mgr.close_dialog();
    }
}

// ---------------------------------------------------------------------------
// Phase F: Join dialog
// ---------------------------------------------------------------------------

pub fn render_join_dialog(
    ctx: &egui::Context,
    ui_mgr: &mut UiManager,
    db_tx: &crossbeam::channel::Sender<DbCommand>,
) {
    let ActiveDialog::JoinDialog { ref mut invite_buf } = ui_mgr.active_dialog else {
        return;
    };

    let mut close = false;

    egui::Window::new("Join Verse")
        .collapsible(false)
        .resizable(true)
        .default_width(400.0)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.label("Paste an invite string to join a verse:");
            ui.add_space(4.0);

            ui.add(
                egui::TextEdit::multiline(invite_buf)
                    .desired_rows(3)
                    .desired_width(f32::INFINITY)
                    .hint_text("Paste invite string here...")
                    .font(egui::TextStyle::Monospace),
            );

            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let can_join = !invite_buf.trim().is_empty();
                if ui
                    .add_enabled(can_join, egui::Button::new("Join"))
                    .clicked()
                {
                    db_tx
                        .send(DbCommand::JoinVerseByInvite {
                            invite_string: invite_buf.trim().to_string(),
                        })
                        .ok();
                    close = true;
                }
                if ui.button("Cancel").clicked() {
                    close = true;
                }
            });
        });

    if close {
        ui_mgr.close_dialog();
    }
}

// ---------------------------------------------------------------------------
// Phase F: Peer debug panel
// ---------------------------------------------------------------------------

pub fn render_peer_debug_panel(
    ctx: &egui::Context,
    ui_mgr: &mut UiManager,
    sync_status: Option<&fe_sync::SyncStatus>,
) {
    if !matches!(ui_mgr.active_dialog, ActiveDialog::PeerDebug) {
        return;
    }

    let mut close = false;

    egui::Window::new("Peer Debug")
        .collapsible(true)
        .resizable(true)
        .default_width(300.0)
        .show(ctx, |ui| {
            if let Some(status) = sync_status {
                ui.label(format!(
                    "Online: {}",
                    if status.online { "Yes" } else { "No" }
                ));
                ui.label(format!("Peer count: {}", status.peer_count));
                if let Some(ref addr) = status.node_addr {
                    ui.add_space(4.0);
                    ui.label("Node address:");
                    ui.monospace(addr);
                }
            } else {
                ui.label("Sync status not available");
            }

            ui.add_space(8.0);
            ui.label(
                egui::RichText::new("Peer list will be populated when gossip discovery is active.")
                    .small()
                    .color(theme::TEXT_MUTED),
            );

            ui.add_space(4.0);
            if ui.button("Close").clicked() {
                close = true;
            }
        });

    if close {
        ui_mgr.close_dialog();
    }
}

// ---------------------------------------------------------------------------
// Node options dialog
// ---------------------------------------------------------------------------

pub fn render_node_options_dialog(
    ctx: &egui::Context,
    ui_mgr: &mut UiManager,
    hierarchy: &mut VerseManager,
    db_tx: &crossbeam::channel::Sender<DbCommand>,
) {
    let ActiveDialog::NodeOptions {
        ref node_id,
        ref mut node_name_buf,
        ref mut webpage_url_buf,
    } = ui_mgr.active_dialog
    else {
        return;
    };

    let current_node_id = node_id.clone();
    let mut close = false;

    let mut still_open = true;
    egui::Window::new("Node Options")
        .open(&mut still_open)
        .collapsible(false)
        .resizable(false)
        .default_width(300.0)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .frame(
            egui::Frame::NONE
                .fill(theme::BG_DIALOG)
                .inner_margin(egui::Margin::same(12))
                .corner_radius(6.0)
                .stroke(egui::Stroke::new(1.0, theme::TEXT_DIM)),
        )
        .show(ctx, |ui| {
            ui.label(egui::RichText::new("Name:").small().color(theme::TEXT_DIM));
            ui.add(
                egui::TextEdit::singleline(node_name_buf)
                    .desired_width(f32::INFINITY),
            );

            ui.add_space(6.0);
            ui.label(
                egui::RichText::new("Webpage URL:")
                    .small()
                    .color(theme::TEXT_DIM),
            );
            ui.add(
                egui::TextEdit::singleline(webpage_url_buf)
                    .hint_text("https://\u{2026}")
                    .desired_width(f32::INFINITY),
            );
            ui.label(
                egui::RichText::new(
                    "This URL will be rendered in the embedded webview when selected.",
                )
                .small()
                .italics()
                .color(theme::TEXT_MUTED),
            );

            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui
                    .add(egui::Button::new("Save").fill(theme::BG_SAVE))
                    .clicked()
                {
                    let url = webpage_url_buf.trim().to_string();
                    let url_opt = if url.is_empty() { None } else { Some(url) };
                    node_options_save_url(hierarchy, &current_node_id, url_opt.clone(), db_tx);
                    close = true;
                }
                if ui
                    .add(egui::Button::new("Cancel").fill(theme::BG_BUTTON))
                    .clicked()
                {
                    close = true;
                }
            });
        });

    if !still_open || close {
        ui_mgr.close_dialog();
    }
}

pub fn node_options_save_url(
    hierarchy: &mut VerseManager,
    node_id: &str,
    url: Option<String>,
    db_tx: &crossbeam::channel::Sender<DbCommand>,
) {
    hierarchy.update_node_url(node_id, url.clone());
    if db_tx
        .send(DbCommand::UpdateNodeUrl {
            node_id: node_id.to_string(),
            url,
        })
        .is_err()
    {
        bevy::log::warn!("db_sender channel closed — UpdateNodeUrl (node options) not persisted");
    }
}

// ---------------------------------------------------------------------------
// Entity Settings dialog (General + Access tabs)
// ---------------------------------------------------------------------------

pub fn render_entity_settings_dialog(
    ctx: &egui::Context,
    ui_mgr: &mut UiManager,
    db_tx: &crossbeam::channel::Sender<DbCommand>,
) {
    // Early-return guard: only render when EntitySettings is active.
    let ActiveDialog::EntitySettings {
        ref entity_type,
        ref entity_id,
        ref entity_name,
        ref parent_verse_id,
        ref parent_fractal_id,
        ref mut active_tab,
        ref mut name_buf,
        ref mut default_access_buf,
        ref mut description_buf,
        ref mut peer_roles,
        ref mut roles_loading,
        ref mut invite_role_buf,
        ref mut invite_expiry_buf,
        ref mut generated_invite_link,
        ref mut pending_delete,
        ref mut api_tokens,
        ref mut api_tokens_loading,
        ref mut api_token_scope_buf,
        ref mut api_token_role_buf,
        ref mut api_token_expiry_buf,
        ref mut generated_api_token,
        ref mut scoped_api_tokens,
        ref mut scoped_tokens_loading,
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

    let mut close = false;
    let mut still_open = true;

    egui::Window::new(title)
        .open(&mut still_open)
        .collapsible(false)
        .resizable(true)
        .default_width(500.0)
        .min_width(400.0)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .frame(
            egui::Frame::NONE
                .fill(theme::BG_DIALOG)
                .inner_margin(egui::Margin::same(16))
                .corner_radius(6.0)
                .stroke(egui::Stroke::new(1.0, theme::TEXT_DIM)),
        )
        .show(ctx, |ui| {
            // Tab bar
            ui.horizontal(|ui| {
                let gen_fill = if current_tab == SettingsTab::General {
                    theme::BG_TAB_ACTIVE
                } else {
                    theme::BG_TAB_INACTIVE
                };
                if ui
                    .add(egui::Button::new(
                        egui::RichText::new("General").color(theme::TEXT_BRIGHT),
                    ).fill(gen_fill))
                    .clicked()
                {
                    *active_tab = SettingsTab::General;
                }

                let acc_fill = if current_tab == SettingsTab::Access {
                    theme::BG_TAB_ACTIVE
                } else {
                    theme::BG_TAB_INACTIVE
                };
                if ui
                    .add(egui::Button::new(
                        egui::RichText::new("Access").color(theme::TEXT_BRIGHT),
                    ).fill(acc_fill))
                    .clicked()
                {
                    *active_tab = SettingsTab::Access;
                }

                let api_fill = if current_tab == SettingsTab::ApiAccess {
                    theme::BG_TAB_ACTIVE
                } else {
                    theme::BG_TAB_INACTIVE
                };
                if ui
                    .add(egui::Button::new(
                        egui::RichText::new("API").color(theme::TEXT_BRIGHT),
                    ).fill(api_fill))
                    .clicked()
                {
                    *active_tab = SettingsTab::ApiAccess;
                    // Request token list when switching to API tab
                    db_tx.send(DbCommand::ListApiTokens).ok();
                }
            });

            ui.add_space(12.0);
            ui.separator();
            ui.add_space(8.0);

            // Tab content
            match current_tab {
                SettingsTab::General => {
                    // --- Rename ---
                    ui.label(egui::RichText::new("Name").color(theme::TEXT_SECTION).strong());
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(name_buf)
                                .desired_width(300.0),
                        );
                        if ui.add(egui::Button::new("Rename").fill(theme::BG_SAVE)).clicked() {
                            let new_name = name_buf.trim().to_string();
                            if !new_name.is_empty() {
                                let etype = match entity_type {
                                    EntitySettingsType::Verse => EntityType::Verse,
                                    EntitySettingsType::Fractal => EntityType::Fractal,
                                    EntitySettingsType::Petal => EntityType::Petal,
                                };
                                db_tx.send(DbCommand::RenameEntity {
                                    entity_type: etype,
                                    entity_id: entity_id.clone(),
                                    new_name,
                                }).ok();
                            }
                        }
                    });

                    // --- Default Access (Verse only) ---
                    if *entity_type == EntitySettingsType::Verse {
                        ui.add_space(12.0);
                        ui.label(egui::RichText::new("Default Access").color(theme::TEXT_SECTION).strong());
                        ui.add_space(4.0);
                        if let Some(ref mut da) = default_access_buf {
                            let mut is_viewer = da == "viewer";
                            if ui.radio_value(&mut is_viewer, true, "Public (viewer) — anyone can see").clicked() {
                                *da = "viewer".to_string();
                                db_tx.send(DbCommand::SetVerseDefaultAccess {
                                    verse_id: entity_id.clone(),
                                    default_access: "viewer".to_string(),
                                }).ok();
                            }
                            if ui.radio_value(&mut is_viewer, false, "Private (none) — explicit grants only").clicked() {
                                *da = "none".to_string();
                                db_tx.send(DbCommand::SetVerseDefaultAccess {
                                    verse_id: entity_id.clone(),
                                    default_access: "none".to_string(),
                                }).ok();
                            }
                        }
                    }

                    // --- Description (Fractal only) ---
                    if *entity_type == EntitySettingsType::Fractal {
                        ui.add_space(12.0);
                        ui.label(egui::RichText::new("Description").color(theme::TEXT_SECTION).strong());
                        ui.add_space(4.0);
                        if let Some(ref mut desc) = description_buf {
                            ui.add(
                                egui::TextEdit::multiline(desc)
                                    .desired_rows(3)
                                    .desired_width(f32::INFINITY),
                            );
                            if ui.add(egui::Button::new("Save Description").fill(theme::BG_SAVE)).clicked() {
                                db_tx.send(DbCommand::UpdateFractalDescription {
                                    fractal_id: entity_id.clone(),
                                    description: desc.clone(),
                                }).ok();
                            }
                        }
                    }

                    // --- Delete ---
                    ui.add_space(20.0);
                    ui.separator();
                    ui.add_space(8.0);
                    if !*pending_delete {
                        if ui.add(egui::Button::new(
                            egui::RichText::new("Delete").color(egui::Color32::WHITE),
                        ).fill(theme::BG_DANGER)).clicked() {
                            *pending_delete = true;
                        }
                    } else {
                        ui.label(egui::RichText::new("Are you sure? This cannot be undone.").color(theme::STATUS_OFFLINE));
                        ui.horizontal(|ui| {
                            if ui.add(egui::Button::new(
                                egui::RichText::new("Confirm Delete").color(egui::Color32::WHITE),
                            ).fill(theme::BG_DANGER)).clicked() {
                                let etype = match entity_type {
                                    EntitySettingsType::Verse => EntityType::Verse,
                                    EntitySettingsType::Fractal => EntityType::Fractal,
                                    EntitySettingsType::Petal => EntityType::Petal,
                                };
                                db_tx.send(DbCommand::DeleteEntity {
                                    entity_type: etype,
                                    entity_id: entity_id.clone(),
                                }).ok();
                                close = true;
                            }
                            if ui.add(egui::Button::new("Cancel").fill(theme::BG_BUTTON)).clicked() {
                                *pending_delete = false;
                            }
                        });
                    }
                }
                SettingsTab::Access => {
                    ui.label(egui::RichText::new("Peer Access").color(theme::TEXT_SECTION).strong());
                    ui.add_space(8.0);

                    if peer_roles.is_empty() && !*roles_loading {
                        ui.label(egui::RichText::new("No peers known yet. Peers will appear here when they connect.").color(theme::TEXT_MUTED).italics());
                    } else if *roles_loading {
                        ui.label(egui::RichText::new("Loading peer roles...").color(theme::TEXT_MUTED).italics());
                    } else {
                        // Peer list table
                        for (i, peer) in peer_roles.iter_mut().enumerate() {
                            let row_bg = if i % 2 == 0 { theme::BG_PEER_ROW_EVEN } else { theme::BG_PEER_ROW_ODD };
                            egui::Frame::NONE.fill(row_bg).inner_margin(egui::Margin::same(4)).show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    // Online indicator
                                    let dot_color = if peer.is_online { theme::ICON_ONLINE } else { theme::ICON_OFFLINE };
                                    ui.colored_label(dot_color, "\u{25CF}");

                                    // Display name
                                    let display = if peer.display_name.is_empty() { &peer.peer_did } else { &peer.display_name };
                                    ui.label(egui::RichText::new(display).strong());

                                    // Truncated DID
                                    if !peer.display_name.is_empty() {
                                        let truncated = if peer.peer_did.len() > 20 {
                                            format!("{}...", &peer.peer_did[..20])
                                        } else {
                                            peer.peer_did.clone()
                                        };
                                        ui.label(egui::RichText::new(truncated).monospace().small().color(theme::TEXT_DIM));
                                    }

                                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                        // Role badge (read-only display for now)
                                        let role_color = match peer.role.as_str() {
                                            "owner" => theme::TEXT_ROLE_OWNER,
                                            "manager" => theme::TEXT_ROLE_MANAGER,
                                            "editor" => theme::TEXT_ROLE_EDITOR,
                                            _ => theme::TEXT_ROLE_VIEWER,
                                        };
                                        ui.label(egui::RichText::new(&peer.role).color(role_color).strong());
                                    });
                                });
                            });
                        }
                    }

                    // --- Invite generation section ---
                    ui.add_space(16.0);
                    ui.separator();
                    ui.add_space(8.0);
                    ui.label(egui::RichText::new("Generate Invite Link").color(theme::TEXT_SECTION).strong());
                    ui.add_space(4.0);

                    ui.horizontal(|ui| {
                        ui.label("Role:");
                        egui::ComboBox::from_id_salt("invite_role")
                            .selected_text(invite_role_buf.as_str())
                            .width(100.0)
                            .show_ui(ui, |ui| {
                                ui.selectable_value(invite_role_buf, "viewer".to_string(), "Viewer");
                                ui.selectable_value(invite_role_buf, "editor".to_string(), "Editor");
                                ui.selectable_value(invite_role_buf, "manager".to_string(), "Manager");
                            });

                        ui.label("Expires:");
                        egui::ComboBox::from_id_salt("invite_expiry")
                            .selected_text(match *invite_expiry_buf {
                                1 => "1 hour",
                                24 => "24 hours",
                                168 => "7 days",
                                720 => "30 days",
                                _ => "24 hours",
                            })
                            .width(100.0)
                            .show_ui(ui, |ui| {
                                ui.selectable_value(invite_expiry_buf, 1, "1 hour");
                                ui.selectable_value(invite_expiry_buf, 24, "24 hours");
                                ui.selectable_value(invite_expiry_buf, 168, "7 days");
                                ui.selectable_value(invite_expiry_buf, 720, "30 days");
                            });

                        if ui.add(egui::Button::new("Generate").fill(theme::BG_SAVE)).clicked() {
                            let _etype = match entity_type {
                                EntitySettingsType::Verse => "verse",
                                EntitySettingsType::Fractal => "fractal",
                                EntitySettingsType::Petal => "petal",
                            };
                            let scope = build_entity_scope(entity_type, entity_id, parent_verse_id, parent_fractal_id);
                            db_tx.send(DbCommand::GenerateScopedInvite {
                                scope,
                                role: invite_role_buf.clone(),
                                expiry_hours: *invite_expiry_buf,
                            }).ok();
                        }
                    });

                    // Show generated link
                    if let Some(ref link) = generated_invite_link {
                        ui.add_space(8.0);
                        egui::Frame::NONE
                            .fill(theme::BG_INVITE_LINK)
                            .inner_margin(egui::Margin::same(8))
                            .corner_radius(4.0)
                            .show(ui, |ui| {
                                let mut link_display = link.clone();
                                ui.add(
                                    egui::TextEdit::multiline(&mut link_display)
                                        .desired_rows(2)
                                        .desired_width(f32::INFINITY)
                                        .font(egui::TextStyle::Monospace),
                                );
                                if ui.button("Copy to Clipboard").clicked() {
                                    ui.ctx().copy_text(link.clone());
                                }
                            });
                    }
                }
                SettingsTab::ApiAccess => {
                    ui.label(egui::RichText::new("API Access Tokens").color(theme::TEXT_SECTION).strong());
                    ui.add_space(4.0);
                    ui.label(egui::RichText::new(
                        "Generate tokens for external API, MCP, and IoT access to this entity."
                    ).small().color(theme::TEXT_MUTED).italics());
                    ui.add_space(8.0);

                    // --- Server info ---
                    ui.label(egui::RichText::new("API Server").color(theme::TEXT_SECTION).strong());
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("Endpoint:").small().color(theme::TEXT_DIM));
                        ui.label(egui::RichText::new("http://127.0.0.1:8765").monospace().small());
                    });
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("MCP:").small().color(theme::TEXT_DIM));
                        ui.label(egui::RichText::new("POST /mcp").monospace().small());
                    });
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("WebSocket:").small().color(theme::TEXT_DIM));
                        ui.label(egui::RichText::new("GET /ws").monospace().small());
                    });

                    ui.add_space(12.0);
                    ui.separator();
                    ui.add_space(8.0);

                    // --- Generate new token ---
                    ui.label(egui::RichText::new("Generate New Token").color(theme::TEXT_SECTION).strong());
                    ui.add_space(4.0);

                    // Auto-fill scope from entity using correct hierarchy
                    if api_token_scope_buf.is_empty() {
                        *api_token_scope_buf = build_entity_scope(entity_type, entity_id, parent_verse_id, parent_fractal_id);
                    }

                    // Auto-load scoped tokens for admin dashboard on first render
                    if scoped_api_tokens.is_empty() && !*scoped_tokens_loading {
                        *scoped_tokens_loading = true;
                        let scope_prefix = build_entity_scope(entity_type, entity_id, parent_verse_id, parent_fractal_id);
                        db_tx.send(DbCommand::ListApiTokensByScope { scope_prefix }).ok();
                    }

                    ui.horizontal(|ui| {
                        ui.label("Scope:");
                        ui.add(
                            egui::TextEdit::singleline(api_token_scope_buf)
                                .desired_width(200.0)
                                .font(egui::TextStyle::Monospace),
                        );
                    });

                    ui.horizontal(|ui| {
                        ui.label("Max Role:");
                        egui::ComboBox::from_id_salt("api_token_role")
                            .selected_text(api_token_role_buf.as_str())
                            .width(100.0)
                            .show_ui(ui, |ui| {
                                ui.selectable_value(api_token_role_buf, "viewer".to_string(), "Viewer");
                                ui.selectable_value(api_token_role_buf, "editor".to_string(), "Editor");
                                ui.selectable_value(api_token_role_buf, "manager".to_string(), "Manager");
                            });

                        ui.label("Expires:");
                        egui::ComboBox::from_id_salt("api_token_expiry")
                            .selected_text(match *api_token_expiry_buf {
                                1 => "1 hour",
                                24 => "24 hours",
                                168 => "7 days",
                                720 => "30 days",
                                _ => "24 hours",
                            })
                            .width(100.0)
                            .show_ui(ui, |ui| {
                                ui.selectable_value(api_token_expiry_buf, 1, "1 hour");
                                ui.selectable_value(api_token_expiry_buf, 24, "24 hours");
                                ui.selectable_value(api_token_expiry_buf, 168, "7 days");
                                ui.selectable_value(api_token_expiry_buf, 720, "30 days");
                            });
                    });

                    ui.add_space(4.0);
                    if ui.add(egui::Button::new("Generate API Token").fill(theme::BG_SAVE)).clicked() {
                        db_tx.send(DbCommand::MintApiToken {
                            scope: api_token_scope_buf.clone(),
                            max_role: api_token_role_buf.clone(),
                            ttl_hours: *api_token_expiry_buf,
                            label: None,
                        }).ok();
                    }

                    // Show generated token
                    if let Some(ref token) = generated_api_token {
                        ui.add_space(8.0);
                        egui::Frame::NONE
                            .fill(theme::BG_INVITE_LINK)
                            .inner_margin(egui::Margin::same(8))
                            .corner_radius(4.0)
                            .show(ui, |ui| {
                                ui.label(egui::RichText::new("Token (copy now — shown only once):").small().color(theme::STATUS_OFFLINE).strong());
                                let mut token_display = token.clone();
                                ui.add(
                                    egui::TextEdit::multiline(&mut token_display)
                                        .desired_rows(3)
                                        .desired_width(f32::INFINITY)
                                        .font(egui::TextStyle::Monospace),
                                );
                                if ui.button("Copy to Clipboard").clicked() {
                                    ui.ctx().copy_text(token.clone());
                                }
                            });
                    }

                    // --- Active tokens list ---
                    ui.add_space(12.0);
                    ui.separator();
                    ui.add_space(8.0);
                    ui.label(egui::RichText::new("Active Tokens").color(theme::TEXT_SECTION).strong());
                    ui.add_space(4.0);

                    if *api_tokens_loading {
                        ui.label(egui::RichText::new("Loading tokens...").color(theme::TEXT_MUTED).italics());
                    } else if api_tokens.is_empty() {
                        ui.label(egui::RichText::new("No active API tokens.").color(theme::TEXT_MUTED).italics());
                    } else {
                        let mut revoke_jti: Option<String> = None;
                        for (i, tok) in api_tokens.iter().enumerate() {
                            let row_bg = if i % 2 == 0 { theme::BG_PEER_ROW_EVEN } else { theme::BG_PEER_ROW_ODD };
                            egui::Frame::NONE.fill(row_bg).inner_margin(egui::Margin::same(4)).show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    let role_color = match tok.max_role.as_str() {
                                        "manager" => theme::TEXT_ROLE_MANAGER,
                                        "editor" => theme::TEXT_ROLE_EDITOR,
                                        _ => theme::TEXT_ROLE_VIEWER,
                                    };
                                    ui.label(egui::RichText::new(&tok.max_role).color(role_color).strong());
                                    ui.label(egui::RichText::new(&tok.scope).monospace().small().color(theme::TEXT_DIM));

                                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                        if ui.add(egui::Button::new("Revoke").fill(theme::BG_DANGER).small()).clicked() {
                                            revoke_jti = Some(tok.jti.clone());
                                        }
                                        ui.label(egui::RichText::new(format!("exp: {}", &tok.expires_at[..10.min(tok.expires_at.len())])).small().color(theme::TEXT_MUTED));
                                    });
                                });
                            });
                        }
                        if let Some(jti) = revoke_jti {
                            db_tx.send(DbCommand::RevokeApiToken { jti }).ok();
                            // Request refresh
                            db_tx.send(DbCommand::ListApiTokens).ok();
                        }
                    }

                    // --- Admin: All tokens scoped to this entity ---
                    ui.add_space(12.0);
                    ui.separator();
                    ui.add_space(8.0);

                    let scope_label = match entity_type {
                        EntitySettingsType::Verse => "Verse",
                        EntitySettingsType::Fractal => "Fractal",
                        EntitySettingsType::Petal => "Petal",
                    };
                    ui.label(egui::RichText::new(format!("All API Tokens in this {}", scope_label)).color(theme::TEXT_SECTION).strong());
                    ui.add_space(2.0);
                    ui.label(egui::RichText::new(
                        format!("Tokens scoped to this {} and all sub-entities.", scope_label.to_lowercase())
                    ).small().color(theme::TEXT_MUTED).italics());
                    ui.add_space(4.0);

                    if *scoped_tokens_loading && scoped_api_tokens.is_empty() {
                        ui.label(egui::RichText::new("Loading scoped tokens...").color(theme::TEXT_MUTED).italics());
                    } else if scoped_api_tokens.is_empty() {
                        ui.label(egui::RichText::new("No active API tokens in this scope.").color(theme::TEXT_MUTED).italics());
                    } else {
                        ui.label(egui::RichText::new(format!("{} active token(s)", scoped_api_tokens.len())).small().color(theme::TEXT_MUTED));
                        ui.add_space(4.0);
                        let mut revoke_scoped_jti: Option<String> = None;
                        for (i, tok) in scoped_api_tokens.iter().enumerate() {
                            let row_bg = if i % 2 == 0 { theme::BG_PEER_ROW_EVEN } else { theme::BG_PEER_ROW_ODD };
                            egui::Frame::NONE.fill(row_bg).inner_margin(egui::Margin::same(4)).show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    let role_color = match tok.max_role.as_str() {
                                        "manager" => theme::TEXT_ROLE_MANAGER,
                                        "editor" => theme::TEXT_ROLE_EDITOR,
                                        _ => theme::TEXT_ROLE_VIEWER,
                                    };
                                    ui.label(egui::RichText::new(&tok.max_role).color(role_color).strong());
                                    ui.label(egui::RichText::new(&tok.scope).monospace().small().color(theme::TEXT_DIM));

                                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                        if ui.add(egui::Button::new("Revoke").fill(theme::BG_DANGER).small()).clicked() {
                                            revoke_scoped_jti = Some(tok.jti.clone());
                                        }
                                        ui.label(egui::RichText::new(format!("exp: {}", &tok.expires_at[..10.min(tok.expires_at.len())])).small().color(theme::TEXT_MUTED));
                                        if let Some(ref lbl) = tok.label {
                                            ui.label(egui::RichText::new(lbl).small().color(theme::TEXT_BRIGHT));
                                        }
                                    });
                                });
                            });
                        }
                        if let Some(jti) = revoke_scoped_jti {
                            db_tx.send(DbCommand::RevokeApiToken { jti }).ok();
                            // Refresh both lists
                            db_tx.send(DbCommand::ListApiTokens).ok();
                            let scope_prefix = build_entity_scope(entity_type, entity_id, parent_verse_id, parent_fractal_id);
                            db_tx.send(DbCommand::ListApiTokensByScope { scope_prefix }).ok();
                        }
                    }
                }
            }
        });

    if !still_open || close {
        ui_mgr.close_dialog();
    }
}
