//! Relay server configuration + reachability health (P2P relay-health hardening,
//! track `p2p_asset_streaming_20260718` FR-1 / decision D-77).
//!
//! See `AGENTS.md` §relay-health for the EOL timeline and design rationale.

use std::str::FromStr;

/// Which relay servers [`crate::endpoint::SyncEndpoint`] should bind against.
///
/// The default matches iroh's own out-of-the-box behavior (n0-hosted relay
/// servers) so existing deployments are unaffected until an operator opts
/// into `Custom` or `Disabled`.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum RelayConfig {
    /// iroh's n0-hosted default relay servers.
    ///
    /// This infrastructure EOLs **2026-12-31** — see AGENTS.md §relay-health.
    #[default]
    Default,
    /// No relay — direct/LAN-only connectivity.
    Disabled,
    /// One or more operator-supplied relay URLs (validated at parse time).
    Custom(Vec<String>),
}

/// Error constructing a [`RelayConfig`] from operator input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RelayConfigError {
    /// A `Custom` URL list was supplied but every entry was blank.
    Empty,
    /// One of the supplied URLs failed iroh's `RelayUrl` parser.
    InvalidUrl { raw: String, reason: String },
}

impl std::fmt::Display for RelayConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => write!(f, "relay URL list was empty after parsing"),
            Self::InvalidUrl { raw, reason } => write!(f, "invalid relay URL {raw:?}: {reason}"),
        }
    }
}

impl std::error::Error for RelayConfigError {}

/// Environment variable operators use to override the relay config (see
/// [`RelayConfig::from_env`]).
pub const RELAY_CONFIG_ENV_VAR: &str = "FE_SYNC_RELAY";

impl RelayConfig {
    /// Parse an operator-supplied relay config string.
    ///
    /// Accepts the case-insensitive keywords `"default"`, `"disabled"`/`"none"`,
    /// or one-or-more relay URLs separated by commas. Every URL is validated
    /// eagerly with iroh's own [`iroh::RelayUrl`] parser so a misconfigured
    /// relay fails at parse time, not on first dial.
    pub fn parse(raw: &str) -> Result<Self, RelayConfigError> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Ok(Self::Default);
        }
        match trimmed.to_ascii_lowercase().as_str() {
            "default" => return Ok(Self::Default),
            "disabled" | "none" => return Ok(Self::Disabled),
            _ => {}
        }

        let mut urls = Vec::new();
        for part in trimmed.split(',') {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            iroh::RelayUrl::from_str(part).map_err(|e| RelayConfigError::InvalidUrl {
                raw: part.to_string(),
                reason: e.to_string(),
            })?;
            urls.push(part.to_string());
        }

        if urls.is_empty() {
            return Err(RelayConfigError::Empty);
        }
        Ok(Self::Custom(urls))
    }

    /// Read [`RELAY_CONFIG_ENV_VAR`] and parse it, falling back to
    /// [`RelayConfig::Default`] (with a loud warning) on a missing var or a
    /// parse failure.
    ///
    /// TODO(ultrapilot): promote this to a real `AppSettings`-sourced value
    /// once FR-7 (application settings surface, D-78) lands. `spawn_sync_thread`'s
    /// signature is intentionally left unchanged here to avoid touching its 3
    /// external call sites (`fractalengine/src/main.rs`,
    /// `fractalengine-relay/src/main.rs`, `fe-test-harness/src/peer.rs`) which
    /// are outside this worker's `fe-sync/src/**` write scope.
    pub fn from_env() -> Self {
        match std::env::var(RELAY_CONFIG_ENV_VAR) {
            Ok(raw) => Self::parse(&raw).unwrap_or_else(|e| {
                tracing::warn!(
                    var = RELAY_CONFIG_ENV_VAR,
                    error = %e,
                    "Invalid relay config in environment, falling back to default n0 relays"
                );
                Self::Default
            }),
            Err(_) => Self::Default,
        }
    }

    /// Convert to the [`iroh::RelayMode`] the endpoint builder expects.
    pub fn to_relay_mode(&self) -> Result<iroh::RelayMode, RelayConfigError> {
        match self {
            Self::Default => Ok(iroh::RelayMode::Default),
            Self::Disabled => Ok(iroh::RelayMode::Disabled),
            Self::Custom(urls) => {
                let mut nodes = Vec::with_capacity(urls.len());
                for raw in urls {
                    let url = iroh::RelayUrl::from_str(raw).map_err(|e| {
                        RelayConfigError::InvalidUrl {
                            raw: raw.clone(),
                            reason: e.to_string(),
                        }
                    })?;
                    nodes.push(iroh::RelayNode::from(url));
                }
                Ok(iroh::RelayMode::Custom(nodes.into_iter().collect()))
            }
        }
    }

    /// `true` if this config relies on iroh's default (n0-hosted, EOL-bound) infra.
    pub fn is_default_infra(&self) -> bool {
        matches!(self, Self::Default)
    }
}

/// Observed reachability of the configured relay(s), surfaced to consumers via
/// [`crate::status::SyncStatus::health`] so relay failure is visible instead of
/// silently queuing forever (see AGENTS.md §relay-health).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum RelayHealth {
    /// No health signal observed yet (thread just started).
    #[default]
    Unknown,
    /// Relay contact succeeded (or no relay is configured/needed).
    Healthy,
    /// A relay operation failed after a prior success — may self-heal.
    Degraded,
    /// The relay could not be reached (or repeated failures after `Degraded`).
    Unreachable,
    /// Relay is intentionally turned off via [`RelayConfig::Disabled`] — not a
    /// failure state, excluded from [`RelayHealth::is_problem`].
    Disabled,
}

impl RelayHealth {
    /// Health immediately after a successful [`crate::endpoint::SyncEndpoint::new`].
    pub fn on_bind_success(relay_config: &RelayConfig) -> Self {
        if matches!(relay_config, RelayConfig::Disabled) {
            Self::Disabled
        } else {
            Self::Healthy
        }
    }

    /// Health after `SyncEndpoint::new` fails to bind at all.
    pub fn on_bind_failure() -> Self {
        Self::Unreachable
    }

    /// Record a runtime relay error. `Healthy` degrades one step; anything
    /// already troubled (or never established) becomes `Unreachable`.
    /// `Disabled` is unaffected — a disabled relay cannot "fail".
    pub fn on_error(self) -> Self {
        match self {
            Self::Disabled => Self::Disabled,
            Self::Healthy => Self::Degraded,
            Self::Degraded | Self::Unreachable | Self::Unknown => Self::Unreachable,
        }
    }

    /// Record a runtime relay success signal, recovering to `Healthy`.
    /// `Disabled` is unaffected.
    pub fn on_success(self) -> Self {
        match self {
            Self::Disabled => Self::Disabled,
            _ => Self::Healthy,
        }
    }

    /// `true` for states an operator should treat as "something's wrong".
    pub fn is_problem(self) -> bool {
        matches!(self, Self::Degraded | Self::Unreachable)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -------------------------------------------------------------------
    // RelayConfig::parse
    // -------------------------------------------------------------------

    #[test]
    fn parse_empty_is_default() {
        assert_eq!(RelayConfig::parse("").unwrap(), RelayConfig::Default);
        assert_eq!(RelayConfig::parse("   ").unwrap(), RelayConfig::Default);
    }

    #[test]
    fn parse_keyword_default_case_insensitive() {
        assert_eq!(RelayConfig::parse("default").unwrap(), RelayConfig::Default);
        assert_eq!(RelayConfig::parse("DEFAULT").unwrap(), RelayConfig::Default);
    }

    #[test]
    fn parse_keyword_disabled_and_none() {
        assert_eq!(
            RelayConfig::parse("disabled").unwrap(),
            RelayConfig::Disabled
        );
        assert_eq!(RelayConfig::parse("None").unwrap(), RelayConfig::Disabled);
    }

    #[test]
    fn parse_single_custom_url() {
        let cfg = RelayConfig::parse("https://relay.example.com").unwrap();
        assert_eq!(
            cfg,
            RelayConfig::Custom(vec!["https://relay.example.com".to_string()])
        );
    }

    #[test]
    fn parse_multiple_custom_urls_with_whitespace() {
        let cfg = RelayConfig::parse(" https://a.example.com , https://b.example.com ").unwrap();
        assert_eq!(
            cfg,
            RelayConfig::Custom(vec![
                "https://a.example.com".to_string(),
                "https://b.example.com".to_string(),
            ])
        );
    }

    #[test]
    fn parse_rejects_invalid_url() {
        let err = RelayConfig::parse("not a url").unwrap_err();
        assert!(matches!(err, RelayConfigError::InvalidUrl { .. }));
    }

    #[test]
    fn parse_rejects_all_blank_custom_entries() {
        let err = RelayConfig::parse(" , , ").unwrap_err();
        assert_eq!(err, RelayConfigError::Empty);
    }

    #[test]
    fn relay_config_default_is_default_variant() {
        assert_eq!(RelayConfig::default(), RelayConfig::Default);
    }

    #[test]
    fn is_default_infra_only_for_default_variant() {
        assert!(RelayConfig::Default.is_default_infra());
        assert!(!RelayConfig::Disabled.is_default_infra());
        assert!(!RelayConfig::Custom(vec!["https://x.example.com".into()]).is_default_infra());
    }

    // -------------------------------------------------------------------
    // RelayConfig::to_relay_mode
    // -------------------------------------------------------------------

    #[test]
    fn to_relay_mode_default() {
        assert_eq!(
            RelayConfig::Default.to_relay_mode().unwrap(),
            iroh::RelayMode::Default
        );
    }

    #[test]
    fn to_relay_mode_disabled() {
        assert_eq!(
            RelayConfig::Disabled.to_relay_mode().unwrap(),
            iroh::RelayMode::Disabled
        );
    }

    #[test]
    fn to_relay_mode_custom_builds_relay_map() {
        let cfg = RelayConfig::Custom(vec!["https://relay.example.com".to_string()]);
        let mode = cfg.to_relay_mode().unwrap();
        match mode {
            iroh::RelayMode::Custom(map) => assert_eq!(map.len(), 1),
            other => panic!("expected RelayMode::Custom, got {other:?}"),
        }
    }

    #[test]
    fn to_relay_mode_custom_rejects_invalid_url_built_by_hand() {
        // RelayConfig::Custom can be constructed directly (bypassing `parse`);
        // to_relay_mode must still validate before handing off to iroh.
        let cfg = RelayConfig::Custom(vec!["not a url".to_string()]);
        assert!(cfg.to_relay_mode().is_err());
    }

    // -------------------------------------------------------------------
    // RelayHealth transitions
    // -------------------------------------------------------------------

    #[test]
    fn relay_health_default_is_unknown() {
        assert_eq!(RelayHealth::default(), RelayHealth::Unknown);
    }

    #[test]
    fn on_bind_success_healthy_for_default_and_custom() {
        assert_eq!(
            RelayHealth::on_bind_success(&RelayConfig::Default),
            RelayHealth::Healthy
        );
        assert_eq!(
            RelayHealth::on_bind_success(&RelayConfig::Custom(
                vec!["https://x.example.com".into()]
            )),
            RelayHealth::Healthy
        );
    }

    #[test]
    fn on_bind_success_disabled_for_disabled_config() {
        assert_eq!(
            RelayHealth::on_bind_success(&RelayConfig::Disabled),
            RelayHealth::Disabled
        );
    }

    #[test]
    fn on_bind_failure_is_unreachable() {
        assert_eq!(RelayHealth::on_bind_failure(), RelayHealth::Unreachable);
    }

    #[test]
    fn on_error_degrades_then_becomes_unreachable() {
        assert_eq!(RelayHealth::Healthy.on_error(), RelayHealth::Degraded);
        assert_eq!(RelayHealth::Degraded.on_error(), RelayHealth::Unreachable);
        assert_eq!(
            RelayHealth::Unreachable.on_error(),
            RelayHealth::Unreachable
        );
        assert_eq!(RelayHealth::Unknown.on_error(), RelayHealth::Unreachable);
    }

    #[test]
    fn on_error_disabled_is_unaffected() {
        assert_eq!(RelayHealth::Disabled.on_error(), RelayHealth::Disabled);
    }

    #[test]
    fn on_success_recovers_to_healthy() {
        assert_eq!(RelayHealth::Degraded.on_success(), RelayHealth::Healthy);
        assert_eq!(RelayHealth::Unreachable.on_success(), RelayHealth::Healthy);
        assert_eq!(RelayHealth::Unknown.on_success(), RelayHealth::Healthy);
    }

    #[test]
    fn on_success_disabled_is_unaffected() {
        assert_eq!(RelayHealth::Disabled.on_success(), RelayHealth::Disabled);
    }

    #[test]
    fn is_problem_only_for_degraded_and_unreachable() {
        assert!(!RelayHealth::Unknown.is_problem());
        assert!(!RelayHealth::Healthy.is_problem());
        assert!(RelayHealth::Degraded.is_problem());
        assert!(RelayHealth::Unreachable.is_problem());
        assert!(!RelayHealth::Disabled.is_problem());
    }
}
