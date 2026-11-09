//! Address policy for every connection an extension endpoint makes: global unicast only, or the declared loopback.
//! Rationale: net/AGENTS.md §addresses (P2P-QA steer 2026-11-09 01:50, SSRF allow-list; one shared predicate).

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use super::connect::is_loopback;

/// reqwest's resolver error type.
type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// True only for a globally routable unicast address; an IPv4-mapped IPv6 address is judged as its IPv4 address.
pub fn is_public_unicast(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => public_v4(v4),
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or_else(|| public_v6(v6), public_v4),
    }
}

fn public_v4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    let special = a == 0 // 0.0.0.0/8 "this network"
        || a == 10 // 10.0.0.0/8 private
        || (a == 100 && (b & 0xc0) == 64) // 100.64.0.0/10 shared address space (CGNAT)
        || a == 127 // loopback
        || (a == 169 && b == 254) // link-local, cloud metadata
        || (a == 172 && (b & 0xf0) == 16) // 172.16.0.0/12 private
        || (a == 192 && b == 0 && (c == 0 || c == 2)) // IETF protocol assignments, TEST-NET-1
        || (a == 192 && b == 88 && c == 99) // 6to4 relay anycast
        || (a == 192 && b == 168) // 192.168.0.0/16 private
        || (a == 198 && (b & 0xfe) == 18) // 198.18.0.0/15 benchmarking
        || (a == 198 && b == 51 && c == 100) // TEST-NET-2
        || (a == 203 && b == 0 && c == 113) // TEST-NET-3
        || a >= 224; // multicast 224/4, reserved 240/4, broadcast
    !special
}

fn public_v6(ip: Ipv6Addr) -> bool {
    let segments = ip.segments();
    // Only 2000::/3 is global unicast: drops ::/96, ::1, NAT64 (64:ff9b::/96, 64:ff9b:1::/48), ULA, link/site-local, multicast.
    if segments[0] & 0xe000 != 0x2000 {
        return false;
    }
    match segments[0] {
        // 2001::/23 IETF protocol assignments (Teredo, ORCHID, benchmarking) and 2001:db8::/32 documentation.
        0x2001 => segments[1] >= 0x0200 && segments[1] != 0x0db8,
        // 6to4 carries an IPv4 address in bits 16-47.
        0x2002 => public_v4(Ipv4Addr::new(
            (segments[1] >> 8) as u8,
            (segments[1] & 0xff) as u8,
            (segments[2] >> 8) as u8,
            (segments[2] & 0xff) as u8,
        )),
        // 3fff::/20 documentation (RFC 9637).
        0x3fff => segments[1] >= 0x1000,
        _ => true,
    }
}

/// Whether a connection for the declared `host` may go to `ip`: the loopback names reach loopback only, any other host global unicast only.
pub(super) fn admits(host: &str, ip: IpAddr) -> bool {
    if is_loopback(host) {
        canonical(ip).is_loopback()
    } else {
        is_public_unicast(ip)
    }
}

fn canonical(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(ip, IpAddr::V4),
        v4 => v4,
    }
}

/// Why a declared host is refused before any lookup: an IP literal it may never reach (`None` for names and admitted literals).
pub(super) fn literal_refusal(host: &str) -> Option<String> {
    let ip = host.parse::<IpAddr>().ok()?;
    (!admits(host, ip)).then(|| format!("{host} is not a public address"))
}

/// Resolve once and return the addresses the declared host admits; one refused address refuses the whole answer.
/// Callers connect to exactly these addresses, so a second lookup can never rebind the name.
pub(super) async fn resolve_checked(host: &str, port: u16) -> Result<Vec<SocketAddr>, String> {
    let addrs: Vec<SocketAddr> = match host.parse::<IpAddr>() {
        Ok(ip) => vec![SocketAddr::new(ip, port)],
        Err(_) => tokio::net::lookup_host((host, port))
            .await
            .map_err(|error| format!("{host} does not resolve: {error}"))?
            .collect(),
    };
    if addrs.is_empty() {
        return Err(format!("{host} does not resolve"));
    }
    match addrs.iter().find(|addr| !admits(host, addr.ip())) {
        Some(refused) => Err(format!("{host} resolves to {}, which is not a public address", refused.ip())),
        None => Ok(addrs),
    }
}

/// The egress clients' resolver: reqwest connects to exactly the addresses checked here (IP literals were checked at build).
pub(super) struct CheckedResolver;

impl reqwest::dns::Resolve for CheckedResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let host = name.as_str().to_ascii_lowercase();
        Box::pin(async move {
            // Port 0: hyper-util puts the URL's port on every address it is handed.
            let checked: Result<reqwest::dns::Addrs, BoxError> = match resolve_checked(&host, 0).await {
                Ok(addrs) => {
                    let addrs: reqwest::dns::Addrs = Box::new(addrs.into_iter());
                    Ok(addrs)
                }
                Err(reason) => Err(reason.into()),
            };
            checked
        })
    }
}
