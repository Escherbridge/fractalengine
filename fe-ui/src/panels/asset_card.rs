//! Inspector "Asset" card: selected node's asset name + a Download button.
//! Queues `UiAction::DownloadNodeAsset`; fe-ui does no file I/O itself — see
//! `crate::asset_ops` for the pending-ops/result-status integration contract.
//!
//! `status` param is new (FR-3 persistent status row) — see
//! `fractalengine/src/AGENTS.md` §assets INTEGRATION_REQUEST for the call-site
//! chain (`panels/mod.rs` → `inspector.rs` → here) needed to thread it in from
//! the `AssetDownloadStatus` Bevy resource.

use bevy_egui::egui;

use crate::actions::{UiAction, UiManager};
use crate::asset_ops::AssetDownloadStatus;
use crate::node_manager::NodeManager;
use crate::theme;
use crate::verse_manager::VerseManager;

pub(crate) fn asset_card_section(
    ui: &mut egui::Ui,
    node_mgr: &NodeManager,
    verse_mgr: &VerseManager,
    ui_mgr: &mut UiManager,
    status: &AssetDownloadStatus,
) {
    let Some(sel) = node_mgr.selected.as_ref() else { return };
    let node = verse_mgr.all_nodes().find(|n| n.id == sel.node_id);
    let has_asset = node.map(|n| n.has_asset).unwrap_or(false);
    let asset_label = node.and_then(|n| n.asset_path.as_deref());

    egui::CollapsingHeader::new(
        egui::RichText::new("Asset").strong().color(theme::TEXT_SECTION),
    )
    .default_open(true)
    .show(ui, |ui| {
        ui.add_space(4.0);
        match asset_label {
            Some(path) => {
                ui.label(egui::RichText::new(path).small().monospace().color(theme::TEXT_DIM));
            }
            None if has_asset => {
                ui.label(
                    egui::RichText::new("Asset present (no local path cached)")
                        .small()
                        .color(theme::TEXT_MUTED),
                );
            }
            None => {
                ui.label(
                    egui::RichText::new("No asset on this node")
                        .small()
                        .color(theme::TEXT_MUTED),
                );
            }
        }

        ui.add_space(6.0);
        let btn = egui::Button::new("\u{2B07} Download asset")
            .fill(if has_asset { theme::BG_SAVE } else { theme::BG_BUTTON })
            .min_size(egui::vec2(ui.available_width(), 26.0));
        let resp = ui.add_enabled(has_asset, btn);
        if resp.clicked() {
            ui_mgr.push_action(UiAction::DownloadNodeAsset {
                node_id: sel.node_id.clone(),
            });
        }
        if !has_asset {
            resp.on_hover_text("This node has no asset to download");
        }

        // Persistent status row (FR-3): stays visible after the toast fades,
        // and only for the node it actually applies to — otherwise a stale
        // result from a previously-selected node would bleed into this card.
        if status.node_id.as_deref() == Some(sel.node_id.as_str()) {
            ui.add_space(4.0);
            if let Some(path) = &status.saved_path {
                ui.label(
                    egui::RichText::new(format!("\u{2713} Saved to {path}"))
                        .small()
                        .color(theme::STATUS_ONLINE),
                );
            } else if let Some(err) = &status.error {
                ui.label(
                    egui::RichText::new(format!("\u{2717} {err}"))
                        .small()
                        .color(theme::STATUS_OFFLINE),
                );
            }
        }
        ui.add_space(4.0);
    });
}
