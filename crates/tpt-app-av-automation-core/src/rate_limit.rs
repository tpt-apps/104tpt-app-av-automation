//! Inbound trigger rate limiting (spec §16).

use std::collections::HashMap;
use std::sync::Mutex;

use crate::time::Timestamp;

/// Simple token-bucket rate limiter for inbound triggers.
///
/// OSC, Art-Net, sACN and DMX are unauthenticated. A malfunctioning or hostile device must not be
/// able to flood the engine, so every inbound source is metered before its events reach rule
/// matching. Over-budget events are dropped and the drop is counted, never silently ignored.
#[derive(Debug)]
pub struct RateLimiter {
    capacity: f64,
    refill_per_millis: f64,
    state: Mutex<HashMap<String, (f64, Timestamp)>>,
    dropped: Mutex<u64>,
}

impl RateLimiter {
    /// Creates a limiter allowing `capacity` events, refilling to `capacity` over `window_millis`.
    pub fn new(capacity: u32, window_millis: u64) -> Self {
        let capacity = f64::from(capacity.max(1));
        let window = window_millis.max(1) as f64;
        Self {
            capacity,
            refill_per_millis: capacity / window,
            state: Mutex::new(HashMap::new()),
            dropped: Mutex::new(0),
        }
    }

    /// Attempts to consume one token for `key` at `now`.
    ///
    /// Returns `false` when the caller is over budget, in which case the event must be dropped and
    /// the drop recorded.
    pub fn try_acquire(&self, key: &str, now: Timestamp) -> bool {
        let mut guard = self.state.lock().expect("rate limiter mutex poisoned");
        let entry = guard.entry(key.to_owned()).or_insert((self.capacity, now));
        let elapsed = now.millis_since(entry.1);
        entry.1 = now;
        entry.0 = (entry.0 + elapsed as f64 * self.refill_per_millis).min(self.capacity);
        if entry.0 >= 1.0 {
            entry.0 -= 1.0;
            true
        } else {
            drop(guard);
            let mut dropped = self.dropped.lock().expect("rate limiter mutex poisoned");
            *dropped += 1;
            false
        }
    }

    /// Tokens currently available for `key`.
    pub fn available(&self, key: &str) -> f64 {
        let guard = self.state.lock().expect("rate limiter mutex poisoned");
        guard.get(key).map(|(v, _)| *v).unwrap_or(self.capacity)
    }

    /// Total number of events dropped because a source exceeded its budget.
    pub fn dropped(&self) -> u64 {
        *self.dropped.lock().expect("rate limiter mutex poisoned")
    }

    /// Drops all accounting, e.g. when a device is reconfigured.
    pub fn reset(&self) {
        self.state
            .lock()
            .expect("rate limiter mutex poisoned")
            .clear();
        *self.dropped.lock().expect("rate limiter mutex poisoned") = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_limiter_throttles_a_flooding_source() {
        let limiter = RateLimiter::new(3, 1_000);
        let t0 = Timestamp::from_millis(0);
        assert!(limiter.try_acquire("osc/9000", t0));
        assert!(limiter.try_acquire("osc/9000", t0));
        assert!(limiter.try_acquire("osc/9000", t0));
        assert!(!limiter.try_acquire("osc/9000", t0), "burst must be capped");
        assert_eq!(limiter.dropped(), 1);

        // Independent budgets per key.
        assert!(limiter.try_acquire("osc/9001", t0));

        // Budget refills at 3 tokens/second, so a full second restores all three.
        let t1 = Timestamp::from_millis(1_000);
        assert!(limiter.try_acquire("osc/9000", t1));
        assert!(limiter.try_acquire("osc/9000", t1));
        assert!(limiter.try_acquire("osc/9000", t1));
        assert!(!limiter.try_acquire("osc/9000", t1));

        // 400 ms at 3 tokens/second restores 1.2 tokens, so exactly one more event passes.
        let t2 = Timestamp::from_millis(1_400);
        assert!(limiter.try_acquire("osc/9000", t2));
        assert!(!limiter.try_acquire("osc/9000", t2));
    }

    #[test]
    fn reset_clears_drops() {
        let limiter = RateLimiter::new(1, 1_000);
        let t0 = Timestamp::from_millis(0);
        assert!(limiter.try_acquire("a", t0));
        assert!(!limiter.try_acquire("a", t0));
        limiter.reset();
        assert_eq!(limiter.dropped(), 0);
        assert!(limiter.try_acquire("a", t0));
    }

    #[test]
    fn zero_capacity_is_clamped_to_one() {
        let limiter = RateLimiter::new(0, 0);
        assert!(limiter.try_acquire("a", Timestamp::from_millis(0)));
    }
}