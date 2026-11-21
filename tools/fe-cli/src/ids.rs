//! Id grammar for every id the CLI interpolates into a URL path or a cache key (AGENTS.md §ids).

use crate::envelope::{CliError, ExitCode};

/// Longest id accepted in a route.
const MAX_ROUTE_ID_CHARS: usize = 128;

/// `[A-Za-z0-9_-]{1,128}`: verse, fractal, petal, node and command ids.
pub fn is_route_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= MAX_ROUTE_ID_CHARS && id.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

/// The manifest package-id grammar (fe-sdk `manifest_v2::is_package_id`): two or more dot-separated labels
/// `[a-z][a-z0-9-]*`, at most 128 bytes.
pub fn is_package_id(id: &str) -> bool {
    let label = |label: &str| {
        let mut bytes = label.bytes();
        bytes.next().is_some_and(|first| first.is_ascii_lowercase()) && bytes.all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    };
    id.len() <= MAX_ROUTE_ID_CHARS && id.split('.').count() >= 2 && id.split('.').all(label)
}

/// Exit 6 unless `id` is a route id; `what` names it (`--petal`, `command`, ...).
pub fn require_route_id(what: &str, id: &str) -> Result<(), CliError> {
    if is_route_id(id) {
        Ok(())
    } else {
        Err(CliError::new(ExitCode::Usage, "invalid-input", format!("{what} must be an id ([A-Za-z0-9_-], at most 128)")))
    }
}

/// Exit 6 unless `id` follows the manifest package-id grammar.
pub fn require_package_id(id: &str) -> Result<(), CliError> {
    if is_package_id(id) {
        Ok(())
    } else {
        Err(CliError::new(ExitCode::Usage, "invalid-input", "the package id does not follow the manifest id grammar"))
    }
}

