//! Static-sandbox feeds: the node's `portal.feeds` admitted into `/__fe/feeds.json` and the static CSP (admission.md §7); see `portal_bundle/AGENTS.md` §feeds.

use std::collections::{BTreeMap, BTreeSet};

use http::header::{CONTENT_LENGTH, CONTENT_SECURITY_POLICY, CONTENT_TYPE};
use http::{HeaderValue, Method, Request, Response, StatusCode};
use serde_json::{json, Map, Value};
use url::{Host, Url};

use super::ipc::PortalBridge;
use super::mount::PortalMounts;
use super::origin;

/// Host-served feed document (admission.md §7.3).
pub const FEEDS_PATH: &str = "/__fe/feeds.json";
/// Feeds kept per mount, in name order; the rest are left out.
pub const MAX_FEEDS: usize = 16;
/// The one route a feed URL may name: `GET /api/v1/shared/{token}`.
const SHARE_ROUTE: &str = "/api/v1/shared/";
/// Longest share token kept.
const MAX_TOKEN_BYTES: usize = 4096;
/// Longest feed name (README §9.3: `[a-z][a-z0-9_]{0,47}`).
const MAX_NAME_BYTES: usize = 48;
const FEEDS_CONTENT_TYPE: &str = "application/json; charset=utf-8";

/// What a resolver reports for one repository node's static feeds.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FeedSource {
    /// `properties.portal.feeds` as written on the node: name → share URL.
    pub feeds: BTreeMap<String, String>,
    /// This peer's own fe-api origin (loopback; default `http://127.0.0.1:8765`).
    pub local_api_origin: Option<String>,
}

/// The kept feeds of one static mount.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Feeds {
    /// The `/__fe/feeds.json` body.
    pub document: Vec<u8>,
    /// `<origin>/api/v1/shared/` of each kept feed: the static CSP's `connect-src` gains these and nothing else.
    pub sources: Vec<String>,
}

/// Keep only feeds that are exactly `<origin>/api/v1/shared/<token>` on this peer's fe-api or an allowed https origin (§7.4).
pub fn admit(source: &FeedSource) -> Feeds {
    let local = source.local_api_origin.as_deref().and_then(local_origin);
    let mut kept = Map::new();
    let mut sources = BTreeSet::new();
    for (name, raw) in &source.feeds {
        if kept.len() == MAX_FEEDS {
            break;
        }
        if !is_feed_name(name) {
            continue;
        }
        let Some((origin, token)) = share_url(raw, local.as_deref()) else {
            continue;
        };
        kept.insert(
            name.clone(),
            json!({ "url": format!("{origin}{SHARE_ROUTE}{token}"), "expires_at": token_expiry(&token) }),
        );
        // Path-scoped source: the static page reaches the share route only, never the rest of that API.
        sources.insert(format!("{origin}{SHARE_ROUTE}"));
    }
    let document = serde_json::to_vec(&json!({ "v": 1, "feeds": Value::Object(kept) })).unwrap_or_default();
    Feeds {
        document,
        sources: sources.into_iter().collect(),
    }
}

/// The `fe-portal` handler: members and the page API (`ipc::serve`), plus `/__fe/feeds.json` for Static mounts only.
pub fn serve(mounts: &PortalMounts, bridge: &PortalBridge, request: &Request<Vec<u8>>) -> Response<Vec<u8>> {
    let mut response = super::ipc::serve(mounts, bridge, request);
    // A reserved-path 404 already passed the method, worker, origin, mount and path checks.
    if response.status() != StatusCode::NOT_FOUND {
        return response;
    }
    let uri = request.uri().to_string();
    let Some((label, tail)) = origin::split(&uri) else {
        return response;
    };
    let path = tail.split(|c: char| c == '?' || c == '#').next().unwrap_or_default();
    if path != FEEDS_PATH {
        return response;
    }
    let Some(mount) = mounts.lookup(&label) else {
        return response;
    };
    let Some(document) = mount.feeds_document() else {
        return response;
    };
    let length = document.len() as u64;
    *response.status_mut() = StatusCode::OK;
    let headers = response.headers_mut();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static(FEEDS_CONTENT_TYPE));
    headers.insert(CONTENT_LENGTH, HeaderValue::from(length));
    headers.insert(CONTENT_SECURITY_POLICY, mount.csp().clone());
    *response.body_mut() = if request.method() == Method::HEAD { Vec::new() } else { document.to_vec() };
    response
}

/// This peer's fe-api origin, accepted only as `http://<loopback IP>:<port>` with nothing after it.
fn local_origin(raw: &str) -> Option<String> {
    let url = Url::parse(raw).ok()?;
    let loopback = match url.host()? {
        Host::Ipv4(ip) => ip.is_loopback(),
        Host::Ipv6(ip) => ip.is_loopback(),
        Host::Domain(_) => false,
    };
    let bare = url.scheme() == "http"
        && loopback
        && url.username().is_empty()
        && url.password().is_none()
        && matches!(url.path(), "" | "/")
        && url.query().is_none()
        && url.fragment().is_none();
    bare.then(|| url.origin().ascii_serialization())
}

/// `(origin, token)` when `raw` is exactly a share URL on the local fe-api or an https origin the raw policy allows.
fn share_url(raw: &str, local: Option<&str>) -> Option<(String, String)> {
    let url = Url::parse(raw).ok()?;
    if !url.username().is_empty() || url.password().is_some() || url.query().is_some() || url.fragment().is_some() {
        return None;
    }
    let token = url.path().strip_prefix(SHARE_ROUTE).filter(|token| is_share_token(token))?;
    let origin = url.origin().ascii_serialization();
    let admissible =
        local == Some(origin.as_str()) || (url.scheme() == "https" && fe_runtime::portal_url::is_url_allowed(&url));
    // An over-long source is left out here rather than Blocking the whole mount at CSP composition.
    let fits = origin.len() + SHARE_ROUTE.len() <= super::csp::MAX_SOURCE_BYTES;
    (admissible && fits && is_host_source(&origin)).then(|| (origin, token.to_owned()))
}

/// `[a-z][a-z0-9_]{0,47}`.
fn is_feed_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= MAX_NAME_BYTES
        && bytes[0].is_ascii_lowercase()
        && bytes[1..].iter().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'_')
}

/// `<payload>.<signature>`, both base64url: the share-token shape (fe-api `share.rs`).
fn is_share_token(token: &str) -> bool {
    let base64url = |part: &str| !part.is_empty() && part.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_');
    token.len() <= MAX_TOKEN_BYTES && token.split_once('.').is_some_and(|(payload, signature)| base64url(payload) && base64url(signature))
}

/// One CSP host-source in the composer's grammar, so a feed can never break the policy (csp.rs).
fn is_host_source(origin: &str) -> bool {
    let rest = origin.strip_prefix("https://").or_else(|| origin.strip_prefix("http://"));
    rest.is_some_and(|rest| {
        !rest.is_empty()
            && rest
                .bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, b'.' | b'-' | b':' | b'[' | b']'))
    })
}

/// The payload's `exp` as RFC 3339, read without verifying the signature: a display hint; the share route decides.
fn token_expiry(token: &str) -> Value {
    let exp = token
        .split_once('.')
        .and_then(|(payload, _)| base64url_decode(payload))
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|claims| claims.get("exp").and_then(Value::as_u64));
    exp.and_then(rfc3339_utc).map_or(Value::Null, Value::String)
}

/// RFC 4648 §5 base64url without padding; `None` on any other byte.
fn base64url_decode(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let mut buffer: u32 = 0;
    let mut bits: u32 = 0;
    for byte in text.bytes() {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'-' => 62,
            b'_' => 63,
            _ => return None,
        };
        buffer = (buffer << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
            buffer &= (1 << bits) - 1;
        }
    }
    Some(out)
}

/// UNIX seconds → `YYYY-MM-DDTHH:MM:SSZ` (Howard Hinnant's `civil_from_days`); `None` past year 9999.
fn rfc3339_utc(seconds: u64) -> Option<String> {
    let days = i64::try_from(seconds / 86_400).ok()?;
    let rest = seconds % 86_400;
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era = (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 { month_index + 3 } else { month_index - 9 };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year <= 9_999).then(|| {
        format!(
            "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
            rest / 3_600,
            rest % 3_600 / 60,
            rest % 60
        )
    })
}
