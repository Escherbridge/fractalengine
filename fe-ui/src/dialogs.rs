use bevy_egui::egui;

use crate::navigation_manager::NavigationManager;
use crate::plugin::{ActiveDialog, CreateKind, UiManager};
use crate::theme;
use crate::verse_manager::VerseManager;
use fe_runtime::messages::DbCommand;

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
// Phase F: Invite dialog
// ---------------------------------------------------------------------------

pub fn render_invite_dialog(ctx: &egui::Context, ui_mgr: &mut UiManager) {
    let ActiveDialog::InviteDialog {
        ref invite_string, ..
    } = ui_mgr.active_dialog
    else {
        return;
    };

    let invite_str_clone = invite_string.clone();
    let mut close = false;

    egui::Window::new("Verse Invite")
        .collapsible(false)
        .resizable(true)
        .default_width(400.0)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.label("Share this invite string with others to let them join your verse:");
            ui.add_space(4.0);

            // Copyable text field with the invite string
            let mut display_str = invite_str_clone.clone();
            ui.add(
                egui::TextEdit::multiline(&mut display_str)
                    .desired_rows(3)
                    .desired_width(f32::INFINITY)
                    .font(egui::TextStyle::Monospace),
            );

            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui.button("Copy to Clipboard").clicked() {
                    ui.ctx().copy_text(invite_str_clone.clone());
                }
                if ui.button("Close").clicked() {
                    close = true;
                }
            });
        });

    if close {
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
