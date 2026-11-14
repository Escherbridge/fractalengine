//! Reachability classifier over a net-report summary plus the iroh probe adapter; see `fe-sync/src/AGENTS.md` §reachability.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
#[cfg(feature = "net-report")]
use std::net::SocketAddr;

/// How a public address was learned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddrSource {
    /// Carried by a locally bound socket (no NAT in the way).
    Local,
    /// Reported by a relay/QAD probe from outside.
    Stun,
    /// A UDP port mapping from the portmapper; never proof of TCP reachability.
    Portmapped,
}

/// One public address candidate with its source tag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PublicAddr {
    pub ip: IpAddr,
    pub source: AddrSource,
}

/// Plain summary of iroh's net report; decouples the classifier from unstable upstream types.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NetReportSummary {
    pub udp_v4: bool,
    pub udp_v6: bool,
    /// `Some(true)` means the reported address differs per destination (symmetric NAT).
    pub mapping_varies_by_dest: Option<bool>,
    pub captive_portal: Option<bool>,
    pub public_addrs: Vec<PublicAddr>,
    pub preferred_relay: Option<String>,
}

/// Outcome of the self-test.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReachabilityVerdict {
    Public,
    BehindNat,
    Cgnat,
    SymmetricNat,
    CaptivePortal,
    NoUdp,
}

/// Source of net-report summaries (real iroh impl behind the `net-report` feature).
pub trait ReachabilityProbe {
    /// Latest summary, or `None` while the first report is still pending.
    fn summary(&mut self) -> Option<NetReportSummary>;
}

fn is_cgnat_v4(ip: Ipv4Addr) -> bool {
    let octets = ip.octets();
    octets[0] == 100 && (64..=127).contains(&octets[1])
}

fn is_public_unicast_v4(ip: Ipv4Addr) -> bool {
    !(ip.is_private()
        || is_cgnat_v4(ip)
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_unspecified()
        || ip.is_multicast()
        || ip.is_broadcast()
        || ip.is_documentation())
}

fn is_public_unicast_v6(ip: Ipv6Addr) -> bool {
    let first_segment = ip.segments()[0];
    let unique_local = (first_segment & 0xfe00) == 0xfc00;
    let link_local = (first_segment & 0xffc0) == 0xfe80;
    let global_unicast = (first_segment & 0xe000) == 0x2000;
    global_unicast && !(unique_local || link_local || ip.is_loopback() || ip.is_multicast())
}

fn is_public_unicast(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_public_unicast_v4(v4),
        IpAddr::V6(v6) => is_public_unicast_v6(v6),
    }
}

fn is_cgnat_or_private(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_cgnat_v4(v4) || v4.is_private(),
        IpAddr::V6(_) => false,
    }
}

/// Maps a summary to a verdict and remediation text; only a locally carried public address is `Public`.
pub fn classify(summary: &NetReportSummary) -> (ReachabilityVerdict, &'static str) {
    if summary.captive_portal == Some(true) {
        return (
            ReachabilityVerdict::CaptivePortal,
            "A captive portal is intercepting traffic. Sign in to the network, then re-run the self-test.",
        );
    }
    if !(summary.udp_v4 || summary.udp_v6) {
        return (
            ReachabilityVerdict::NoUdp,
            "UDP is blocked. Allow outbound UDP, or use a network that does; relaying over HTTPS still works.",
        );
    }
    if summary.mapping_varies_by_dest == Some(true) {
        return (
            ReachabilityVerdict::SymmetricNat,
            "The router uses a different public port per destination (symmetric NAT). Enable port forwarding or a DMZ host, or serve from a machine with a public address.",
        );
    }
    let carries_public_address = summary
        .public_addrs
        .iter()
        .any(|addr| addr.source == AddrSource::Local && is_public_unicast(addr.ip));
    if carries_public_address {
        return (
            ReachabilityVerdict::Public,
            "This machine carries a public address directly.",
        );
    }
    let behind_carrier_nat = summary
        .public_addrs
        .iter()
        .any(|addr| addr.source == AddrSource::Stun && is_cgnat_or_private(addr.ip));
    if behind_carrier_nat {
        return (
            ReachabilityVerdict::Cgnat,
            "The provider shares one public address between subscribers (carrier-grade NAT). Ask for a public address, use IPv6, or serve from another machine.",
        );
    }
    (
        ReachabilityVerdict::BehindNat,
        "This machine is behind a home router. Forward the relay port to it, or serve from a machine with a public address.",
    )
}

/// Real probe over iroh's unstable net report; the only place that touches the upstream type.
#[cfg(feature = "net-report")]
pub struct IrohNetReportProbe {
    endpoint: iroh::Endpoint,
    portmapped: Vec<SocketAddr>,
}

#[cfg(feature = "net-report")]
impl IrohNetReportProbe {
    /// `portmapped` holds the `Portmapped` direct addresses from `watch_addr`.
    pub fn new(endpoint: iroh::Endpoint, portmapped: Vec<SocketAddr>) -> Self {
        Self { endpoint, portmapped }
    }
}

/// Converts an iroh report into a summary; a reported address equal to a bound socket is `Local`.
#[cfg(feature = "net-report")]
pub fn summarize_iroh(
    report: &iroh::unstable_net_report::NetReport,
    bound_sockets: &[SocketAddr],
    portmapped: &[SocketAddr],
) -> NetReportSummary {
    let mut public_addrs = Vec::new();
    let reported = [
        report.global_v4.map(SocketAddr::V4),
        report.global_v6.map(SocketAddr::V6),
    ];
    for socket in reported.into_iter().flatten() {
        let source = if bound_sockets.contains(&socket) {
            AddrSource::Local
        } else {
            AddrSource::Stun
        };
        public_addrs.push(PublicAddr { ip: socket.ip(), source });
    }
    for socket in bound_sockets {
        let already_listed = public_addrs.iter().any(|addr| addr.ip == socket.ip());
        if socket.is_ipv6() && !already_listed && is_public_unicast(socket.ip()) {
            public_addrs.push(PublicAddr { ip: socket.ip(), source: AddrSource::Local });
        }
    }
    for socket in portmapped {
        public_addrs.push(PublicAddr { ip: socket.ip(), source: AddrSource::Portmapped });
    }
    NetReportSummary {
        udp_v4: report.udp_v4,
        udp_v6: report.udp_v6,
        mapping_varies_by_dest: report.mapping_varies_by_dest(),
        captive_portal: report.captive_portal,
        public_addrs,
        preferred_relay: report.preferred_relay.as_ref().map(ToString::to_string),
    }
}

#[cfg(feature = "net-report")]
impl ReachabilityProbe for IrohNetReportProbe {
    fn summary(&mut self) -> Option<NetReportSummary> {
        use iroh::Watcher;
        let mut watcher = self.endpoint.net_report();
        let report = watcher.get()?;
        Some(summarize_iroh(&report, &self.endpoint.bound_sockets(), &self.portmapped))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(text: &str, source: AddrSource) -> PublicAddr {
        PublicAddr { ip: text.parse().unwrap(), source }
    }

    fn udp(public_addrs: Vec<PublicAddr>) -> NetReportSummary {
        NetReportSummary { udp_v4: true, public_addrs, ..Default::default() }
    }

    #[test]
    fn classify_table_cgnat_symmetric_captive_noudp_public_and_portmapped_only() {
        use ReachabilityVerdict::*;
        let symmetric = NetReportSummary {
            mapping_varies_by_dest: Some(true),
            ..udp(vec![addr("203.0.113.9", AddrSource::Stun)])
        };
        let captive = NetReportSummary {
            captive_portal: Some(true),
            ..udp(vec![])
        };
        let table: Vec<(&str, NetReportSummary, ReachabilityVerdict)> = vec![
            ("no udp", NetReportSummary::default(), NoUdp),
            ("captive wins over udp", captive, CaptivePortal),
            ("symmetric", symmetric, SymmetricNat),
            ("cgnat 100.64/10", udp(vec![addr("100.72.1.5", AddrSource::Stun)]), Cgnat),
            ("double nat rfc1918", udp(vec![addr("192.168.1.9", AddrSource::Stun)]), Cgnat),
            ("public via bound socket", udp(vec![addr("93.184.216.34", AddrSource::Local)]), Public),
            (
                "public v6 bound beats cgnat v4",
                udp(vec![
                    addr("100.72.1.5", AddrSource::Stun),
                    addr("2606:4700::1", AddrSource::Local),
                ]),
                Public,
            ),
            ("stun-only public is behind nat", udp(vec![addr("203.0.113.9", AddrSource::Stun)]), BehindNat),
            (
                "portmapped-only public is behind nat",
                udp(vec![addr("203.0.113.9", AddrSource::Portmapped)]),
                BehindNat,
            ),
            ("no address yet", udp(vec![]), BehindNat),
        ];
        for (label, summary, expected) in table {
            let (verdict, remediation) = classify(&summary);
            assert_eq!(verdict, expected, "{label}");
            assert!(!remediation.is_empty(), "{label}: remediation text");
        }
    }
}
