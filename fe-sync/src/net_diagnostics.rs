//! Connection-kind derivation and the API-to-sync diagnostics bridge (see `AGENTS.md` §net-diagnostics).

use crate::messages::{SyncCommand, SyncCommandSender};
use crate::reachability::ReachabilityVerdict;
use crate::relay_config::{RelayConfig, RelayHealth};
use fe_runtime::net_diagnostics::{
    render_addrs, NetworkDiagnostics, NetworkDiagnosticsCall, NetworkDiagnosticsCallReceiver, NodeDiagnostics,
    PeerConnectionKind, PeerDiagnostics,
};
use iroh::endpoint::{RemoteInfo, TransportAddrUsage};
use iroh::TransportAddr;
use std::collections::BTreeSet;
use std::net::SocketAddr;

/// How long the bridge waits for the sync thread to answer one diagnostics request.
const DIAGNOSTICS_REPLY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Active transport addresses of a remote; inactive or stale entries never count.
pub fn active_addrs(info: &RemoteInfo) -> Vec<TransportAddr> {
    info.addrs()
        .filter(|entry| matches!(entry.usage(), TransportAddrUsage::Active))
        .map(|entry| entry.addr().clone())
        .collect()
}

/// Classifies active paths: IP only is Direct, relay only is Relayed, both Mixed, none Unknown.
pub fn connection_kind<'a>(active: impl Iterator<Item = &'a TransportAddr>) -> (PeerConnectionKind, Option<String>) {
    let (mut has_ip, mut has_relay, mut relay_host) = (false, false, None);
    for addr in active {
        match addr {
            TransportAddr::Ip(_) => has_ip = true,
            TransportAddr::Relay(url) => {
                has_relay = true;
                if relay_host.is_none() {
                    relay_host = url.host_str().map(str::to_owned);
                }
            }
            _ => {}
        }
    }
    let kind = match (has_ip, has_relay) {
        (true, false) => PeerConnectionKind::Direct,
        (false, true) => PeerConnectionKind::Relayed,
        (true, true) => PeerConnectionKind::Mixed,
        (false, false) => PeerConnectionKind::Unknown,
    };
    (kind, relay_host)
}

/// Builds one peer row from its active paths; addresses are redacted unless `detail`.
pub fn peer_diagnostics(peer_did: String, verse_ids: Vec<String>, active: &[TransportAddr], detail: bool) -> PeerDiagnostics {
    let (kind, relay_host) = connection_kind(active.iter());
    let ip_addrs: Vec<&SocketAddr> = active
        .iter()
        .filter_map(|addr| match addr {
            TransportAddr::Ip(socket) => Some(socket),
            _ => None,
        })
        .collect();
    PeerDiagnostics { peer_did, verse_ids, kind, relay_host, addrs: render_addrs(ip_addrs, detail) }
}

/// Hint shown when peers are reachable only through a relay; None when nothing needs attention.
pub fn downgrade_hint(relay_mode: &str, home_relay: Option<&str>, kinds: &[PeerConnectionKind]) -> Option<String> {
    let relayed = kinds.iter().filter(|kind| **kind == PeerConnectionKind::Relayed).count();
    let direct = kinds
        .iter()
        .filter(|kind| matches!(kind, PeerConnectionKind::Direct | PeerConnectionKind::Mixed))
        .count();
    if relayed > 0 && direct == 0 {
        return Some("All peers connect through a relay; direct paths were not established (check NAT or port mapping).".into());
    }
    if home_relay.is_none() && relay_mode != "disabled" && !kinds.is_empty() && direct == 0 {
        return Some("No home relay is selected and no direct path exists; peers may be unreachable.".into());
    }
    None
}

/// Display label for the relay configuration.
pub fn relay_mode_label(config: &RelayConfig) -> &'static str {
    match config {
        RelayConfig::Default => "default",
        RelayConfig::Disabled => "disabled",
        RelayConfig::Custom(_) => "custom",
    }
}

/// Display label for relay health.
pub fn relay_health_label(health: RelayHealth) -> &'static str {
    match health {
        RelayHealth::Unknown => "unknown",
        RelayHealth::Healthy => "healthy",
        RelayHealth::Degraded => "degraded",
        RelayHealth::Unreachable => "unreachable",
        RelayHealth::Disabled => "disabled",
    }
}

/// Display label for a self-test verdict.
pub fn reachability_label(verdict: ReachabilityVerdict) -> &'static str {
    match verdict {
        ReachabilityVerdict::Public => "public",
        ReachabilityVerdict::BehindNat => "behind_nat",
        ReachabilityVerdict::Cgnat => "cgnat",
        ReachabilityVerdict::SymmetricNat => "symmetric_nat",
        ReachabilityVerdict::CaptivePortal => "captive_portal",
        ReachabilityVerdict::NoUdp => "no_udp",
    }
}

/// Runs the reachability self-test on demand; `None` while the first net report is pending.
#[cfg(feature = "net-report")]
pub fn reachability_self_test(endpoint: &iroh::Endpoint) -> Option<&'static str> {
    use crate::reachability::{classify, IrohNetReportProbe, ReachabilityProbe};
    let summary = IrohNetReportProbe::new(endpoint.clone(), Vec::new()).summary()?;
    Some(reachability_label(classify(&summary).0))
}

/// Probe not built into this binary: the verdict stays pending.
#[cfg(not(feature = "net-report"))]
pub fn reachability_self_test(_endpoint: &iroh::Endpoint) -> Option<&'static str> {
    None
}

/// Everything the sync thread knows at one instant, ready to be shaped into the DTO.
pub struct SnapshotInputs<'a> {
    pub relay_mode: &'a str,
    pub home_relay: Option<String>,
    pub relay_health: RelayHealth,
    pub portmapper: &'a str,
    /// Cached self-test verdict label; `None` renders as `pending`.
    pub reachability_verdict: Option<&'a str>,
    pub relay_server_state: &'a str,
    /// Hazard B: open verses whose adverts omit this node's advertised home relay.
    pub unadvertised_home_verses: &'a BTreeSet<String>,
    pub peers: Vec<PeerDiagnostics>,
}

/// Shapes the node and peer rows; the downgrade hint and the hazard-B warning share one field.
pub fn build_snapshot(inputs: SnapshotInputs<'_>) -> NetworkDiagnostics {
    let kinds: Vec<PeerConnectionKind> = inputs.peers.iter().map(|peer| peer.kind).collect();
    let mut hints: Vec<String> = downgrade_hint(inputs.relay_mode, inputs.home_relay.as_deref(), &kinds)
        .into_iter()
        .collect();
    if !inputs.unadvertised_home_verses.is_empty() {
        let verses: Vec<&str> = inputs.unadvertised_home_verses.iter().map(String::as_str).collect();
        hints.push(format!(
            "The home relay is not advertised in verse(s) {}; their peers may not reach this node through it.",
            verses.join(", ")
        ));
    }
    NetworkDiagnostics {
        node: NodeDiagnostics {
            relay_mode: inputs.relay_mode.to_owned(),
            home_relay: inputs.home_relay,
            relay_health: relay_health_label(inputs.relay_health).to_owned(),
            portmapper: inputs.portmapper.to_owned(),
            reachability_verdict: inputs.reachability_verdict.unwrap_or("pending").to_owned(),
            relay_server_state: inputs.relay_server_state.to_owned(),
            downgrade_hint: if hints.is_empty() { None } else { Some(hints.join(" ")) },
        },
        peers: inputs.peers,
    }
}

/// One row per connected peer from live `remote_info`; each entry pairs an endpoint with the verses it shares with us.
pub async fn collect_peer_rows(
    endpoint: &iroh::Endpoint,
    by_peer: Vec<(iroh::EndpointId, Vec<String>)>,
    detail: bool,
) -> Vec<PeerDiagnostics> {
    let mut rows = Vec::with_capacity(by_peer.len());
    for (id, verse_ids) in by_peer {
        let active = match endpoint.remote_info(id).await {
            Some(info) => active_addrs(&info),
            None => Vec::new(),
        };
        rows.push(peer_diagnostics(crate::replicator::peer_did_key(&id), verse_ids, &active, detail));
    }
    rows
}

/// Spawns the API-to-sync bridge: each call becomes a `GetNetworkDiagnostics` command and its answer is relayed to the caller's oneshot.
pub fn bridge_network_diagnostics(
    receiver: NetworkDiagnosticsCallReceiver,
    sync_command_sender: SyncCommandSender,
) -> std::io::Result<std::thread::JoinHandle<()>> {
    bridge_network_diagnostics_with(receiver, move |call| {
        let (reply_sender, reply_receiver) = crossbeam::channel::bounded(1);
        sync_command_sender
            .send(SyncCommand::GetNetworkDiagnostics { detail: call.detail, reply: reply_sender })
            .map_err(|_| "the sync thread is not accepting commands".to_owned())?;
        let snapshot = reply_receiver
            .recv_timeout(DIAGNOSTICS_REPLY_TIMEOUT)
            .map_err(|error| format!("the sync thread did not answer: {error}"))?;
        if call.reply.send(snapshot).is_err() {
            tracing::debug!("network diagnostics caller is no longer waiting");
        }
        Ok(())
    })
}

/// Generic bridge loop; `forward` owns the reply.
pub fn bridge_network_diagnostics_with(
    receiver: NetworkDiagnosticsCallReceiver,
    forward: impl Fn(NetworkDiagnosticsCall) -> Result<(), String> + Send + 'static,
) -> std::io::Result<std::thread::JoinHandle<()>> {
    std::thread::Builder::new().name("net-diagnostics-bridge".into()).spawn(move || {
        while let Ok(call) = receiver.recv() {
            if let Err(error) = forward(call) {
                tracing::warn!(%error, "network diagnostics call could not be forwarded to the sync thread");
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(text: &str) -> TransportAddr {
        TransportAddr::Ip(text.parse().unwrap())
    }
    fn relay(text: &str) -> TransportAddr {
        TransportAddr::Relay(text.parse().unwrap())
    }

    #[test]
    fn remote_info_active_addrs_map_to_direct_relayed_mixed_unknown() {
        let cases: Vec<(Vec<TransportAddr>, PeerConnectionKind, Option<&str>)> = vec![
            (vec![ip("203.0.113.7:4000")], PeerConnectionKind::Direct, None),
            (vec![relay("https://relay.example.com")], PeerConnectionKind::Relayed, Some("relay.example.com")),
            (
                vec![ip("[2001:db8::1]:4000"), relay("https://relay.example.com")],
                PeerConnectionKind::Mixed,
                Some("relay.example.com"),
            ),
            (vec![], PeerConnectionKind::Unknown, None),
        ];
        for (addrs, expected_kind, expected_host) in cases {
            let (kind, host) = connection_kind(addrs.iter());
            assert_eq!(kind, expected_kind);
            assert_eq!(host.as_deref(), expected_host);
        }
        let row = peer_diagnostics("did:key:z".into(), vec!["v1".into()], &[ip("203.0.113.7:51820")], false);
        assert_eq!(row.addrs, vec!["ipv4:ephemeral"]);
        assert!(downgrade_hint("default", Some("r"), &[PeerConnectionKind::Relayed]).is_some());
        assert!(downgrade_hint("default", Some("r"), &[PeerConnectionKind::Relayed, PeerConnectionKind::Direct]).is_none());

        let unadvertised: BTreeSet<String> = ["verse-a".to_owned()].into();
        let snapshot = build_snapshot(SnapshotInputs {
            relay_mode: "disabled",
            home_relay: None,
            relay_health: RelayHealth::Disabled,
            portmapper: "unknown",
            reachability_verdict: None,
            relay_server_state: "not-built",
            unadvertised_home_verses: &unadvertised,
            peers: vec![row],
        });
        assert_eq!(snapshot.node.reachability_verdict, "pending");
        assert_eq!(snapshot.node.relay_health, "disabled");
        assert!(snapshot.node.downgrade_hint.as_deref().is_some_and(|hint| hint.contains("verse-a")));
    }
}
