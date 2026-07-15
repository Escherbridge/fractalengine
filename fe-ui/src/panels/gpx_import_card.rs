//! GIS panel "GPX Import" section: file picker + persistent status row.
//! Queues `UiAction::GpxImportFile`; fe-ui does no GPX parsing/file I/O
//! itself — see `crate::gpx_ops` for the pending-ops/result-status
//! integration contract (mirrors `panels::asset_card`/`crate::asset_ops`).

use bevy_egui::egui;

use crate::actions::{UiAction, UiManager};
use crate::gpx_ops::GpxImportStatus;
use crate::theme;

/// GPX import button (rfd file picker, filtered to `.gpx`) + a persistent
/// status row for the active petal — mirrors `asset_card`'s Download
/// button + status-row idiom. The main binary drains `PendingGpxOps` and
/// writes the outcome to `GpxImportStatus`.
pub(crate) fn gpx_import_section(
    ui: &mut egui::Ui,
    ui_mgr: &mut UiManager,
    gpx_status: &GpxImportStatus,
    petal_id: &str,
) {
    ui.label(
        egui::RichText::new("GPX Import")
            .strong()
            .color(theme::TEXT_SECTION),
    );
    ui.add_space(4.0);
    if ui
        .add(egui::Button::new("\u{1F4C1} Import GPX...").fill(theme::BG_BUTTON))
        .clicked()
    {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("GPX Tracks", &["gpx"])
            .add_filter("All Files", &["*"])
            .pick_file()
        {
            ui_mgr.push_action(UiAction::GpxImportFile {
                petal_id: petal_id.to_string(),
                path,
            });
        }
    }

    // Persistent status row, gated to the active petal so a stale result
    // from a previously-active petal doesn't bleed in (see asset_card's
    // FR-3 status-row precedent).
    if gpx_status.petal_id.as_deref() == Some(petal_id) {
        ui.add_space(4.0);
        if let Some(err) = &gpx_status.error {
            ui.label(
                egui::RichText::new(format!("\u{2717} {err}"))
                    .small()
                    .color(theme::STATUS_OFFLINE),
            );
        } else {
            ui.label(
                egui::RichText::new(format!(
                    "\u{2713} Imported {} track(s), {} waypoint(s)",
                    gpx_status.track_count, gpx_status.waypoint_count
                ))
                .small()
                .color(theme::STATUS_ONLINE),
            );
        }
    }
}
