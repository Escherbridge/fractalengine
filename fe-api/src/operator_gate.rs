//! Guard chain shared by every operator-gated endpoint (elevated query,
//! screenshot, camera, input): opt-in → typed Owner → per-DID rate — see
//! AGENTS.md §operator-gate.

use fe_identity::api_token::ApiClaims;

use crate::server::ApiState;

/// Refusal text for a token below Owner (identical on every gated surface).
pub const OWNER_REQUIRED: &str = "insufficient permissions (owner required)";

/// One deny-by-default endpoint's policy: its opt-in env var and its rate budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OperatorGate {
    /// Env var whose value `1` (and only `1`) opts the node in; `0` opts it
    /// out even when `default_on` is `true`. Any other value is ignored
    /// (fail closed) — see [`opt_in_value_enables`].
    pub opt_in_env: &'static str,
    /// Human name used in logs ("elevated query endpoint").
    pub endpoint: &'static str,
    /// The loud startup WARN when the operator opts in (what the switch exposes).
    pub enabled_warning: &'static str,
    /// Refusal text while the node has not opted in.
    pub disabled_message: &'static str,
    /// Rate-limiter key prefix; the key is `<prefix>:<did>`.
    pub rate_key_prefix: &'static str,
    /// Calls admitted per DID per second.
    pub rate_per_sec: u32,
    /// The limit as named in the 429 text ("1 screenshot/sec").
    pub rate_label: &'static str,
    /// Whether an unset (or empty) env value enables this surface
    /// (agent_vision_20261031 DD-1/§5.1). `true` for screenshot/camera
    /// (default-on, `=0` disables); `false` for elevated-query/input
    /// (default-off, `=1` enables) — the pre-existing posture.
    pub default_on: bool,
}

/// Why a gate refused, in gate order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateRefusal {
    /// The operator has not opted in (carries the gate's refusal text).
    Disabled(&'static str),
    /// The token is below Owner.
    InsufficientRole,
    /// Over the per-DID budget (carries the limiter's text).
    RateLimited(String),
}

impl GateRefusal {
    /// Client-facing text (REST error body, MCP tool error).
    pub fn message(&self) -> String {
        match self {
            Self::Disabled(message) => (*message).to_string(),
            Self::InsufficientRole => OWNER_REQUIRED.to_string(),
            Self::RateLimited(reason) => reason.clone(),
        }
    }
}

/// Resolve one env value against a gate's default (spec agent_vision_20261031
/// §5.1's table): unset/`""` takes `default_on`; `1` always enables; `0`
/// always disables; anything else always disables (fail closed), regardless
/// of `default_on`.
pub fn opt_in_value_enables(value: Option<&str>, default_on: bool) -> bool {
    match value {
        None | Some("") => default_on,
        Some("1") => true,
        Some("0") => false,
        Some(_) => false,
    }
}

impl OperatorGate {
    /// Read the opt-in once at startup against this gate's `default_on`
    /// (§5.1's table): loud WARN whenever the result is on — a default-on
    /// variant when the env was unset, the plain variant on an explicit `1` —
    /// a warning for any unrecognized value (fail closed either way), and
    /// silence for an explicit `0` or an unset default-off gate.
    pub fn opted_in_from_env(&self) -> bool {
        let value = std::env::var(self.opt_in_env).ok();
        let value_ref = value.as_deref();
        let enabled = opt_in_value_enables(value_ref, self.default_on);
        let unset_or_empty = matches!(value_ref, None | Some(""));
        if enabled {
            if unset_or_empty {
                tracing::warn!(
                    "{} unset — {} enabled by default: {} (set {}=0 to disable)",
                    self.opt_in_env, self.endpoint, self.enabled_warning, self.opt_in_env
                );
            } else {
                tracing::warn!("{}=1: {}", self.opt_in_env, self.enabled_warning);
            }
        } else if let Some(other) = value_ref.filter(|v| !v.is_empty() && *v != "0") {
            tracing::warn!(
                "{}={other:?} ignored (only `1`/`0` are recognized) — {} stays disabled{}",
                self.opt_in_env,
                self.endpoint,
                if self.default_on {
                    " (fail closed despite the default-on policy)"
                } else {
                    ""
                }
            );
        }
        enabled
    }

    /// Gate 1: the operator opt-in, checked before role, rate limit or work.
    pub fn check_opted_in(&self, allowed: bool) -> Result<(), GateRefusal> {
        if allowed {
            Ok(())
        } else {
            Err(GateRefusal::Disabled(self.disabled_message))
        }
    }

    /// Gate 3: the per-DID budget (shares the query limiter map).
    pub async fn check_rate(
        &self,
        state: &ApiState,
        claims: &ApiClaims,
    ) -> Result<(), GateRefusal> {
        crate::query_guard::check_rate_limit(
            state,
            &format!("{}:{}", self.rate_key_prefix, claims.sub),
            self.rate_per_sec,
            self.rate_label,
        )
        .await
        .map_err(GateRefusal::RateLimited)
    }

    /// The full REST chain: opt-in → Owner (typed `RoleLevel`, so a role-string
    /// typo can never parse to admit-all) → rate. MCP runs the role floor in its
    /// dispatcher instead (FR-5) and calls the other two gates directly.
    pub async fn admit(
        &self,
        allowed: bool,
        state: &ApiState,
        claims: &ApiClaims,
    ) -> Result<(), GateRefusal> {
        self.check_opted_in(allowed)?;
        if crate::auth::require_role_level(claims, fe_database::RoleLevel::Owner).is_err() {
            return Err(GateRefusal::InsufficientRole);
        }
        self.check_rate(state, claims).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// spec agent_vision_20261031 §5.1's table, both `default_on` columns:
    /// unset/`""` follows the default, `1`/`0` are absolute, and garbage
    /// values fail closed either way.
    #[test]
    fn per_gate_default_decides_the_unset_and_garbage_cases() {
        let cases: &[(Option<&str>, bool, bool)] = &[
            (None, false, false),
            (None, true, true),
            (Some(""), false, false),
            (Some(""), true, true),
            (Some("1"), false, true),
            (Some("1"), true, true),
            (Some("0"), false, false),
            (Some("0"), true, false),
            (Some("true"), false, false),
            (Some("true"), true, false),
            (Some("yes"), false, false),
            (Some("yes"), true, false),
            (Some(" 1"), false, false),
            (Some(" 1"), true, false),
            (Some("1 "), false, false),
            (Some("1 "), true, false),
        ];
        for &(value, default_on, expected) in cases {
            assert_eq!(
                opt_in_value_enables(value, default_on),
                expected,
                "value={value:?} default_on={default_on}"
            );
        }
    }
}
