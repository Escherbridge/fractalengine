//! Token bucket on the injected clock: burst `ceil(rate_hz)`, refilled continuously (bus.md §3).

/// A rate limiter that never sleeps; callers ask and are told yes or no.
#[derive(Debug, Clone)]
pub struct TokenBucket {
    rate_per_ms: f64,
    capacity: f64,
    tokens: f64,
    last_ms: u64,
}

impl TokenBucket {
    /// A full bucket refilling at `rate_hz`; a non-positive or non-finite rate never refills.
    pub fn new(rate_hz: f64, now_ms: u64) -> Self {
        let rate = if rate_hz.is_finite() && rate_hz > 0.0 { rate_hz } else { 0.0 };
        let capacity = rate.ceil().max(1.0);
        Self { rate_per_ms: rate / 1_000.0, capacity, tokens: capacity, last_ms: now_ms }
    }

    /// A full bucket of `burst` tokens refilling at `rate_hz` (the portal write budget: 2/s, burst 5).
    pub fn with_burst(rate_hz: f64, burst: u32, now_ms: u64) -> Self {
        let mut bucket = Self::new(rate_hz, now_ms);
        bucket.capacity = f64::from(burst.max(1));
        bucket.tokens = bucket.capacity;
        bucket
    }

    /// Take one token if one is available at `now_ms`.
    pub fn try_take(&mut self, now_ms: u64) -> bool {
        self.refill(now_ms);
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }

    /// Full again and untouched for `idle_ms`: forgetting it changes nothing.
    pub fn is_idle(&mut self, now_ms: u64, idle_ms: u64) -> bool {
        let untouched = now_ms.saturating_sub(self.last_ms) >= idle_ms;
        self.refill(now_ms);
        untouched && self.tokens >= self.capacity
    }

    fn refill(&mut self, now_ms: u64) {
        if now_ms > self.last_ms {
            let elapsed = (now_ms - self.last_ms) as f64;
            self.tokens = (self.tokens + elapsed * self.rate_per_ms).min(self.capacity);
            self.last_ms = now_ms;
        }
    }
}
