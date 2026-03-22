use ipnet::IpNet;
use std::net::IpAddr;

pub const BLOCKED_HOSTS: &[&str] = &["localhost", "127.0.0.1", "::1", "0.0.0.0"];

pub const BLOCKED_RANGES: &[&str] = &[
    "10.0.0.0/8",
    "172.16.0.0/12",
    "192.168.0.0/16",
    "169.254.0.0/16",
    "fc00::/7",
];

pub fn is_url_allowed(url: &url::Url) -> bool {
    // Only allow http and https schemes
    match url.scheme() {
        "http" | "https" => {}
        _ => return false,
    }
    if let Some(host) = url.host_str() {
        if BLOCKED_HOSTS.contains(&host) {
            return false;
        }
        if let Ok(ip) = host.parse::<IpAddr>() {
            for range in BLOCKED_RANGES {
                if let Ok(net) = range.parse::<IpNet>() {
                    if net.contains(&ip) {
                        return false;
                    }
                }
            }
        }
    }
    true
}
