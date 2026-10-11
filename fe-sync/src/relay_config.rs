//! Relay server configuration + reachability health (P2P relay-health hardening,
//! track `p2p_asset_streaming_20260718` FR-1 / decision D-77).
//!
//! See `AGENTS.md` §relay-health for the relay-infrastructure notes, the
//! health model, and the continuous home-relay watcher (SYNC-RELAYWATCH).

use std::str::FromStr;

/// Which relay servers [`crate::endpoint::SyncEndpoint`] should bind against.
///
/// The default matches iroh's own out-of-the-box behavior (n0-hosted relay
/// servers) so existing deployments are unaffected until an operator opts
/// into `Custom` or `Disabled`.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum RelayConfig {
    /// iroh's n0-hosted default (public, rate-limited) relay servers.
    ///
    /// Fine for development; production should configure `Custom` — see
    /// AGENTS.md §relay-health.
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

/// Relay spec from the GUI's persisted `AppSettings`, registered once before
/// the sync thread spawns.
static SETTINGS_RELAY_SPEC: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// Registers the settings-sourced relay spec (first call wins; later calls are ignored).
pub fn set_settings_relay_spec(raw: String) {
    if SETTINGS_RELAY_SPEC.set(raw).is_err() {
        tracing::warn!("settings relay spec already registered; ignoring the new value");
    }
}

/// Relay precedence: the environment override wins over the settings spec;
/// absent both → [`RelayConfig::Default`]. A spec that fails to parse warns and
/// falls back to `Default` (it does not fall through to the next source).
pub fn resolve(env: Option<&str>, settings: Option<&str>) -> RelayConfig {
    let (source, raw) = match (env, settings) {
        (Some(raw), _) => (RELAY_CONFIG_ENV_VAR, raw),
        (None, Some(raw)) => ("settings.relay", raw),
        (None, None) => return RelayConfig::Default,
    };
    RelayConfig::parse(raw).unwrap_or_else(|e| {
        tracing::warn!(
            source,
            error = %e,
            "Invalid relay config, falling back to default n0 relays"
        );
        RelayConfig::Default
    })
}

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

    /// The startup relay config: [`RELAY_CONFIG_ENV_VAR`] if set, else the
    /// application-settings spec registered via [`set_settings_relay_spec`],
    /// else [`RelayConfig::Default`]. See AGENTS.md §relay-health.
    pub fn from_env() -> Self {
        resolve(
            std::env::var(RELAY_CONFIG_ENV_VAR).ok().as_deref(),
            SETTINGS_RELAY_SPEC.get().map(String::as_str),
        )
    }

    /// Convert to the [`iroh::RelayMode`] the endpoint builder expects.
    pub fn to_relay_mode(&self) -> Result<iroh::RelayMode, RelayConfigError> {
        match self {
            Self::Default => Ok(iroh::RelayMode::Default),
            Self::Disabled => Ok(iroh::RelayMode::Disabled),
            Self::Custom(urls) => {
                let mut relays = Vec::with_capacity(urls.len());
                for raw in urls {
                    let url = iroh::RelayUrl::from_str(raw).map_err(|e| {
                        RelayConfigError::InvalidUrl {
                            raw: raw.clone(),
                            reason: e.to_string(),
                        }
                    })?;
                    relays.push(url);
                }
                if relays.is_empty() {
                    // iroh 1.x refuses to bind a Custom map with no relays.
                    return Err(RelayConfigError::Empty);
                }
                Ok(iroh::RelayMode::custom(relays))
            }
        }
    }

    /// `true` if this config relies on iroh's default (public, rate-limited
    /// n0-hosted) relay infra.
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
    ///
    /// iroh 1.x binds without contacting a relay, so a successful bind proves
    /// nothing about relay reachability: relay configs start at `Unknown` and
    /// the home-relay watcher ([`Self::on_relay_status`]) promotes them once a
    /// relay connects. `Disabled` is the fixed point.
    pub fn on_bind_success(relay_config: &RelayConfig) -> Self {
        if matches!(relay_config, RelayConfig::Disabled) {
            Self::Disabled
        } else {
            Self::Unknown
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

    /// Fold one home-relay status snapshot into the health (SYNC-RELAYWATCH).
    ///
    /// Any connected relay → [`Self::on_success`]; otherwise an auth denial →
    /// `Unreachable` directly (it never self-heals); otherwise any recorded
    /// connection error → [`Self::on_error`] (two strikes from `Healthy`, the
    /// anti-flap rule); an empty snapshot (no home relay chosen yet) or one
    /// with no error yet leaves the health unchanged. `Disabled` is a fixed
    /// point.
    pub fn on_relay_status(self, statuses: &[RelayStatusSummary]) -> Self {
        if self == Self::Disabled {
            return Self::Disabled;
        }
        if statuses.iter().any(|s| s.connected) {
            self.on_success()
        } else if statuses.iter().any(|s| s.auth_denied) {
            Self::Unreachable
        } else if statuses.iter().any(|s| s.has_error) {
            self.on_error()
        } else {
            self
        }
    }
}

/// Pure summary of one `iroh::endpoint::RelayStatus` — the input to
/// [`RelayHealth::on_relay_status`]. (Upstream `RelayStatus::new` is
/// crate-private, so the mapping is tested against this summary.)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RelayStatusSummary {
    /// The endpoint is connected to this home relay.
    pub connected: bool,
    /// A connection attempt to this relay has failed (`last_error()` is set).
    pub has_error: bool,
    /// The relay denied our authentication (`auth_denied_reason()` is set).
    pub auth_denied: bool,
}

impl RelayStatusSummary {
    /// Summarize an upstream relay status entry.
    pub fn from_status(status: &iroh::endpoint::RelayStatus) -> Self {
        Self {
            connected: status.is_connected(),
            has_error: status.last_error().is_some(),
            auth_denied: status.auth_denied_reason().is_some(),
        }
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

    // -------------------------------------------------------------------
    // resolve (env > settings > default)
    // -------------------------------------------------------------------

    #[test]
    fn resolve_precedence_table() {
        let custom = |url: &str| RelayConfig::Custom(vec![url.to_string()]);
        let cases: [(Option<&str>, Option<&str>, RelayConfig); 7] = [
            (None, None, RelayConfig::Default),
            (None, Some("disabled"), RelayConfig::Disabled),
            (
                None,
                Some("https://settings.example.com"),
                custom("https://settings.example.com"),
            ),
            (
                Some("https://env.example.com"),
                Some("disabled"),
                custom("https://env.example.com"),
            ),
            (Some("disabled"), None, RelayConfig::Disabled),
            // An invalid source falls back to Default, never to the next source.
            (Some("not a url"), Some("disabled"), RelayConfig::Default),
            (None, Some("not a url"), RelayConfig::Default),
        ];
        for (env, settings, expected) in cases {
            assert_eq!(
                resolve(env, settings),
                expected,
                "env {env:?}, settings {settings:?}"
            );
        }
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
    fn on_bind_success_unknown_for_default_and_custom() {
        // A bind contacts no relay on iroh 1.x — the watcher confirms later.
        assert_eq!(
            RelayHealth::on_bind_success(&RelayConfig::Default),
            RelayHealth::Unknown
        );
        assert_eq!(
            RelayHealth::on_bind_success(&RelayConfig::Custom(
                vec!["https://x.example.com".into()]
            )),
            RelayHealth::Unknown
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

    // -------------------------------------------------------------------
    // RelayHealth::on_relay_status (SYNC-RELAYWATCH mapping)
    // -------------------------------------------------------------------

    #[test]
    fn on_relay_status_transition_table() {
        let connected = RelayStatusSummary {
            connected: true,
            ..Default::default()
        };
        let errored = RelayStatusSummary {
            has_error: true,
            ..Default::default()
        };
        let denied = RelayStatusSummary {
            has_error: true,
            auth_denied: true,
            ..Default::default()
        };
        let pending = RelayStatusSummary::default();
        use RelayHealth::*;
        let cases: &[(RelayHealth, &[RelayStatusSummary], RelayHealth)] = &[
            // First connection confirms the relay.
            (Unknown, &[connected], Healthy),
            (Degraded, &[connected], Healthy),
            (Unreachable, &[connected], Healthy),
            // One connected home relay outweighs a failing one.
            (Healthy, &[errored, connected], Healthy),
            // Two strikes from Healthy (anti-flap).
            (Healthy, &[errored], Degraded),
            (Degraded, &[errored], Unreachable),
            (Unknown, &[errored], Unreachable),
            // Auth denial never self-heals: straight to Unreachable.
            (Healthy, &[denied], Unreachable),
            // No home relay chosen yet / no failure observed: unchanged.
            (Unknown, &[], Unknown),
            (Healthy, &[], Healthy),
            (Healthy, &[pending], Healthy),
            // Disabled is a fixed point.
            (Disabled, &[connected], Disabled),
            (Disabled, &[denied], Disabled),
        ];
        for (from, statuses, expected) in cases {
            assert_eq!(
                from.on_relay_status(statuses),
                *expected,
                "{from:?} + {statuses:?}"
            );
        }
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
