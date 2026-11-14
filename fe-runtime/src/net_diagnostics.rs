//! Network diagnostics DTO, API call seam and address redaction (see `fe-sync/src/AGENTS.md` §net-diagnostics).

use serde::{Deserialize, Serialize};
use std::net::SocketAddr;

/// How a peer is currently reached, derived from live paths only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PeerConnectionKind {
    Direct,
    Relayed,
    Mixed,
    Unknown,
}

/// This node's own network posture; each field is a display string so UI and API share one shape.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeDiagnostics {
    pub relay_mode: String,
    pub home_relay: Option<String>,
    pub relay_health: String,
    pub portmapper: String,
    pub reachability_verdict: String,
    pub relay_server_state: String,
    pub downgrade_hint: Option<String>,
}

/// One remote peer's connection state; `addrs` is redacted unless detail was requested.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerDiagnostics {
    pub peer_did: String,
    pub verse_ids: Vec<String>,
    pub kind: PeerConnectionKind,
    pub relay_host: Option<String>,
    pub addrs: Vec<String>,
}

/// Full diagnostics snapshot.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetworkDiagnostics {
    pub node: NodeDiagnostics,
    pub peers: Vec<PeerDiagnostics>,
}

/// A diagnostics request plus its one-shot reply seam (API to sync thread).
#[derive(Debug)]
pub struct NetworkDiagnosticsCall {
    /// When true, peer addresses are returned unredacted.
    pub detail: bool,
    pub reply: tokio::sync::oneshot::Sender<NetworkDiagnostics>,
}

/// Channel half for the API-to-sync diagnostics seam.
pub type NetworkDiagnosticsCallSender = crossbeam::channel::Sender<NetworkDiagnosticsCall>;
/// Receiving half, owned by the bridge.
pub type NetworkDiagnosticsCallReceiver = crossbeam::channel::Receiver<NetworkDiagnosticsCall>;

/// Reduces a socket address to its family and port class, e.g. `ipv4:ephemeral`.
pub fn redact(addr: &SocketAddr) -> String {
    let family = if addr.is_ipv4() { "ipv4" } else { "ipv6" };
    let port_class = match addr.port() {
        0..=1023 => "well-known",
        1024..=49151 => "registered",
        _ => "ephemeral",
    };
    format!("{family}:{port_class}")
}

/// Renders addresses for a peer row: full text when `detail`, otherwise redacted.
pub fn render_addrs<'a>(addrs: impl IntoIterator<Item = &'a SocketAddr>, detail: bool) -> Vec<String> {
    addrs
        .into_iter()
        .map(|addr| if detail { addr.to_string() } else { redact(addr) })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostics_redact_ips_by_default() {
        let addrs: Vec<SocketAddr> = vec![
            "203.0.113.7:51820".parse().unwrap(),
            "[2001:db8::1]:443".parse().unwrap(),
            "10.0.0.5:7000".parse().unwrap(),
        ];
        let redacted = render_addrs(&addrs, false);
        assert_eq!(redacted, vec!["ipv4:ephemeral", "ipv6:well-known", "ipv4:registered"]);
        assert!(redacted.iter().all(|row| !row.contains("203.0.113") && !row.contains("2001:db8")));
        let detailed = render_addrs(&addrs, true);
        assert_eq!(detailed[0], "203.0.113.7:51820");

        let snapshot = NetworkDiagnostics {
            node: NodeDiagnostics::default(),
            peers: vec![PeerDiagnostics {
                peer_did: "did:key:z".into(),
                verse_ids: vec![],
                kind: PeerConnectionKind::Direct,
                relay_host: None,
                addrs: redacted,
            }],
        };
        let json = serde_json::to_string(&snapshot).unwrap();
        assert!(!json.contains("203.0.113"));
        assert!(json.contains("\"direct\""));
        let back: NetworkDiagnostics = serde_json::from_str(&json).unwrap();
        assert_eq!(back, snapshot);
    }
}
