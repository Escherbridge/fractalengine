//! Connection-kind derivation and the API-to-sync diagnostics bridge (see `AGENTS.md` §net-diagnostics).

use fe_runtime::net_diagnostics::{
    render_addrs, NetworkDiagnosticsCall, NetworkDiagnosticsCallReceiver, PeerConnectionKind, PeerDiagnostics,
};
use iroh::endpoint::{RemoteInfo, TransportAddrUsage};
use iroh::TransportAddr;
use std::net::SocketAddr;

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

/// Spawns the thread that forwards API diagnostics calls to the sync thread; `forward` owns the reply.
pub fn bridge_network_diagnostics(
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
    }
}
