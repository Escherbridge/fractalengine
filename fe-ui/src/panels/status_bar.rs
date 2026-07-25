//! Bottom status bar: online/peer indicators, active verse, space counts, and
//! the persistent panel-guard error segment (Q-5, FR-7) — see
//! `ui_shell/AGENTS.md` §modal.

use bevy_egui::egui;

use crate::actions::UiManager;
use crate::atlas::DashboardState;
use crate::dialogs::ActiveDialog;
use crate::navigation_manager::NavigationManager;
use crate::theme;
use crate::ui_shell::modal::ModalManagerState;

pub(crate) fn status_bar(
    ctx: &egui::Context,
    dashboard: &DashboardState,
    sync_status: Option<&fe_sync::SyncStatus>,
    nav: &NavigationManager,
    ui_mgr: &mut UiManager,
    modal: &ModalManagerState,
) {
    egui::TopBottomPanel::bottom("statusbar")
        .exact_height(22.0)
        .frame(
            egui::Frame::NONE
                .fill(theme::BG_STATUSBAR)
                .inner_margin(egui::Margin::symmetric(8, 2)),
        )
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                // Phase F: prefer SyncStatus over DashboardState for online/peer info
                let is_online = sync_status.map(|s| s.online).unwrap_or(dashboard.is_online);
                let peer_count = sync_status
                    .map(|s| s.peer_count as u64)
                    .unwrap_or(dashboard.peer_count);

                if is_online {
                    ui.colored_label(theme::STATUS_ONLINE_DOT, "\u{25CF}");
                    ui.label(
                        egui::RichText::new("Online")
                            .small()
                            .color(theme::STATUS_ONLINE),
                    );
                } else {
                    ui.colored_label(theme::STATUS_OFFLINE_DOT, "\u{25CF}");
                    ui.label(
                        egui::RichText::new("Offline")
                            .small()
                            .color(theme::STATUS_OFFLINE),
                    );
                }

                ui.separator();
                // Clickable peer count opens debug panel
                let peer_label = egui::RichText::new(format!("{} peers", peer_count))
                    .small()
                    .color(theme::TEXT_DIM);
                if ui
                    .add(egui::Label::new(peer_label).sense(egui::Sense::click()))
                    .on_hover_text("Click for peer debug panel")
                    .clicked()
                {
                    if matches!(ui_mgr.active_dialog, ActiveDialog::PeerDebug) {
                        ui_mgr.close_dialog();
                    } else {
                        ui_mgr.open_dialog(ActiveDialog::PeerDebug);
                    }
                }

                // Phase F: show active verse name
                if let Some(ref _vid) = nav.active_verse_id {
                    ui.separator();
                    ui.label(
                        egui::RichText::new(format!("Verse: {}", nav.active_verse_name))
                            .small()
                            .color(theme::TEXT_DIM),
                    );
                } else if !is_online {
                    ui.separator();
                    ui.label(
                        egui::RichText::new("Local only")
                            .small()
                            .color(theme::TEXT_MUTED),
                    );
                }

                ui.separator();
                // "rooms" is a legacy table with no in-app create path — not shown.
                ui.label(
                    egui::RichText::new(format!(
                        "{} petals  {} models",
                        dashboard.petal_count, dashboard.model_count
                    ))
                    .small()
                    .color(theme::TEXT_MUTED),
                );

                // Persistent guard-error segment (Q-5, FR-7): a disabled
                // panel's error stays visible for the session — no
                // auto-clear on petal switch, unlike the general §6
                // clears-on-resolution tier (ratified exception for this
                // guard, since the panel itself stays disabled).
                if let Some(err) = &modal.last_error {
                    ui.separator();
                    ui.label(
                        egui::RichText::new(format!("\u{26A0} {err}"))
                            .small()
                            .color(theme::STATUS_OFFLINE),
                    );
                }
            });
        });
}
