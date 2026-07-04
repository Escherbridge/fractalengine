use bevy_egui::egui;

use crate::plugin::{
    ActiveDialog, DownloadStatus, HexonManagerTab, UiAction, UiManager,
};
use crate::theme;

/// Format bytes into human-readable size string.
fn format_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{} B", bytes)
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else if bytes < 1024 * 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.2} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    }
}

pub fn render_hexon_manager(
    ctx: &egui::Context,
    ui_mgr: &mut UiManager,
    petal_map: &crate::plugin::PetalMapState,
    active_petal_id: Option<&str>,
) {
    let ActiveDialog::HexonManager {
        ref mut installed_tilesets,
        ref mut available_tilesets,
        ref mut download_progress,
        ref mut filter_text,
        ref mut active_tab,
        ref storage_info,
        ref loading,
        ref mut pending_remove,
    } = ui_mgr.active_dialog
    else {
        return;
    };

    let current_tab = *active_tab;
    let is_loading = *loading;
    let close = false;
    let mut still_open = true;
    let mut actions: Vec<UiAction> = Vec::new();

    egui::Window::new("Hexon Manager")
        .open(&mut still_open)
        .collapsible(false)
        .resizable(true)
        .default_width(800.0)
        .default_height(600.0)
        .min_width(600.0)
        .min_height(400.0)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .frame(
            egui::Frame::NONE
                .fill(theme::BG_DIALOG)
                .inner_margin(egui::Margin::same(16))
                .corner_radius(6.0)
                .stroke(egui::Stroke::new(1.0, theme::TEXT_DIM)),
        )
        .show(ctx, |ui| {
            // Top bar: search + install from file
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(filter_text)
                        .hint_text("Search tilesets...")
                        .desired_width(300.0),
                );

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add(egui::Button::new("Install from file...").fill(theme::BG_SAVE))
                        .clicked()
                    {
                        if let Some(path) = rfd::FileDialog::new()
                            .add_filter("Hexon archive", &["hexon"])
                            .pick_file()
                        {
                            actions.push(UiAction::HexonInstallFromFile(path));
                        }
                    }
                    if ui
                        .add(egui::Button::new("Refresh").fill(theme::BG_BUTTON))
                        .clicked()
                    {
                        actions.push(UiAction::HexonRefreshList);
                    }
                });
            });

            ui.add_space(8.0);

            // Tab bar
            ui.horizontal(|ui| {
                for (tab, label) in [
                    (HexonManagerTab::Installed, "Installed"),
                    (HexonManagerTab::Available, "Available"),
                    (HexonManagerTab::Downloads, "Downloads"),
                ] {
                    let fill = if current_tab == tab {
                        theme::BG_TAB_ACTIVE
                    } else {
                        theme::BG_TAB_INACTIVE
                    };
                    if ui
                        .add(
                            egui::Button::new(
                                egui::RichText::new(label).color(theme::TEXT_BRIGHT),
                            )
                            .fill(fill),
                        )
                        .clicked()
                    {
                        *active_tab = tab;
                    }
                }
            });

            ui.add_space(8.0);
            ui.separator();
            ui.add_space(8.0);

            if is_loading {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Loading...");
                });
            } else {
                let filter_lower = filter_text.to_lowercase();

                match current_tab {
                    HexonManagerTab::Installed => {
                        render_installed_tab(ui, installed_tilesets, &filter_lower, &mut actions, pending_remove, petal_map, active_petal_id);
                    }
                    HexonManagerTab::Available => {
                        render_available_tab(ui, available_tilesets, &filter_lower, &mut actions);
                    }
                    HexonManagerTab::Downloads => {
                        render_downloads_tab(ui, download_progress, &mut actions);
                    }
                }
            }

            // Storage summary footer
            ui.add_space(8.0);
            ui.separator();
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(format!(
                        "{} tilesets, {} total",
                        storage_info.count,
                        format_size(storage_info.total_bytes),
                    ))
                    .small()
                    .color(theme::TEXT_MUTED),
                );
                ui.separator();
                ui.label(
                    egui::RichText::new(&storage_info.base_dir)
                        .small()
                        .color(theme::TEXT_DIM),
                );
                if ui
                    .add(
                        egui::Button::new(
                            egui::RichText::new("Open folder").small(),
                        )
                        .fill(theme::BG_BUTTON),
                    )
                    .clicked()
                {
                    actions.push(UiAction::HexonOpenStorageDir);
                }
            });
        });

    // Drain collected actions
    for action in actions {
        ui_mgr.push_action(action);
    }

    if !still_open || close {
        ui_mgr.close_dialog();
    }
}

fn render_installed_tab(
    ui: &mut egui::Ui,
    tilesets: &mut [crate::plugin::InstalledTilesetDto],
    filter: &str,
    actions: &mut Vec<UiAction>,
    pending_remove: &mut Option<String>,
    petal_map: &crate::plugin::PetalMapState,
    active_petal_id: Option<&str>,
) {
    if tilesets.is_empty() {
        ui.label(
            egui::RichText::new("No tilesets installed. Use \"Install from file...\" or download from the Available tab.")
                .color(theme::TEXT_MUTED),
        );
        return;
    }

    egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
        // Header row
        egui::Grid::new("installed_header")
            .num_columns(7)
            .striped(false)
            .spacing([12.0, 4.0])
            .show(ui, |ui| {
                ui.label(egui::RichText::new("Region").color(theme::TEXT_SECTION).strong());
                ui.label(egui::RichText::new("Zoom").color(theme::TEXT_SECTION).strong());
                ui.label(egui::RichText::new("Tiles").color(theme::TEXT_SECTION).strong());
                ui.label(egui::RichText::new("Size").color(theme::TEXT_SECTION).strong());
                ui.label(egui::RichText::new("Seeding").color(theme::TEXT_SECTION).strong());
                ui.label(egui::RichText::new("Map").color(theme::TEXT_SECTION).strong());
                ui.label(egui::RichText::new("Actions").color(theme::TEXT_SECTION).strong());
                ui.end_row();
            });

        ui.separator();

        egui::Grid::new("installed_grid")
            .num_columns(7)
            .striped(true)
            .spacing([12.0, 6.0])
            .show(ui, |ui| {
                for ts in tilesets.iter_mut() {
                    if !filter.is_empty() && !ts.region_name.to_lowercase().contains(filter) {
                        continue;
                    }

                    ui.label(&ts.region_name);
                    ui.label(format!("{}-{}", ts.zoom_range.0, ts.zoom_range.1));
                    ui.label(ts.tile_count.to_string());
                    ui.label(format_size(ts.size_bytes));

                    // Seeding toggle
                    let mut seeding = ts.seeding_enabled;
                    if ui
                        .checkbox(&mut seeding, "")
                        .on_hover_text("Allow other peers to download tiles from your copy")
                        .changed()
                    {
                        ts.seeding_enabled = seeding;
                        actions.push(UiAction::HexonToggleSeeding(
                            ts.hexon_id.clone(),
                            seeding,
                        ));
                    }

                    // Map column: set/clear this tileset as the active petal's map.
                    let is_active_map = petal_map.tileset_ids.iter().any(|id| id == &ts.hexon_id);
                    if is_active_map {
                        if ui
                            .add(
                                egui::Button::new(
                                    egui::RichText::new("Active map \u{2713}").color(theme::TEXT_BRIGHT),
                                )
                                .fill(theme::BG_BUTTON_ACTIVE),
                            )
                            .on_hover_text("Click to clear this petal's map")
                            .clicked()
                        {
                            if let Some(pid) = active_petal_id {
                                actions.push(UiAction::PetalSetMap {
                                    petal_id: pid.to_string(),
                                    tileset: None,
                                });
                            }
                        }
                    } else {
                        let response = ui
                            .add_enabled(
                                active_petal_id.is_some(),
                                egui::Button::new("Set petal map").fill(theme::BG_BUTTON),
                            )
                            .on_disabled_hover_text("Navigate to a petal first");
                        if response.clicked() {
                            if let Some(pid) = active_petal_id {
                                actions.push(UiAction::PetalSetMap {
                                    petal_id: pid.to_string(),
                                    tileset: Some(ts.clone()),
                                });
                            }
                        }
                    }

                    // Remove button with confirmation
                    let is_pending = pending_remove.as_ref() == Some(&ts.hexon_id);
                    if is_pending {
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new(format!(
                                    "Remove {}? ({} freed)",
                                    ts.region_name,
                                    format_size(ts.size_bytes),
                                ))
                                .small()
                                .color(theme::TEXT_MUTED),
                            );
                            if ui
                                .add(
                                    egui::Button::new("Yes")
                                        .fill(theme::BG_DANGER),
                                )
                                .clicked()
                            {
                                actions.push(UiAction::HexonRemoveTileset(ts.hexon_id.clone()));
                                *pending_remove = None;
                            }
                            if ui
                                .add(egui::Button::new("No").fill(theme::BG_BUTTON))
                                .clicked()
                            {
                                *pending_remove = None;
                            }
                        });
                    } else if ui
                        .add(
                            egui::Button::new(
                                egui::RichText::new("Remove").color(egui::Color32::WHITE),
                            )
                            .fill(theme::BG_DANGER),
                        )
                        .clicked()
                    {
                        *pending_remove = Some(ts.hexon_id.clone());
                    }

                    ui.end_row();
                }
            });
    });
}

fn render_available_tab(
    ui: &mut egui::Ui,
    tilesets: &[crate::plugin::AvailableTilesetDto],
    filter: &str,
    actions: &mut Vec<UiAction>,
) {
    if tilesets.is_empty() {
        ui.label(
            egui::RichText::new("No peers connected. Available tilesets will appear when you connect to peers sharing terrain data.")
                .color(theme::TEXT_MUTED),
        );
        return;
    }

    egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
        egui::Grid::new("available_header")
            .num_columns(6)
            .striped(false)
            .spacing([12.0, 4.0])
            .show(ui, |ui| {
                ui.label(egui::RichText::new("Region").color(theme::TEXT_SECTION).strong());
                ui.label(egui::RichText::new("Zoom").color(theme::TEXT_SECTION).strong());
                ui.label(egui::RichText::new("Tiles").color(theme::TEXT_SECTION).strong());
                ui.label(egui::RichText::new("Est. Size").color(theme::TEXT_SECTION).strong());
                ui.label(egui::RichText::new("Peers").color(theme::TEXT_SECTION).strong());
                ui.label(egui::RichText::new("").color(theme::TEXT_SECTION).strong());
                ui.end_row();
            });

        ui.separator();

        egui::Grid::new("available_grid")
            .num_columns(6)
            .striped(true)
            .spacing([12.0, 6.0])
            .show(ui, |ui| {
                for ts in tilesets {
                    if !filter.is_empty() && !ts.region_name.to_lowercase().contains(filter) {
                        continue;
                    }

                    ui.label(&ts.region_name);
                    ui.label(format!("{}-{}", ts.zoom_range.0, ts.zoom_range.1));
                    ui.label(ts.tile_count.to_string());
                    ui.label(format_size(ts.approx_size_bytes));
                    ui.label(ts.peer_count.to_string());

                    if ts.already_installed {
                        ui.label(
                            egui::RichText::new("Installed")
                                .color(theme::TEXT_MUTED)
                                .italics(),
                        );
                    } else if ui
                        .add(egui::Button::new("Download").fill(theme::BG_SAVE))
                        .clicked()
                    {
                        actions.push(UiAction::HexonStartDownload(ts.hexon_id.clone()));
                    }

                    ui.end_row();
                }
            });
    });
}

fn render_downloads_tab(
    ui: &mut egui::Ui,
    downloads: &std::collections::HashMap<String, crate::plugin::DownloadProgress>,
    actions: &mut Vec<UiAction>,
) {
    if downloads.is_empty() {
        ui.label(
            egui::RichText::new("No active downloads.")
                .color(theme::TEXT_MUTED),
        );
        return;
    }

    egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
        for (id, dl) in downloads {
            ui.group(|ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(&dl.tileset_id)
                            .color(theme::TEXT_BRIGHT)
                            .strong(),
                    );

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        match &dl.status {
                            DownloadStatus::Queued => {
                                ui.label(
                                    egui::RichText::new("Queued")
                                        .color(theme::TEXT_MUTED),
                                );
                            }
                            DownloadStatus::Downloading => {
                                if ui
                                    .add(
                                        egui::Button::new("Cancel")
                                            .fill(theme::BG_DANGER),
                                    )
                                    .clicked()
                                {
                                    actions.push(UiAction::HexonCancelDownload(id.clone()));
                                }
                            }
                            DownloadStatus::Verifying => {
                                ui.label(
                                    egui::RichText::new("Verifying...")
                                        .color(theme::TEXT_DIM),
                                );
                            }
                            DownloadStatus::Complete => {
                                ui.label(
                                    egui::RichText::new("Complete")
                                        .color(egui::Color32::from_rgb(100, 200, 100)),
                                );
                            }
                            DownloadStatus::Failed(msg) => {
                                ui.label(
                                    egui::RichText::new(format!("Failed: {}", msg))
                                        .color(egui::Color32::from_rgb(200, 80, 80)),
                                );
                            }
                        }
                    });
                });

                // Chunk progress
                if dl.total_chunks > 0 {
                    let pct = dl.chunks_received as f32 / dl.total_chunks as f32;
                    ui.add(egui::ProgressBar::new(pct).text(format!(
                        "{}/{} chunks ({:.0}%)",
                        dl.chunks_received,
                        dl.total_chunks,
                        pct * 100.0,
                    )));
                }

                // Byte progress
                if dl.total_bytes_estimate > 0 {
                    ui.label(
                        egui::RichText::new(format!(
                            "{} / {}",
                            format_size(dl.bytes_received),
                            format_size(dl.total_bytes_estimate),
                        ))
                        .small()
                        .color(theme::TEXT_DIM),
                    );
                }
            });

            ui.add_space(4.0);
        }
    });
}
