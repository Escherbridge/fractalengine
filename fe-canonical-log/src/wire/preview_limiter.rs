//! Per-principal, per-scope preview rate limiting (§7.2.3).
//!
//! This module MUST import nothing from `commit`, `subscription`, `snapshot`, or `cursor`;
//! see the module-boundary note in `preview.rs`. The one authorization import it does carry,
//! [`SessionAuthorizationTable`], reaches none of those four either.
//!
//! §7.2.3 requires "the service's explicit finite default" when a capability's
//! `max_preview_hz` caveat is absent, but D-CL24 reserves that number as caller policy (M7):
//! this type takes it as a required constructor parameter and defines no `Default` impl that
//! would pick one. See `src/wire/AGENTS.md` for the two undefined product numbers this leaves.

use std::collections::HashMap;

use crate::envelope::Scope;

use super::error::WireError;
use super::require_current_session_generation;
use super::session::SessionAuthorizationTable;

/// A rate expressed as an integer event budget over an integer window, never a float — no
/// canonical scalar in this crate is a float, and this type follows the same discipline even
/// though it never crosses the wire.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PreviewRateLimit {
    /// Maximum preview events admitted within one window.
    pub max_events: u32,
    /// Window length, milliseconds.
    pub window_duration_ms: u64,
}

impl PreviewRateLimit {
    /// Builds a limit; the caller supplies both numbers, no default is picked here.
    pub const fn new(max_events: u32, window_duration_ms: u64) -> Self {
        Self {
            max_events,
            window_duration_ms,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Bucket {
    window_start_ms: u64,
    count_in_window: u32,
}

/// A token-bucket-style limiter keyed by `(principal public key, exact scope)`.
///
/// Distinct from — and unreachable through any call path in — `commit`, `subscription`, or
/// `snapshot`: a rate-limited preview can never affect committed history (§7.2.3 rule 3).
pub struct PreviewRateLimiter {
    default_limit: PreviewRateLimit,
    buckets: HashMap<([u8; 32], Scope), Bucket>,
}

impl PreviewRateLimiter {
    /// Builds a limiter. `default_limit` is the finite fallback §7.2.3 requires when a
    /// principal's capability carries no `max_preview_hz` caveat; the caller chooses it.
    pub fn new(default_limit: PreviewRateLimit) -> Self {
        Self {
            default_limit,
            buckets: HashMap::new(),
        }
    }

    /// Checks and records one preview event for `principal_public_key` at `scope`, at `now_ms`.
    ///
    /// `sessions` and `session_generation` are required, not optional: §6 rule 3 stops a stale
    /// generation before any preview work, so a revoked session can never push previews into a
    /// scope it no longer holds. `effective_limit`, when given, is the caller's already-resolved
    /// capability `max_preview_hz` caveat; `None` falls back to the constructor's finite
    /// default. Returns [`WireError::PreviewRateLimited`] without mutating bucket state when the
    /// budget for the current window is exhausted.
    pub fn check_and_record(
        &mut self,
        sessions: &SessionAuthorizationTable,
        session_generation: u64,
        principal_public_key: [u8; 32],
        scope: Scope,
        effective_limit: Option<PreviewRateLimit>,
        now_ms: u64,
    ) -> Result<(), WireError> {
        require_current_session_generation(sessions, session_generation)?;

        let limit = effective_limit.unwrap_or(self.default_limit);
        let bucket = self
            .buckets
            .entry((principal_public_key, scope))
            .or_insert(Bucket {
                window_start_ms: now_ms,
                count_in_window: 0,
            });

        if now_ms.saturating_sub(bucket.window_start_ms) >= limit.window_duration_ms {
            bucket.window_start_ms = now_ms;
            bucket.count_in_window = 0;
        }

        if bucket.count_in_window >= limit.max_events {
            return Err(WireError::PreviewRateLimited);
        }
        bucket.count_in_window += 1;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::envelope::{Hash32, Identifier32};

    fn scope_a() -> Scope {
        Scope::verse_wide(Identifier32([0x11; 32]))
    }

    fn scope_b() -> Scope {
        Scope::verse_wide(Identifier32([0x22; 32]))
    }

    fn accepted_sessions() -> (SessionAuthorizationTable, u64) {
        let mut sessions = SessionAuthorizationTable::new();
        let generation = sessions
            .accept_binding([1; 16], Hash32([0xaa; 32]), [0xbb; 32])
            .expect("accept");
        (sessions, generation)
    }

    #[test]
    fn the_default_limit_applies_when_no_effective_caveat_is_supplied() {
        let mut limiter = PreviewRateLimiter::new(PreviewRateLimit::new(2, 1_000));
        let (sessions, generation) = accepted_sessions();
        let principal = [0x01; 32];

        assert!(limiter
            .check_and_record(&sessions, generation, principal, scope_a(), None, 0)
            .is_ok());
        assert!(limiter
            .check_and_record(&sessions, generation, principal, scope_a(), None, 100)
            .is_ok());
        assert_eq!(
            limiter.check_and_record(&sessions, generation, principal, scope_a(), None, 200),
            Err(WireError::PreviewRateLimited)
        );
    }

    #[test]
    fn the_window_resets_after_its_duration_elapses() {
        let mut limiter = PreviewRateLimiter::new(PreviewRateLimit::new(1, 1_000));
        let (sessions, generation) = accepted_sessions();
        let principal = [0x01; 32];

        assert!(limiter
            .check_and_record(&sessions, generation, principal, scope_a(), None, 0)
            .is_ok());
        assert_eq!(
            limiter.check_and_record(&sessions, generation, principal, scope_a(), None, 500),
            Err(WireError::PreviewRateLimited)
        );
        assert!(
            limiter
                .check_and_record(&sessions, generation, principal, scope_a(), None, 1_001)
                .is_ok(),
            "a fresh window admits events again"
        );
    }

    #[test]
    fn limits_are_separated_by_principal_and_by_exact_scope() {
        let mut limiter = PreviewRateLimiter::new(PreviewRateLimit::new(1, 1_000));
        let (sessions, generation) = accepted_sessions();
        let first_principal = [0x01; 32];
        let second_principal = [0x02; 32];

        assert!(limiter
            .check_and_record(&sessions, generation, first_principal, scope_a(), None, 0)
            .is_ok());
        assert!(
            limiter
                .check_and_record(&sessions, generation, second_principal, scope_a(), None, 0)
                .is_ok(),
            "a different principal has its own budget"
        );
        assert!(
            limiter
                .check_and_record(&sessions, generation, first_principal, scope_b(), None, 0)
                .is_ok(),
            "a different scope has its own budget"
        );
    }

    #[test]
    fn an_effective_caveat_overrides_the_default() {
        let mut limiter = PreviewRateLimiter::new(PreviewRateLimit::new(100, 1_000));
        let (sessions, generation) = accepted_sessions();
        let principal = [0x01; 32];
        let strict = PreviewRateLimit::new(1, 1_000);

        assert!(limiter
            .check_and_record(&sessions, generation, principal, scope_a(), Some(strict), 0)
            .is_ok());
        assert_eq!(
            limiter.check_and_record(
                &sessions,
                generation,
                principal,
                scope_a(),
                Some(strict),
                100
            ),
            Err(WireError::PreviewRateLimited)
        );
    }

    /// A revoked generation is rejected ahead of the budget, and consumes none of it: the stale
    /// caller cannot even use the limiter as an oracle for how much budget remains.
    #[test]
    fn a_stale_session_generation_is_rejected_without_consuming_budget() {
        let mut limiter = PreviewRateLimiter::new(PreviewRateLimit::new(1, 1_000));
        let (mut sessions, generation) = accepted_sessions();
        let principal = [0x01; 32];
        sessions.invalidate();

        assert_eq!(
            limiter.check_and_record(&sessions, generation, principal, scope_a(), None, 0),
            Err(WireError::StaleSessionGeneration { generation })
        );

        let fresh = sessions
            .accept_binding([1; 16], Hash32([0xaa; 32]), [0xbb; 32])
            .expect("re-accept");
        assert!(
            limiter
                .check_and_record(&sessions, fresh, principal, scope_a(), None, 0)
                .is_ok(),
            "the rejected stale event consumed no budget"
        );
    }
}
