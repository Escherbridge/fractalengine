//! Canonical literal-address URL policy; see src/AGENTS.md section security.

use ipnet::IpNet;
use std::net::IpAddr;

pub const BLOCKED_HOSTS: &[&str] = &[
    "localhost",
    "localhost.localdomain",
    "localhost6",
    "localhost6.localdomain6",
    "ip6-localhost",
    "ip6-loopback",
    "127.0.0.1",
    "::1",
    "0.0.0.0",
];

pub const BLOCKED_RANGES: &[&str] = &[
    "10.0.0.0/8",
    "172.16.0.0/12",
    "192.168.0.0/16",
    "169.254.0.0/16",
    "fc00::/7",
];

fn blocked_address(address: IpAddr) -> bool {
    address.is_loopback()
        || matches!(address, IpAddr::V4(ip) if ip.is_unspecified())
        || BLOCKED_RANGES.iter().any(|range| {
            range
                .parse::<IpNet>()
                .is_ok_and(|net| net.contains(&address))
        })
}

pub fn is_url_allowed(url: &url::Url) -> bool {
    if !matches!(url.scheme(), "http" | "https") {
        return false;
    }
    match url.host() {
        Some(url::Host::Domain(host)) => {
            let host = host.trim_end_matches('.');
            !BLOCKED_HOSTS
                .iter()
                .any(|blocked| host.eq_ignore_ascii_case(blocked))
                && !host
                    .rsplit('.')
                    .next()
                    .is_some_and(|label| label.eq_ignore_ascii_case("localhost"))
        }
        Some(url::Host::Ipv4(ip)) => !blocked_address(IpAddr::V4(ip)),
        Some(url::Host::Ipv6(ip)) => {
            !blocked_address(IpAddr::V6(ip))
                && ip
                    .to_ipv4()
                    .is_none_or(|mapped| !blocked_address(IpAddr::V4(mapped)))
        }
        None => false,
    }
}

/// Pure preview for editor validation; effect admission uses `embedded_allowed`.
pub fn embedded_preview_allowed(raw: &str) -> bool {
    raw.parse::<url::Url>()
        .is_ok_and(|url| is_url_allowed(&url))
        || crate::qa_web::preview_allows(raw)
}

/// Shared persistence/navigation admission with live fixture ownership checks.
pub fn embedded_allowed(raw: &str) -> bool {
    raw.parse::<url::Url>()
        .is_ok_and(|url| is_url_allowed(&url))
        || crate::qa_web::allows(raw)
}
