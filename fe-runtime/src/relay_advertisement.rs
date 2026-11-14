//! `__relays` advertisement row (v1) and the single advertised-relay URL validator; see `fe-runtime/src/AGENTS.md` §relay-advertisement.

use serde::{Deserialize, Serialize};

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
    Invalid(String),
    UnsupportedScheme(String),
    MissingHost,
}

impl std::fmt::Display for RelayUrlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => write!(f, "relay url is empty"),
            Self::Invalid(reason) => write!(f, "relay url is not an absolute url: {reason}"),
            Self::UnsupportedScheme(scheme) => {
                write!(f, "relay url scheme '{scheme}' is not http or https")
            }
            Self::MissingHost => write!(f, "relay url has no host"),
        }
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

/// THE advertised-relay URL validator: absolute http/https with a host (v1; NET-14 tightens).
pub fn validate_advertised_relay_url(raw: &str) -> Result<url::Url, RelayUrlError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(RelayUrlError::Empty);
    }
    let parsed =
        url::Url::parse(trimmed).map_err(|error| RelayUrlError::Invalid(error.to_string()))?;
    match parsed.scheme() {
        "http" | "https" => {}
        other => return Err(RelayUrlError::UnsupportedScheme(other.to_string())),
    }
    if parsed.host_str().map_or(true, str::is_empty) {
        return Err(RelayUrlError::MissingHost);
    }
    Ok(parsed)
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

        let url_cases: [(&str, bool); 6] = [
            ("https://relay.example.org", true),
            ("http://relay.example.org:3340", true),
            ("ftp://relay.example.org", false),
            ("/relative/path", false),
            ("relay.example.org", false),
            ("   ", false),
        ];
        for (raw, accepted) in url_cases {
            assert_eq!(
                validate_advertised_relay_url(raw).is_ok(),
                accepted,
                "url case {raw:?}"
            );
        }
    }
}
