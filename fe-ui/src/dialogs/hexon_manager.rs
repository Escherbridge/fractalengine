//! Map Manager (hexon package UI): installed / available / downloads tabs.
//!
//! FR-2 (shell_ux_sidebar_20260725, D-A10): rendered as a one-at-a-time
//! RIGHT-SIDEBAR section (not a floating modal). `ActiveDialog::HexonManager`
//! is retained ONLY as the state carrier (its fields are populated by the
//! non-owned sync/bridge writers — `terrain_map/events.rs`, `actions/hexon.rs`,
//! `terrain_bridge.rs`); `ui_shell::right_sidebar` owns the section lifecycle
//! (self-seed on open, clear on close/switch). Returns `true` when the user
//! requested close so the section manager can drop both the carrier and the
//! `RightSidebarSection::Maps` request. See `dialogs/AGENTS.md` §hexon-manager.

use bevy_egui::egui;

use crate::accessibility::ResponseExt;

use super::ActiveDialog;
use crate::actions::{UiAction, UiManager};
use crate::panels::widgets::{clipped_value, confirm_button_named, role_denial_reason, role_gated};
use crate::plugin::LocalUserRole;
use crate::terrain_map::dto::{
    AvailableTilesetDto, DownloadStatus, HexonManagerTab, InstalledTilesetDto,
};
use crate::terrain_map::PetalMapState;
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

/// `local_role` is the live `LocalUserRole` resource (threaded by
/// `ui_shell::right_sidebar::render_maps_section`) — Set/Clear/Remove gate on
/// it (X-02); the server-side `fe-policy` check on `SetPetalTerrain` stays
/// authoritative regardless of what this control shows (ui_ux §3 "gating is
/// advisory only").
pub fn render_hexon_manager(
    ctx: &egui::Context,
    section_state: &mut crate::ui_shell::right_sidebar::RightSidebarState,
    ui_mgr: &mut UiManager,
    petal_map: &mut PetalMapState,
    active_petal_id: Option<&str>,
    active_verse_id: Option<&str>,
    downloads: &crate::terrain_map::dto::DownloadProgressMap,
    registry_url: Option<&str>,
    local_role: &LocalUserRole,
) -> bool {
    let file_dialogs = ui_mgr.file_dialogs.clone();
    let mut dialog_failure = None;
    let ActiveDialog::HexonManager {
        ref mut installed_tilesets,
        ref mut available_tilesets,
        ref mut filter_text,
        ref mut active_tab,
        ref storage_info,
        ref loading,
        // M-04: Remove now uses `widgets::confirm_button`, which owns its own
        // arm/disarm state in egui temp memory — this dialog no longer needs
        // a resource-level pending id. The `ActiveDialog::HexonManager` field
        // itself is owned by `dialogs/mod.rs` (outside this slice), so it
        // stays in the struct unused rather than being removed here.
        pending_remove: ref mut _pending_remove,
        ref mut registry_uri_buf,
    } = ui_mgr.active_dialog
    else {
        return false;
    };

    let current_tab = *active_tab;
    let is_loading = *loading;
    let mut close = false;
    let mut actions: Vec<UiAction> = Vec::new();

    crate::ui_shell::right_sidebar::section_chrome(ctx, section_state,
        crate::ui_shell::right_sidebar::RightSidebarSection::Maps, |ui| {
            // Top bar: search + install from file
            ui.vertical(|ui| {
                ui.add(
                    egui::TextEdit::singleline(filter_text)
                        .hint_text("Search maps...")
                        .desired_width(ui.available_width()),
                )
                .named(ui, "Search installed maps");

                ui.horizontal_wrapped(|ui| {
                    if ui
                        .add(egui::Button::new("Close").fill(theme::BG_BUTTON)).observed(ui)
                        .clicked()
                    {
                        close = true;
                    }
                    if ui
                        .add(egui::Button::new("Refresh").fill(theme::BG_BUTTON)).observed(ui)
                        .clicked()
                    {
                        actions.push(UiAction::HexonRefreshList);
                    }
                    if ui
                        .add(egui::Button::new("Install from file...").fill(theme::BG_SAVE)).observed(ui)
                        .clicked()
                    {
                        match file_dialogs.choose(crate::file_dialogs::FileDialogRequest {
                            kind: fe_runtime::qa_debug::DialogKind::Open,
                            verse_id: active_verse_id,
                            filters: &[("Hexon archive", &["hexon"])],
                            suggested_name: None,
                        }) {
                            Ok(Some(path)) => actions.push(UiAction::HexonInstallFromFile(path)),
                            Ok(None) => {}
                            Err(error) => dialog_failure = Some(error.to_string()),
                        }
                    }
                });
            });

            ui.add_space(8.0);

            // "Install from registry" (PAS-P5 S7b, A6/A9): disabled with a
            // calm hint when no registry is configured, rather than a faked
            // control (ui_ux §6) — `AppSettings.hexon_registry_url` is the
            // only source of `registry_url`, so this control is the field's
            // real, live consumer.
            ui.vertical(|ui| {
                ui.add_enabled_ui(registry_url.is_some(), |ui| {
                    ui.add(
                        egui::TextEdit::singleline(registry_uri_buf)
                            .hint_text("hexon_id or hexon_id@version")
                            .desired_width((ui.available_width() - 120.0).max(64.0)),
                    )
                    .named(ui, "Registry map ID and optional version");
                    let can_install = registry_url.is_some() && !registry_uri_buf.trim().is_empty();
                    if ui
                        .add_enabled(
                            can_install,
                            egui::Button::new("Install from registry").fill(theme::BG_SAVE),
                        ).observed(ui)
                        .clicked()
                    {
                        actions.push(UiAction::HexonInstallFromRegistry(
                            registry_uri_buf.trim().to_string(),
                        ));
                        registry_uri_buf.clear();
                    }
                });
                if registry_url.is_none() {
                    ui.label(
                        egui::RichText::new("Set a registry URL in Settings → Network")
                            .small()
                            .color(theme::TEXT_MUTED),
                    );
                }
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
                            egui::Button::new(egui::RichText::new(label).color(theme::TEXT_BRIGHT))
                                .fill(fill),
                        ).observed(ui)
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
                        render_installed_tab(
                            ui,
                            installed_tilesets,
                            &filter_lower,
                            &mut actions,
                            petal_map,
                            active_petal_id,
                            local_role,
                        );
                    }
                    HexonManagerTab::Available => {
                        render_available_tab(ui, available_tilesets, &filter_lower, &mut actions);
                    }
                    HexonManagerTab::Downloads => {
                        render_downloads_tab(ui, downloads, &mut actions);
                    }
                }
            }

            // Storage summary footer
            ui.add_space(8.0);
            ui.separator();
            ui.add_space(4.0);
            ui.label(egui::RichText::new(format!("{} maps, {} total", storage_info.count, format_size(storage_info.total_bytes))).small().color(theme::TEXT_MUTED));
            clipped_value(ui, &storage_info.base_dir, theme::TEXT_DIM);
            if ui.button("Open folder").observed(ui).clicked() { actions.push(UiAction::HexonOpenStorageDir); }

        });

    // Drain collected actions
    if let Some(error) = dialog_failure {
        ui_mgr.show_error(error, ctx.input(|i| i.time));
    }
    for action in actions {
        ui_mgr.push_action(action);
    }

    // The section manager (`ui_shell::right_sidebar`) owns tear-down: drop the
    // carrier AND the `Maps` section request when the user clicks Close.
    close
}

/// M-01: a 440 px side panel can't fit a 7-column `Grid` (Map/Actions clip at
/// default width — the two columns the user actually clicks). One card per
/// map, with a wrapping header line plus an action row, has no column widths
/// to clip.
fn render_installed_tab(
    ui: &mut egui::Ui,
    tilesets: &mut [InstalledTilesetDto],
    filter: &str,
    actions: &mut Vec<UiAction>,
    petal_map: &mut PetalMapState,
    active_petal_id: Option<&str>,
    local_role: &LocalUserRole,
) {
    // Scale controls for the petal's active map (above the tileset cards).
    if let Some(pid) = active_petal_id {
        if let Some(active_ts) = tilesets
            .iter()
            .find(|ts| petal_map.tileset_ids.iter().any(|id| id == &ts.hexon_id))
            .cloned()
        {
            render_scale_controls(ui, &active_ts, petal_map, actions, pid);
            ui.add_space(8.0);
            ui.separator();
            ui.add_space(8.0);
        }
    }

    if tilesets.is_empty() {
        ui.label(
            egui::RichText::new(
                "No maps installed. Use \"Install from file...\" or download from the Available tab.",
            )
            .color(theme::TEXT_MUTED),
        );
        return;
    }

    egui::ScrollArea::vertical()
        .auto_shrink(false)
        .show(ui, |ui| {
            for ts in tilesets.iter_mut() {
                if !filter.is_empty() && !ts.region_name.to_lowercase().contains(filter) {
                    continue;
                }
                render_installed_map_card(ui, ts, actions, petal_map, active_petal_id, local_role);
                ui.add_space(6.0);
            }
        });
}

/// One installed-map card: a clipped/copyable region name + seeding toggle
/// on the header line, a plain stats line, then the Map and Remove actions —
/// see `render_installed_tab`'s doc comment (M-01).
fn render_installed_map_card(
    ui: &mut egui::Ui,
    ts: &mut InstalledTilesetDto,
    actions: &mut Vec<UiAction>,
    petal_map: &mut PetalMapState,
    active_petal_id: Option<&str>,
    local_role: &LocalUserRole,
) {
    egui::Frame::NONE
        .fill(theme::BG_BUTTON)
        .inner_margin(egui::Margin::same(8))
        .corner_radius(4.0)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                clipped_value(ui, &ts.region_name, theme::TEXT_BRIGHT);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let mut seeding = ts.seeding_enabled;
                    if ui
                        .checkbox(&mut seeding, "Seed")
                        .named(ui, format_args!("Seed map {}", ts.region_name))
                        .on_hover_text("Allow other peers to download tiles from your copy")
                        .changed()
                    {
                        ts.seeding_enabled = seeding;
                        actions.push(UiAction::HexonToggleSeeding(ts.hexon_id.clone(), seeding));
                    }
                });
            });
            ui.label(
                egui::RichText::new(format!(
                    "Zoom {}-{} \u{00b7} {} tiles \u{00b7} {}",
                    ts.zoom_range.0,
                    ts.zoom_range.1,
                    ts.tile_count,
                    format_size(ts.size_bytes),
                ))
                .small()
                .color(theme::TEXT_MUTED),
            );
            ui.add_space(4.0);

            ui.horizontal(|ui| {
                // M-03: while THIS petal has a correlated set in flight, show
                // "Applying…" in place of the optimistic "Active map ✓" so a
                // denial (handled by `handle_petal_terrain_set_failed`) never
                // looks like it already landed (ui_ux §6).
                let is_active_map = petal_map.tileset_ids.iter().any(|id| id == &ts.hexon_id);
                if is_active_map && petal_map.pending_set.is_some() {
                    ui.add_enabled(
                        false,
                        egui::Button::new("Applying\u{2026}").fill(theme::BG_BUTTON),
                    )
                    .named(ui, format_args!("Applying map {}", ts.region_name));
                } else if is_active_map {
                    let reason = map_action_denial_reason(local_role, active_petal_id);
                    let resp = role_gated(
                        ui,
                        local_role.can_edit(),
                        &reason,
                        egui::Button::new(
                            egui::RichText::new("Active map \u{2713}").color(theme::TEXT_BRIGHT),
                        )
                        .fill(theme::BG_BUTTON_ACTIVE),
                    )
                    .named(ui, format_args!("Clear petal map {}", ts.region_name))
                    .on_hover_text("Click to clear this petal's map");
                    if resp.clicked() {
                        if let Some(pid) = active_petal_id {
                            actions.push(UiAction::PetalSetMap {
                                petal_id: pid.to_string(),
                                tileset: None,
                            });
                        }
                    }
                } else {
                    let reason = map_action_denial_reason(local_role, active_petal_id);
                    let resp = role_gated(
                        ui,
                        local_role.can_edit() && active_petal_id.is_some(),
                        &reason,
                        egui::Button::new("Set petal map").fill(theme::BG_BUTTON),
                    )
                    .named(ui, format_args!("Set petal map to {}", ts.region_name));
                    if resp.clicked() {
                        if let Some(pid) = active_petal_id {
                            actions.push(UiAction::PetalSetMap {
                                petal_id: pid.to_string(),
                                tileset: Some(ts.clone()),
                            });
                        }
                    }
                }

                // M-04: "Remove" → "Confirm remove" (two-step), gated the same
                // way as Set/Clear above (X-02). A disabled scope around the
                // one `confirm_button` keeps the denied look identical to the
                // allowed one instead of a hand-styled lookalike.
                let can_remove = local_role.can_edit();
                let remove = ui.add_enabled_ui(can_remove, |ui| {
                    confirm_button_named(
                        ui,
                        ("hexon-remove", ts.hexon_id.as_str()),
                        "Remove",
                        "Confirm remove",
                        format_args!("Remove map {}", ts.region_name),
                    )
                });
                if !can_remove {
                    remove
                        .response
                        .on_disabled_hover_text(role_denial_reason(local_role, "Editor", None));
                }
                if remove.inner {
                    actions.push(UiAction::HexonRemoveTileset(ts.hexon_id.clone()));
                }
            });
        });
}

/// The Map action's disabled-hover reason: "navigate to a petal first" when
/// there is no active petal (not a role issue), otherwise the role denial.
fn map_action_denial_reason(local_role: &LocalUserRole, active_petal_id: Option<&str>) -> String {
    if active_petal_id.is_none() {
        "Navigate to a petal first".to_string()
    } else if !local_role.can_edit() {
        role_denial_reason(local_role, "Editor", None)
    } else {
        String::new()
    }
}

/// World-scale presets: (label, world units per real meter). `1:N` → scale `1/N`.
const SCALE_PRESETS: &[(&str, f64)] = &[
    ("1:1 human", 1.0),
    ("1:10", 0.1),
    ("1:100", 0.01),
    ("1:1000 region", 0.001),
];

/// Minimum / maximum world scale for the log slider (1:10000 .. 1:1).
const SCALE_MIN: f64 = 0.0001;
const SCALE_MAX: f64 = 1.0;

/// Terrain world-scale settings for the active petal's map: presets + a log
/// slider. Writes `petal_map.world_scale` for a live camera preview and emits
/// `PetalSetMapScale` (persist + respawn) on release / preset click.
fn render_scale_controls(
    ui: &mut egui::Ui,
    active_ts: &InstalledTilesetDto,
    petal_map: &mut PetalMapState,
    actions: &mut Vec<UiAction>,
    petal_id: &str,
) {
    // FR-6: the hexon's declared scale_bounds (surfaced via terrain_json) win
    // over the generic slider range when present — the slider cannot exceed
    // the hexon-authoritative clamp (C1).
    let (slider_min, slider_max) = petal_map
        .scale_bounds
        .filter(|[min, max]| min.is_finite() && max.is_finite() && *min > 0.0 && max >= min)
        .map(|[min, max]| (min, max))
        .unwrap_or((SCALE_MIN, SCALE_MAX));

    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new("Terrain scale")
                .color(theme::TEXT_SECTION)
                .strong(),
        );
        let ratio = if petal_map.world_scale > 0.0 {
            (1.0 / petal_map.world_scale).round() as i64
        } else {
            1
        };
        ui.label(
            egui::RichText::new(format!("1:{ratio}"))
                .small()
                .color(theme::TEXT_MUTED),
        )
        .on_hover_text("World units per real meter (region ↔ human scale of space)");
        if petal_map.scale_bounds.is_some() {
            ui.label(
                egui::RichText::new("(map-bounded)")
                    .small()
                    .color(theme::TEXT_DIM),
            );
        }
    });

    ui.horizontal(|ui| {
        for (label, preset) in SCALE_PRESETS {
            let clamped_preset = preset.clamp(slider_min, slider_max);
            let selected = (petal_map.world_scale - clamped_preset).abs() < 1e-9;
            let fill = if selected {
                theme::BG_BUTTON_ACTIVE
            } else {
                theme::BG_BUTTON
            };
            if ui
                .add(egui::Button::new(*label).fill(fill))
                .named(ui, format_args!("Set terrain scale to {label}"))
                .clicked()
            {
                petal_map.world_scale = clamped_preset;
                actions.push(UiAction::PetalSetMapScale {
                    petal_id: petal_id.to_string(),
                    tileset: active_ts.clone(),
                    world_scale: clamped_preset,
                });
            }
        }
    });

    // Log slider: live-preview the camera on drag; persist terrain on release.
    let mut scale = petal_map.world_scale.clamp(slider_min, slider_max);
    let resp = ui
        .add(
            egui::Slider::new(&mut scale, slider_min..=slider_max)
                .logarithmic(true)
                .custom_formatter(|v, _| format!("1:{}", (1.0 / v).round() as i64))
                .text("scale"),
        )
        .named(ui, "Terrain world scale");
    if resp.changed() {
        petal_map.world_scale = scale;
    }
    if resp.drag_stopped() || (resp.changed() && !resp.dragged()) {
        actions.push(UiAction::PetalSetMapScale {
            petal_id: petal_id.to_string(),
            tileset: active_ts.clone(),
            world_scale: scale,
        });
    }
}

fn render_available_tab(
    ui: &mut egui::Ui,
    tilesets: &[AvailableTilesetDto],
    filter: &str,
    actions: &mut Vec<UiAction>,
) {
    if tilesets.is_empty() {
        ui.label(
            egui::RichText::new(
                "No peers connected. Available maps will appear when you connect to peers sharing terrain data.",
            )
            .color(theme::TEXT_MUTED),
        );
        return;
    }

    egui::ScrollArea::vertical()
        .auto_shrink(false)
        .show(ui, |ui| {
            egui::Grid::new("available_header")
                .num_columns(6)
                .striped(false)
                .spacing([12.0, 4.0])
                .show(ui, |ui| {
                    ui.label(
                        egui::RichText::new("Region")
                            .color(theme::TEXT_SECTION)
                            .strong(),
                    );
                    ui.label(
                        egui::RichText::new("Zoom")
                            .color(theme::TEXT_SECTION)
                            .strong(),
                    );
                    ui.label(
                        egui::RichText::new("Tiles")
                            .color(theme::TEXT_SECTION)
                            .strong(),
                    );
                    ui.label(
                        egui::RichText::new("Est. Size")
                            .color(theme::TEXT_SECTION)
                            .strong(),
                    );
                    ui.label(
                        egui::RichText::new("Peers")
                            .color(theme::TEXT_SECTION)
                            .strong(),
                    );
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
                            .named(ui, format_args!("Download map {}", ts.region_name))
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
    downloads: &crate::terrain_map::dto::DownloadProgressMap,
    actions: &mut Vec<UiAction>,
) {
    if downloads.is_empty() {
        ui.label(egui::RichText::new("No active downloads.").color(theme::TEXT_MUTED));
        return;
    }

    egui::ScrollArea::vertical()
        .auto_shrink(false)
        .show(ui, |ui| {
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
                                        egui::RichText::new("Queued").color(theme::TEXT_MUTED),
                                    );
                                }
                                DownloadStatus::Downloading => {
                                    if ui
                                        .add(egui::Button::new("Cancel").fill(theme::BG_DANGER))
                                        .named(
                                            ui,
                                            format_args!(
                                                "Cancel download of map {}",
                                                dl.tileset_id
                                            ),
                                        )
                                        .clicked()
                                    {
                                        actions.push(UiAction::HexonCancelDownload(id.clone()));
                                    }
                                }
                                DownloadStatus::Verifying => {
                                    ui.label(
                                        egui::RichText::new("Verifying...").color(theme::TEXT_DIM),
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
