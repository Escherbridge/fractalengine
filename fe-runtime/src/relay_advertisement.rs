//! `__relays` advertisement row (v1) and the single advertised-relay URL validator; see `fe-runtime/src/AGENTS.md` §relay-advertisement.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

/// Operator opt-in letting PEER-advertised relays use private ranges and plain http (default off).
pub const ALLOW_PRIVATE_RELAYS_ENV: &str = "FE_SYNC_ALLOW_PRIVATE_RELAYS";

/// Longest accepted advertised relay URL, in bytes.
pub const MAX_RELAY_URL_BYTES: usize = 2048;

/// Reserved replica table holding advertised relay rows.
pub const RELAY_ADVERT_TABLE: &str = "__relays";

/// The only row version this build understands.
pub const RELAY_ADVERT_ROW_VERSION: u32 = 1;

/// One advertised relay row; unknown extra fields are tolerated (forward compatible).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelayAdvertisementRow {
    pub v: u32,
    pub url: String,
    pub advertised_by: String,
    #[serde(default)]
    pub note: Option<String>,
}

/// Outcome of parsing raw row bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowParse {
    Row(RelayAdvertisementRow),
    /// Version other than 1: the caller warns and skips.
    UnknownVersion(u32),
    Malformed(String),
}

/// Why an advertised relay URL was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RelayUrlError {
    Empty,
    TooLong,
    Invalid(String),
    UnsupportedScheme(String),
    MissingHost,
    /// Userinfo, query or fragment: a replicated row never carries credentials or extras.
    UnexpectedComponent(&'static str),
    /// Loopback, unspecified, link-local, multicast, broadcast/reserved or `localhost`: never dialled.
    RefusedDestination(String),
    /// Private, shared (CGNAT) or unique-local range without the operator opt-in.
    PrivateDestination(String),
    /// Plain http without the operator opt-in.
    InsecureScheme,
}

impl std::fmt::Display for RelayUrlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => write!(f, "relay url is empty"),
            Self::TooLong => write!(f, "relay url is longer than {MAX_RELAY_URL_BYTES} bytes"),
            Self::Invalid(reason) => write!(f, "relay url is not an absolute url: {reason}"),
            Self::UnsupportedScheme(scheme) => {
                write!(f, "relay url scheme '{scheme}' is not http or https")
            }
            Self::MissingHost => write!(f, "relay url has no host"),
            Self::UnexpectedComponent(component) => {
                write!(f, "relay url must not carry a {component}")
            }
            Self::RefusedDestination(host) => {
                write!(f, "relay host {host} is a local, link-local, multicast or reserved address")
            }
            Self::PrivateDestination(host) => write!(
                f,
                "relay host {host} is a private address (allowed only with {ALLOW_PRIVATE_RELAYS_ENV}=1)"
            ),
            Self::InsecureScheme => write!(
                f,
                "relay url must be https (http allowed only with {ALLOW_PRIVATE_RELAYS_ENV}=1)"
            ),
        }
    }
}

/// Operator policy for peer-advertised relays; the operator's own configured relay never goes through it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AdvertisedRelayPolicy {
    /// Accept private ranges and plain http (`FE_SYNC_ALLOW_PRIVATE_RELAYS=1`).
    pub allow_private: bool,
}

impl AdvertisedRelayPolicy {
    /// Public https only (the default).
    pub const STRICT: Self = Self {
        allow_private: false,
    };

    /// Only a trimmed, case-insensitive `1`, `true` or `yes` opts in.
    pub fn from_env_value(value: Option<&str>) -> Self {
        let allow_private = value.is_some_and(|raw| {
            let raw = raw.trim();
            raw == "1" || raw.eq_ignore_ascii_case("true") || raw.eq_ignore_ascii_case("yes")
        });
        Self { allow_private }
    }

    /// Reads `FE_SYNC_ALLOW_PRIVATE_RELAYS` from the process environment.
    pub fn from_env() -> Self {
        Self::from_env_value(std::env::var(ALLOW_PRIVATE_RELAYS_ENV).ok().as_deref())
    }

    /// The process-wide policy, read once.
    pub fn process() -> Self {
        static POLICY: OnceLock<AdvertisedRelayPolicy> = OnceLock::new();
        *POLICY.get_or_init(Self::from_env)
    }
}

/// Where an address sits, as far as dialling a peer-advertised relay is concerned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddressClass {
    Public,
    /// RFC 1918, 100.64/10, fc00::/7 or fec0::/10: operator opt-in only.
    Private,
    /// Never dialled for a peer-advertised relay.
    Refused,
}

/// Reach of an accepted advertised relay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelayReach {
    Public,
    Private,
}

/// A peer-advertised relay URL that passed the validator; only constructible through it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdvertisedRelayUrl {
    url: url::Url,
    reach: RelayReach,
}

impl AdvertisedRelayUrl {
    pub fn url(&self) -> &url::Url {
        &self.url
    }

    pub fn reach(&self) -> RelayReach {
        self.reach
    }

    /// Only a public https relay may ever be promoted to this node's home relay, opt-in or not.
    pub fn home_eligible(&self) -> bool {
        self.reach == RelayReach::Public && self.url.scheme() == "https"
    }

    pub fn into_url(self) -> url::Url {
        self.url
    }
}

impl std::fmt::Display for AdvertisedRelayUrl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.url, f)
    }
}

/// Classifies one address; IPv4 embedded in IPv6 (mapped, compatible, NAT64) is judged as IPv4.
pub fn classify_relay_address(address: IpAddr) -> AddressClass {
    match address {
        IpAddr::V4(ip) => classify_ipv4(ip),
        IpAddr::V6(ip) => classify_ipv6(ip),
    }
}

fn classify_ipv4(ip: Ipv4Addr) -> AddressClass {
    let [first, second, ..] = ip.octets();
    // 0/8 "this network", 240/4 reserved and 255.255.255.255 broadcast are never relay hosts.
    if ip.is_loopback() || first == 0 || ip.is_link_local() || ip.is_multicast() || first >= 240 {
        AddressClass::Refused
    } else if ip.is_private() || (first == 100 && (64..=127).contains(&second)) {
        AddressClass::Private
    } else {
        AddressClass::Public
    }
}

fn classify_ipv6(ip: Ipv6Addr) -> AddressClass {
    let leading = ip.segments()[0];
    if ip.is_loopback() || ip.is_unspecified() || ip.is_multicast() || leading & 0xffc0 == 0xfe80 {
        return AddressClass::Refused;
    }
    if let Some(embedded) = embedded_ipv4(ip) {
        return classify_ipv4(embedded);
    }
    if leading & 0xfe00 == 0xfc00 || leading & 0xffc0 == 0xfec0 {
        AddressClass::Private
    } else {
        AddressClass::Public
    }
}

/// IPv4 carried in `::ffff:a.b.c.d` (mapped), `::a.b.c.d` (compatible) or `64:ff9b::a.b.c.d` (NAT64).
fn embedded_ipv4(ip: Ipv6Addr) -> Option<Ipv4Addr> {
    let segments = ip.segments();
    let tail = Ipv4Addr::from((u32::from(segments[6]) << 16) | u32::from(segments[7]));
    match segments {
        [0, 0, 0, 0, 0, 0xffff, _, _] | [0, 0, 0, 0, 0, 0, _, _] | [0x64, 0xff9b, 0, 0, 0, 0, _, _] => {
            Some(tail)
        }
        _ => None,
    }
}

/// Applies the address rules to one address a dial site resolved (or pinned) for an advertised relay.
pub fn check_resolved_relay_address(
    address: IpAddr,
    policy: AdvertisedRelayPolicy,
) -> Result<RelayReach, RelayUrlError> {
    match classify_relay_address(address) {
        AddressClass::Public => Ok(RelayReach::Public),
        AddressClass::Private if policy.allow_private => Ok(RelayReach::Private),
        AddressClass::Private => Err(RelayUrlError::PrivateDestination(address.to_string())),
        AddressClass::Refused => Err(RelayUrlError::RefusedDestination(address.to_string())),
    }
}

impl std::error::Error for RelayUrlError {}

/// Parse row bytes; checks the version before the full shape so a v2 row is skipped, not malformed.
pub fn parse_row(bytes: &[u8]) -> RowParse {
    let value: serde_json::Value = match serde_json::from_slice(bytes) {
        Ok(value) => value,
        Err(error) => return RowParse::Malformed(error.to_string()),
    };
    let Some(version) = value.get("v").and_then(serde_json::Value::as_u64) else {
        return RowParse::Malformed("missing or non-integer 'v'".to_string());
    };
    if version != u64::from(RELAY_ADVERT_ROW_VERSION) {
        return RowParse::UnknownVersion(u32::try_from(version).unwrap_or(u32::MAX));
    }
    match serde_json::from_value::<RelayAdvertisementRow>(value) {
        Ok(row) => RowParse::Row(row),
        Err(error) => RowParse::Malformed(error.to_string()),
    }
}

/// THE advertised-relay URL validator under the process policy (`FE_SYNC_ALLOW_PRIVATE_RELAYS`); NET-14 tightens.
pub fn validate_advertised_relay_url(raw: &str) -> Result<AdvertisedRelayUrl, RelayUrlError> {
    validate_advertised_relay_url_with(raw, AdvertisedRelayPolicy::process())
}

/// Synchronous destination check (no DNS): dial sites re-check every resolved address with `check_resolved_relay_address`.
pub fn validate_advertised_relay_url_with(
    raw: &str,
    policy: AdvertisedRelayPolicy,
) -> Result<AdvertisedRelayUrl, RelayUrlError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(RelayUrlError::Empty);
    }
    if trimmed.len() > MAX_RELAY_URL_BYTES {
        return Err(RelayUrlError::TooLong);
    }
    let parsed =
        url::Url::parse(trimmed).map_err(|error| RelayUrlError::Invalid(error.to_string()))?;
    match parsed.scheme() {
        "https" => {}
        "http" if policy.allow_private => {}
        "http" => return Err(RelayUrlError::InsecureScheme),
        other => return Err(RelayUrlError::UnsupportedScheme(other.to_string())),
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(RelayUrlError::UnexpectedComponent("userinfo"));
    }
    if parsed.query().is_some() {
        return Err(RelayUrlError::UnexpectedComponent("query"));
    }
    if parsed.fragment().is_some() {
        return Err(RelayUrlError::UnexpectedComponent("fragment"));
    }
    let reach = match parsed.host() {
        None => return Err(RelayUrlError::MissingHost),
        Some(url::Host::Ipv4(ip)) => check_resolved_relay_address(IpAddr::V4(ip), policy)?,
        Some(url::Host::Ipv6(ip)) => check_resolved_relay_address(IpAddr::V6(ip), policy)?,
        Some(url::Host::Domain(domain)) => {
            let normalized = domain.trim_end_matches('.').to_ascii_lowercase();
            if normalized.is_empty() {
                return Err(RelayUrlError::MissingHost);
            }
            if normalized == "localhost" || normalized.ends_with(".localhost") {
                return Err(RelayUrlError::RefusedDestination(normalized));
            }
            // A domain can still resolve to a refused address: the dial site checks every resolved address.
            RelayReach::Public
        }
    };
    Ok(AdvertisedRelayUrl { url: parsed, reach })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relay_advertisement_rows_and_urls_table() {
        let valid = br#"{"v":1,"url":"https://relay.example.org","advertised_by":"did:key:z1","note":"eu"}"#;
        match parse_row(valid) {
            RowParse::Row(row) => {
                assert_eq!(row.url, "https://relay.example.org");
                assert_eq!(row.note.as_deref(), Some("eu"));
            }
            other => panic!("expected Row, got {other:?}"),
        }

        let extra = br#"{"v":1,"url":"https://r.example.org","advertised_by":"d","future_field":[1,2]}"#;
        assert!(matches!(parse_row(extra), RowParse::Row(row) if row.note.is_none()));

        let v2 = br#"{"v":2,"url":"https://r.example.org","advertised_by":"d"}"#;
        assert_eq!(parse_row(v2), RowParse::UnknownVersion(2));

        assert!(matches!(parse_row(b"{not json"), RowParse::Malformed(_)));
        assert!(matches!(parse_row(br#"{"v":1}"#), RowParse::Malformed(_)));

        // (url, accepted under STRICT, accepted with the private opt-in, home eligible when accepted)
        let long_url = format!("https://relay.example.org/{}", "a".repeat(MAX_RELAY_URL_BYTES));
        let url_cases: Vec<(&str, bool, bool, bool)> = vec![
            ("https://relay.example.org", true, true, true),
            ("https://8.8.8.8:3340", true, true, true),
            ("https://[2001:db8::1]", true, true, true),
            ("https://[::ffff:8.8.8.8]", true, true, true),
            ("http://relay.example.org:3340", false, true, false),
            ("ftp://relay.example.org", false, false, false),
            ("/relative/path", false, false, false),
            ("relay.example.org", false, false, false),
            ("   ", false, false, false),
            (long_url.as_str(), false, false, false),
            ("https://user:secret@relay.example.org", false, false, false),
            ("https://relay.example.org/?token=1", false, false, false),
            ("https://relay.example.org/#frag", false, false, false),
            // Always refused: loopback, unspecified, link-local (cloud metadata), multicast, broadcast, reserved.
            ("https://127.0.0.1", false, false, false),
            ("https://2130706433", false, false, false),
            ("https://0.0.0.0", false, false, false),
            ("https://169.254.169.254", false, false, false),
            ("https://224.0.0.1", false, false, false),
            ("https://255.255.255.255", false, false, false),
            ("https://240.0.0.1", false, false, false),
            ("https://[::1]", false, false, false),
            ("https://[::]", false, false, false),
            ("https://[fe80::1]", false, false, false),
            ("https://[ff02::1]", false, false, false),
            ("https://[::ffff:127.0.0.1]", false, false, false),
            ("https://[::ffff:169.254.169.254]", false, false, false),
            ("https://[::127.0.0.1]", false, false, false),
            ("https://[64:ff9b::7f00:1]", false, false, false),
            ("https://localhost", false, false, false),
            ("https://LOCALHOST.", false, false, false),
            ("https://relay.localhost", false, false, false),
            // Private ranges: operator opt-in only, and never home eligible.
            ("https://10.0.0.5", false, true, false),
            ("https://172.16.4.4", false, true, false),
            ("https://192.168.1.10", false, true, false),
            ("https://100.64.0.1", false, true, false),
            ("https://[fd00::1]", false, true, false),
            ("https://[::ffff:10.0.0.5]", false, true, false),
            ("http://192.168.1.10:3340", false, true, false),
        ];
        let open = AdvertisedRelayPolicy { allow_private: true };
        for (raw, strict_ok, open_ok, home) in url_cases {
            let strict = validate_advertised_relay_url_with(raw, AdvertisedRelayPolicy::STRICT);
            assert_eq!(strict.is_ok(), strict_ok, "strict {raw:?}: {strict:?}");
            let opted_in = validate_advertised_relay_url_with(raw, open);
            assert_eq!(opted_in.is_ok(), open_ok, "opt-in {raw:?}: {opted_in:?}");
            if let Ok(accepted) = opted_in {
                assert_eq!(accepted.home_eligible(), home, "home eligibility {raw:?}");
            }
        }
        assert!(matches!(
            validate_advertised_relay_url_with("https://169.254.169.254", open),
            Err(RelayUrlError::RefusedDestination(_))
        ));

        // A dial site re-checks each resolved address of a domain host.
        let resolved: [(IpAddr, Option<RelayReach>); 3] = [
            ("93.184.216.34".parse().unwrap(), Some(RelayReach::Public)),
            ("10.1.2.3".parse().unwrap(), None),
            ("::ffff:127.0.0.1".parse().unwrap(), None),
        ];
        for (address, reach) in resolved {
            assert_eq!(
                check_resolved_relay_address(address, AdvertisedRelayPolicy::STRICT).ok(),
                reach,
                "resolved {address}"
            );
        }

        let env_cases: [(Option<&str>, bool); 5] =
            [(None, false), (Some(""), false), (Some("0"), false), (Some(" TRUE "), true), (Some("1"), true)];
        for (value, allow_private) in env_cases {
            assert_eq!(AdvertisedRelayPolicy::from_env_value(value).allow_private, allow_private, "{value:?}");
        }
    }
}
