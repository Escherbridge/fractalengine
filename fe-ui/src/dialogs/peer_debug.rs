//! Peer debug overlay panel with the network diagnostics block (see `dialogs/AGENTS.md`).

use crate::ui_shell::layout_policy::WindowTargets;
use bevy_egui::egui;
use fe_runtime::net_diagnostics::{NetworkDiagnostics, PeerConnectionKind};

use crate::accessibility::ResponseExt;

use super::ActiveDialog;
use crate::actions::UiManager;
use crate::theme;

/// Characters of a peer identifier kept in the table.
const PEER_LABEL_CHARS: usize = 16;

pub fn render_peer_debug_panel(
    ctx: &egui::Context,
    ui_mgr: &mut UiManager,
    sync_status: Option<&fe_sync::SyncStatus>,
    diagnostics: Option<&fe_sync::status::NetworkDiagnosticsState>,
    sync_sender: Option<&fe_sync::SyncCommandSenderRes>,
) {
    if !matches!(ui_mgr.active_dialog, ActiveDialog::PeerDebug) {
        return;
    }

    let mut close = false;
    let mut refresh = false;

    crate::ui_shell::layout_policy::bounded_window(ctx, "Peer Debug")
        .collapsible(true)
        .resizable(true)
        .default_width(360.0)
        .show_with_targets(ctx, |ui| {
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
            ui.separator();
            match diagnostics.and_then(|state| state.0.as_ref()) {
                Some(snapshot) => render_network_block(ui, snapshot),
                None => {
                    ui.label(
                        egui::RichText::new("No network snapshot yet")
                            .small()
                            .color(theme::TEXT_MUTED),
                    );
                }
            }

            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let can_refresh = sync_sender.is_some();
                if ui
                    .add_enabled(can_refresh, egui::Button::new("Refresh"))
                    .observed(ui)
                    .clicked()
                {
                    refresh = true;
                }
                if ui.button("Close").observed(ui).clicked() {
                    close = true;
                }
            });
        });

    if refresh {
        request_diagnostics(sync_sender);
    }
    if close {
        ui_mgr.close_dialog();
    }
}

/// Asks the sync thread for a fresh redacted snapshot; the answer arrives through `NetworkDiagnosticsState`.
fn request_diagnostics(sync_sender: Option<&fe_sync::SyncCommandSenderRes>) {
    let Some(sender) = sync_sender else { return };
    // The state resource carries the result, so the direct reply is dropped.
    let (reply, _unused_receiver) = crossbeam::channel::bounded(1);
    if sender
        .0
        .send(fe_sync::SyncCommand::GetNetworkDiagnostics {
            detail: false,
            reply,
        })
        .is_err()
    {
        bevy::log::warn!("Sync channel closed; GetNetworkDiagnostics not sent");
    }
}

fn connection_label(kind: PeerConnectionKind) -> &'static str {
    match kind {
        PeerConnectionKind::Direct => "Direct",
        PeerConnectionKind::Relayed => "Relayed",
        PeerConnectionKind::Mixed => "Mixed",
        PeerConnectionKind::Unknown => "Unknown",
    }
}

fn short_peer(peer_did: &str) -> String {
    let total = peer_did.chars().count();
    if total <= PEER_LABEL_CHARS {
        return peer_did.to_owned();
    }
    let tail: String = peer_did.chars().skip(total - PEER_LABEL_CHARS).collect();
    format!("\u{2026}{tail}")
}

/// Node posture rows plus the peer table; peer addresses are never drawn.
fn render_network_block(ui: &mut egui::Ui, snapshot: &NetworkDiagnostics) {
    let node = &snapshot.node;
    ui.label(egui::RichText::new("Network").strong().color(theme::TEXT_HEADING));
    egui::Grid::new("peer_debug_node_grid")
        .num_columns(2)
        .spacing([12.0, 2.0])
        .show(ui, |ui| {
            let home_relay = node.home_relay.as_deref().unwrap_or("none");
            for (name, value) in [
                ("Relay mode", node.relay_mode.as_str()),
                ("Home relay", home_relay),
                ("Relay health", node.relay_health.as_str()),
                ("Port mapping", node.portmapper.as_str()),
                ("Reachability", node.reachability_verdict.as_str()),
                ("Relay server", node.relay_server_state.as_str()),
            ] {
                ui.label(egui::RichText::new(name).color(theme::TEXT_DIM));
                ui.label(value);
                ui.end_row();
            }
        });
    if let Some(hint) = node.downgrade_hint.as_deref() {
        ui.add_space(4.0);
        ui.label(egui::RichText::new(hint).color(theme::STATUS_OFFLINE));
    }
    if !node.unadvertised_home_verses.is_empty() {
        // Hazard B (AC-9): peers in these verses may not reach this node through its home relay.
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new(format!(
                "Home relay not advertised in {} verse(s); their peers may not reach you through it.",
                node.unadvertised_home_verses.len()
            ))
            .color(theme::STATUS_OFFLINE),
        );
    }

    ui.add_space(8.0);
    ui.label(egui::RichText::new("Peers").strong().color(theme::TEXT_HEADING));
    if snapshot.peers.is_empty() {
        ui.label(egui::RichText::new("No peers connected").small().color(theme::TEXT_MUTED));
        return;
    }
    egui::Grid::new("peer_debug_peer_grid")
        .num_columns(3)
        .spacing([12.0, 2.0])
        .show(ui, |ui| {
            for heading in ["Peer", "Path", "Relay"] {
                ui.label(egui::RichText::new(heading).color(theme::TEXT_DIM));
            }
            ui.end_row();
            for peer in &snapshot.peers {
                ui.monospace(short_peer(&peer.peer_did));
                ui.label(connection_label(peer.kind));
                ui.label(peer.relay_host.as_deref().unwrap_or("\u{2014}"));
                ui.end_row();
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use fe_runtime::net_diagnostics::{NodeDiagnostics, PeerDiagnostics};

    fn collect_text(shape: &egui::epaint::Shape, out: &mut String) {
        match shape {
            egui::epaint::Shape::Text(text) => {
                out.push_str(text.galley.text());
                out.push('\n');
            }
            egui::epaint::Shape::Vec(inner) => inner.iter().for_each(|s| collect_text(s, out)),
            _ => {}
        }
    }

    #[test]
    fn network_panel_renders_fixture_without_ips() {
        let snapshot = NetworkDiagnostics {
            node: NodeDiagnostics {
                relay_mode: "default".into(),
                home_relay: Some("relay.example.org".into()),
                relay_health: "healthy".into(),
                portmapper: "upnp".into(),
                reachability_verdict: "reachable: direct".into(),
                relay_server_state: "off".into(),
                downgrade_hint: Some("2 verses omit your home relay".into()),
                unadvertised_home_verses: vec!["01JVERSEB".into()],
            },
            peers: vec![
                PeerDiagnostics {
                    peer_did: "did:key:z6MkhaXgBZDvotDkL5257faiztiGiC2QtKLGpbnnEGta2doK".into(),
                    verse_ids: vec![],
                    kind: PeerConnectionKind::Relayed,
                    relay_host: Some("relay.example.org".into()),
                    addrs: vec!["203.0.113.7:51820".into()],
                },
                PeerDiagnostics {
                    peer_did: "did:key:z6Mkother".into(),
                    verse_ids: vec![],
                    kind: PeerConnectionKind::Direct,
                    relay_host: None,
                    addrs: vec!["[2001:db8::1]:443".into()],
                },
            ],
        };

        let ctx = egui::Context::default();
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| render_network_block(ui, &snapshot));
        });
        let mut text = String::new();
        for clipped in &output.shapes {
            collect_text(&clipped.shape, &mut text);
        }

        for expected in [
            "Relay mode", "Home relay", "Relay health", "Port mapping", "Reachability",
            "Relay server", "relay.example.org", "reachable: direct",
            "2 verses omit your home relay", "Home relay not advertised in 1 verse(s)", "Relayed", "Direct",
        ] {
            assert!(text.contains(expected), "missing label {expected:?} in {text}");
        }
        assert!(!text.contains("203.0.113") && !text.contains("2001:db8"), "address leaked: {text}");
    }
}
