use bevy_egui::egui;

use crate::navigation_manager::NavigationManager;
use crate::plugin::{
    ContextMenuState, CreateDialogState, CreateKind, GltfImportState, InviteDialogState,
    JoinDialogState, NodeOptionsState, PeerDebugPanelState,
};
use crate::theme;
use crate::verse_manager::VerseManager;
use fe_runtime::messages::DbCommand;

// ---------------------------------------------------------------------------
// Context menu (viewport right-click)
// ---------------------------------------------------------------------------

pub fn render_context_menu(
    ctx: &egui::Context,
    context_menu: &mut ContextMenuState,
    gltf_import: &mut GltfImportState,
) {
    if !context_menu.open {
        return;
    }

    let pos = egui::pos2(context_menu.screen_pos[0], context_menu.screen_pos[1]);
    let world = context_menu.world_pos;

    egui::Area::new(egui::Id::new("viewport_context_menu"))
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
                        gltf_import.open = true;
                        gltf_import.position = world;
                        gltf_import.file_path_buf.clear();
                        gltf_import.name_buf.clear();
                        context_menu.open = false;
                    }

                    if ui
                        .add(egui::Button::new("Add Empty Node").fill(egui::Color32::TRANSPARENT))
                        .clicked()
                    {
                        // TODO(tracked): CreateEmptyNode at cursor world pos — requires DbCommand::CreateNodeAtPosition
                        context_menu.open = false;
                    }
                });
        });

    // Close on click elsewhere
    if ctx.input(|i| i.pointer.any_pressed()) {
        let ptr = ctx.input(|i| i.pointer.interact_pos());
        if let Some(ptr_pos) = ptr {
            let menu_rect = egui::Rect::from_min_size(pos, egui::vec2(160.0, 80.0));
            if !menu_rect.contains(ptr_pos) {
                context_menu.open = false;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Create dialog (Verse / Fractal / Petal / Node)
// ---------------------------------------------------------------------------

pub fn render_create_dialog(
    ctx: &egui::Context,
    create_dialog: &mut CreateDialogState,
    hierarchy: &mut VerseManager,
    nav: &mut NavigationManager,
    db_tx: &crossbeam::channel::Sender<DbCommand>,
) {
    if !create_dialog.open {
        return;
    }

    let title = match create_dialog.kind {
        CreateKind::Verse => "Create Verse",
        CreateKind::Fractal => "Create Fractal",
        CreateKind::Petal => "Create Petal",
        CreateKind::Node => "Create Node",
    };

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
                egui::TextEdit::singleline(&mut create_dialog.name_buf)
                    .hint_text("Enter name\u{2026}")
                    .desired_width(f32::INFINITY),
            );

            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui
                    .add(egui::Button::new("Create").fill(theme::BG_SAVE))
                    .clicked()
                {
                    let name = create_dialog.name_buf.trim().to_string();
                    if !name.is_empty() {
                        apply_create(
                            hierarchy,
                            nav,
                            create_dialog.kind,
                            &create_dialog.parent_id,
                            &name,
                            db_tx,
                        );
                        create_dialog.open = false;
                        create_dialog.name_buf.clear();
                    }
                }
                if ui
                    .add(egui::Button::new("Cancel").fill(theme::BG_BUTTON))
                    .clicked()
                {
                    create_dialog.open = false;
                    create_dialog.name_buf.clear();
                }
            });
        });

    if !still_open {
        create_dialog.open = false;
    }
}

/// Send a DB command to create the entity. The hierarchy will update when
/// the DbResult comes back — no local push to avoid duplicates.
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
    gltf_import: &mut GltfImportState,
    nav: &NavigationManager,
    db_tx: &crossbeam::channel::Sender<DbCommand>,
) {
    if !gltf_import.open {
        return;
    }

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
                    egui::TextEdit::singleline(&mut gltf_import.file_path_buf)
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
                        gltf_import.file_path_buf = path.display().to_string();
                        if gltf_import.name_buf.is_empty() {
                            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                                gltf_import.name_buf = stem.to_string();
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
                egui::TextEdit::singleline(&mut gltf_import.name_buf)
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
                for (axis, val) in ["X", "Y", "Z"].iter().zip(gltf_import.position.iter()) {
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
                    let file_path = gltf_import.file_path_buf.trim().to_string();
                    let name = gltf_import.name_buf.trim().to_string();
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
                                position: gltf_import.position,
                            })
                            .ok();
                    }
                    gltf_import.open = false;
                }
                if ui
                    .add(egui::Button::new("Cancel").fill(theme::BG_BUTTON))
                    .clicked()
                {
                    gltf_import.open = false;
                }
            });
        });

    if !still_open {
        gltf_import.open = false;
    }
}

// ---------------------------------------------------------------------------
// Phase F: Invite dialog
// ---------------------------------------------------------------------------

pub fn render_invite_dialog(ctx: &egui::Context, invite_dialog: &mut InviteDialogState) {
    if !invite_dialog.open {
        return;
    }

    egui::Window::new("Verse Invite")
        .collapsible(false)
        .resizable(true)
        .default_width(400.0)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.label("Share this invite string with others to let them join your verse:");
            ui.add_space(4.0);

            // Copyable text field with the invite string
            let mut invite_str = invite_dialog.invite_string.clone();
            ui.add(
                egui::TextEdit::multiline(&mut invite_str)
                    .desired_rows(3)
                    .desired_width(f32::INFINITY)
                    .font(egui::TextStyle::Monospace),
            );

            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui.button("Copy to Clipboard").clicked() {
                    ui.ctx().copy_text(invite_dialog.invite_string.clone());
                }
                if ui.button("Close").clicked() {
                    invite_dialog.open = false;
                    invite_dialog.invite_string.clear();
                }
            });
        });
}

// ---------------------------------------------------------------------------
// Phase F: Join dialog
// ---------------------------------------------------------------------------

pub fn render_join_dialog(
    ctx: &egui::Context,
    join_dialog: &mut JoinDialogState,
    db_tx: &crossbeam::channel::Sender<DbCommand>,
) {
    if !join_dialog.open {
        return;
    }

    egui::Window::new("Join Verse")
        .collapsible(false)
        .resizable(true)
        .default_width(400.0)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.label("Paste an invite string to join a verse:");
            ui.add_space(4.0);

            ui.add(
                egui::TextEdit::multiline(&mut join_dialog.invite_buf)
                    .desired_rows(3)
                    .desired_width(f32::INFINITY)
                    .hint_text("Paste invite string here...")
                    .font(egui::TextStyle::Monospace),
            );

            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let can_join = !join_dialog.invite_buf.trim().is_empty();
                if ui
                    .add_enabled(can_join, egui::Button::new("Join"))
                    .clicked()
                {
                    db_tx
                        .send(DbCommand::JoinVerseByInvite {
                            invite_string: join_dialog.invite_buf.trim().to_string(),
                        })
                        .ok();
                    join_dialog.open = false;
                    join_dialog.invite_buf.clear();
                }
                if ui.button("Cancel").clicked() {
                    join_dialog.open = false;
                    join_dialog.invite_buf.clear();
                }
            });
        });
}

// ---------------------------------------------------------------------------
// Phase F: Peer debug panel
// ---------------------------------------------------------------------------

pub fn render_peer_debug_panel(
    ctx: &egui::Context,
    peer_debug: &mut PeerDebugPanelState,
    sync_status: Option<&fe_sync::SyncStatus>,
) {
    if !peer_debug.open {
        return;
    }

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
                peer_debug.open = false;
            }
        });
}

// ---------------------------------------------------------------------------
// Node options dialog
// ---------------------------------------------------------------------------

pub fn render_node_options_dialog(
    ctx: &egui::Context,
    node_options: &mut NodeOptionsState,
    hierarchy: &mut VerseManager,
) {
    if !node_options.open {
        return;
    }

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
                egui::TextEdit::singleline(&mut node_options.node_name_buf)
                    .desired_width(f32::INFINITY),
            );

            ui.add_space(6.0);
            ui.label(
                egui::RichText::new("Webpage URL:")
                    .small()
                    .color(theme::TEXT_DIM),
            );
            ui.add(
                egui::TextEdit::singleline(&mut node_options.webpage_url_buf)
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
                    let node_id = node_options.node_id.clone();
                    let url = node_options.webpage_url_buf.trim().to_string();
                    let url_opt = if url.is_empty() { None } else { Some(url) };
                    node_options_save_url(hierarchy, &node_id, url_opt);
                    node_options.open = false;
                }
                if ui
                    .add(egui::Button::new("Cancel").fill(theme::BG_BUTTON))
                    .clicked()
                {
                    node_options.open = false;
                }
            });
        });

    if !still_open {
        node_options.open = false;
    }
}

pub fn node_options_save_url(
    hierarchy: &mut VerseManager,
    node_id: &str,
    url: Option<String>,
) {
    hierarchy.update_node_url(node_id, url);
}
