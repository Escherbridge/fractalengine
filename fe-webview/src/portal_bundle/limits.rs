//! IPC budgets (OD-8, ipc-v1.md §8) and the token bucket enforcing them; see `portal_bundle/AGENTS.md` §ipc.

use std::time::{Duration, Instant};

/// Largest envelope a page may post (check 3).
pub const MAX_ENVELOPE_BYTES: usize = 64 * 1024;
/// Longest sender URL read before any parse (P-2; the `qa_navigation` bound).
pub const MAX_SOURCE_BYTES: usize = 8192;
/// Digits of the decimal envelope `id`.
pub const MAX_ID_DIGITS: usize = 10;
/// Page messages per second, and the burst.
pub const MESSAGE_RATE: (f64, f64) = (50.0, 50.0);
/// Requests accepted but not yet answered, per page.
pub const MAX_PENDING_REPLIES: usize = 64;
/// Live subscriptions per document.
pub const MAX_LIVE_SUBSCRIPTIONS: usize = 16;
/// One delta delivery per subscription per interval (4 Hz).
pub const DELIVERY_INTERVAL: Duration = Duration::from_millis(250);
/// `set_property` per second, and the burst.
pub const SET_PROPERTY_RATE: (f64, f64) = (2.0, 5.0);
/// `invoke` per second, and the burst.
pub const INVOKE_RATE: (f64, f64) = (5.0, 5.0);
/// `intent` per second, and the burst.
pub const INTENT_RATE: (f64, f64) = (2.0, 4.0);
/// Host ceiling on a declared stream's `max_rate_hz`.
pub const MAX_EMIT_RATE_HZ: u32 = 10;
/// Largest `act` value and `invoke` args, serialized.
pub const MAX_ARGUMENT_BYTES: usize = 16 * 1024;
/// Check-5 refusals audited per second, and the burst; the rest are counted only.
pub const AUDIT_REFUSAL_RATE: (f64, f64) = (2.0, 5.0);
/// Readings carried by one coalesced delivery; older rows are dropped and counted.
pub const MAX_READINGS_PER_DELIVERY: usize = 1_000;
/// Scene changes held for one coalesced delivery; older changes are dropped and counted.
pub const MAX_CHANGES_PER_DELIVERY: usize = 1_024;
/// Bus events held for one coalesced delivery (the broker's guest queue bound).
pub const MAX_EVENTS_PER_DELIVERY: usize = 256;
/// Broker events taken per frame.
pub const MAX_BROKER_EVENTS_PER_FRAME: usize = 256;
/// Envelopes waiting for the next send-time gate.
pub const MAX_OUTBOX: usize = 512;
/// A broker or write reply later than this answers `unknown_outcome` (the broker's own bound is 5 s).
pub const REPLY_DEADLINE: Duration = Duration::from_secs(15);

/// Token bucket: capacity = burst, refilled continuously at `rate` per second.
#[derive(Debug, Clone)]
pub struct TokenBucket {
    rate: f64,
    burst: f64,
    tokens: f64,
    refilled_at: Instant,
}

impl TokenBucket {
    /// A full bucket.
    pub fn new((rate, burst): (f64, f64), now: Instant) -> Self {
        Self {
            rate,
            burst,
            tokens: burst,
            refilled_at: now,
        }
    }

    /// A bucket for a declared stream rate: `rate` capped at [`MAX_EMIT_RATE_HZ`], burst `ceil(rate)`.
    pub fn for_stream(max_rate_hz: u32, now: Instant) -> Self {
        let rate = f64::from(max_rate_hz.min(MAX_EMIT_RATE_HZ).max(1));
        Self::new((rate, rate.ceil()), now)
    }

    /// Take one token if available; time never runs backwards for the refill.
    pub fn try_take(&mut self, now: Instant) -> bool {
        let elapsed = now.saturating_duration_since(self.refilled_at).as_secs_f64();
        self.tokens = (self.tokens + elapsed * self.rate).min(self.burst);
        self.refilled_at = self.refilled_at.max(now);
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

/// The `limits` object `hello` reports (ipc-v1.md §4.3).
pub fn hello_limits() -> serde_json::Value {
    serde_json::json!({
        "envelope_bytes": MAX_ENVELOPE_BYTES,
        "messages_per_sec": MESSAGE_RATE.0,
        "pending_replies": MAX_PENDING_REPLIES,
        "subscriptions": MAX_LIVE_SUBSCRIPTIONS,
        "delivery_hz": 4,
        "set_property_per_sec": SET_PROPERTY_RATE.0,
        "set_property_burst": SET_PROPERTY_RATE.1,
        "invoke_per_sec": INVOKE_RATE.0,
        "invoke_burst": INVOKE_RATE.1,
        "intent_per_sec": INTENT_RATE.0,
        "intent_burst": INTENT_RATE.1,
        "argument_bytes": MAX_ARGUMENT_BYTES,
    })
}
