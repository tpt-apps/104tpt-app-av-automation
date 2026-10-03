//! Inbound trigger rate limiting and adaptive backoff (spec §16).

use std::collections::HashMap;
use std::sync::Mutex;

use crate::time::Timestamp;

/// Backoff policy applied to a source that keeps exceeding its budget.
///
/// Dropping over-budget packets is not enough for a source that ignores the signal, so once a key
/// has been dropped [`BackoffPolicy::threshold`] times in a row it is *muted* for a doubling
/// window: a source that keeps ignoring it is muted for twice as long each time, up to
/// `max_millis`. A source that recovers is never penalised — the first event it gets through
/// accepted clears its strikes and its backoff level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BackoffPolicy {
    /// Consecutive over-budget drops tolerated before the source is muted; `0` disables backoff.
    pub threshold: u32,
    /// How long a source is muted after the first backoff, in milliseconds.
    pub initial_millis: u64,
    /// Longest mute, in milliseconds; further backoffs hold at this value.
    pub max_millis: u64,
}

impl Default for BackoffPolicy {
    fn default() -> Self {
        Self {
            threshold: 3,
            initial_millis: 250,
            max_millis: 5_000,
        }
    }
}

impl BackoffPolicy {
    /// A policy that never mutes a source, used when only rate limiting is wanted.
    pub const fn disabled() -> Self {
        Self {
            threshold: 0,
            initial_millis: 0,
            max_millis: 0,
        }
    }

    /// The mute length for backoff `level` (1-based), doubling up to `max_millis`.
    fn mute_for(&self, level: u32) -> u64 {
        if self.threshold == 0 || level == 0 {
            return 0;
        }
        let shift = (level - 1).min(32);
        self.initial_millis
            .saturating_mul(1u64 << shift)
            .min(self.max_millis)
    }
}

/// Per-source accounting.
#[derive(Debug, Clone, Copy, PartialEq)]
struct SourceState {
    tokens: f64,
    last_refill: Timestamp,
    /// Consecutive over-budget drops, reset by any accepted event.
    strikes: u32,
    /// Current backoff level; `0` means not muted.
    level: u32,
    /// Instant the mute expires, in epoch milliseconds.
    muted_until: u64,
}

impl SourceState {
    fn fresh(capacity: f64, now: Timestamp) -> Self {
        Self {
            tokens: capacity,
            last_refill: now,
            strikes: 0,
            level: 0,
            muted_until: 0,
        }
    }
}

/// Token-bucket rate limiter with adaptive backoff for inbound triggers.
///
/// OSC, Art-Net, sACN and DMX are unauthenticated. A malfunctioning or hostile device must not be
/// able to flood the engine, so every inbound source is metered before its events reach rule
/// matching. Over-budget events are dropped and the drop is counted, never silently ignored.
#[derive(Debug)]
pub struct RateLimiter {
    capacity: f64,
    refill_per_millis: f64,
    policy: BackoffPolicy,
    state: Mutex<HashMap<String, SourceState>>,
    dropped: Mutex<u64>,
    muted_events: Mutex<u64>,
    backoffs: Mutex<u64>,
}

impl RateLimiter {
    /// Creates a limiter allowing `capacity` events, refilling to `capacity` over `window_millis`.
    ///
    /// Backoff is disabled; use [`RateLimiter::with_backoff`] to enable it.
    pub fn new(capacity: u32, window_millis: u64) -> Self {
        Self::with_backoff(capacity, window_millis, BackoffPolicy::disabled())
    }

    /// Creates a limiter with an explicit backoff policy.
    pub fn with_backoff(capacity: u32, window_millis: u64, policy: BackoffPolicy) -> Self {
        let capacity = f64::from(capacity.max(1));
        let window = window_millis.max(1) as f64;
        Self {
            capacity,
            refill_per_millis: capacity / window,
            policy,
            state: Mutex::new(HashMap::new()),
            dropped: Mutex::new(0),
            muted_events: Mutex::new(0),
            backoffs: Mutex::new(0),
        }
    }

    /// The active backoff policy.
    pub fn policy(&self) -> BackoffPolicy {
        self.policy
    }

    /// Attempts to consume one token for `key` at `now`.
    ///
    /// Returns `false` when the caller is over budget or the key is muted, in which case the event
    /// must be dropped and the drop recorded.
    ///
    /// A poisoned lock is recovered rather than propagated. This runs for every inbound datagram on
    /// every protocol listener, so panicking here would take that listener thread down with it, and
    /// the watchdog does not help: the process stays up and `/health` keeps reporting nominal
    /// while one protocol has silently stopped listening. Recovery is sound because the guarded
    /// state is plain accounting, a map of token counts plus `u64` counters, with no partially
    /// updated structure a panic mid-write could leave inconsistent.
    pub fn try_acquire(&self, key: &str, now: Timestamp) -> bool {
        let mut guard = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let (capacity, refill) = (self.capacity, self.refill_per_millis);
        let entry = guard
            .entry(key.to_owned())
            .or_insert_with(|| SourceState::fresh(capacity, now));
        let elapsed = now.millis_since(entry.last_refill);
        entry.last_refill = now;
        entry.tokens = (entry.tokens + elapsed as f64 * refill).min(capacity);

        if entry.level > 0 && now.as_millis() < entry.muted_until {
            drop(guard);
            self.bump_muted();
            return false;
        }
        if entry.tokens >= capacity {
            // The bucket has refilled, so the source has recovered: forgive its past behaviour
            // instead of letting strikes accumulate across the whole session.
            entry.strikes = 0;
            entry.level = 0;
        }

        if entry.tokens >= 1.0 {
            entry.tokens -= 1.0;
            entry.strikes = 0;
            entry.level = 0;
            true
        } else {
            entry.strikes = entry.strikes.saturating_add(1);
            if entry.strikes >= self.policy.threshold {
                entry.strikes = 0;
                entry.level = entry.level.saturating_add(1);
                entry.muted_until = now
                    .as_millis()
                    .saturating_add(self.policy.mute_for(entry.level));
                drop(guard);
                self.bump_backoff();
                self.bump_dropped();
                return false;
            }
            drop(guard);
            self.bump_dropped();
            false
        }
    }

    fn bump_dropped(&self) {
        let mut dropped = self
            .dropped
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *dropped += 1;
    }

    fn bump_muted(&self) {
        let mut muted = self
            .muted_events
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *muted += 1;
    }

    fn bump_backoff(&self) {
        let mut backoffs = self
            .backoffs
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *backoffs += 1;
    }

    /// Tokens currently available for `key`.
    pub fn available(&self, key: &str) -> f64 {
        let guard = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        guard.get(key).map(|s| s.tokens).unwrap_or(self.capacity)
    }

    /// How much longer `key` stays muted at `now`, in milliseconds; `0` when it may be admitted.
    pub fn muted_for_ms(&self, key: &str, now: Timestamp) -> u64 {
        let guard = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        guard
            .get(key)
            .filter(|s| s.level > 0 && now.as_millis() < s.muted_until)
            .map(|s| s.muted_until - now.as_millis())
            .unwrap_or(0)
    }

    /// Total number of events dropped because a source exceeded its budget or was muted.
    pub fn dropped(&self) -> u64 {
        *self
            .dropped
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Events rejected while their source was already muted.
    pub fn muted(&self) -> u64 {
        *self
            .muted_events
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Number of backoff episodes started, i.e. distinct times a source was muted.
    pub fn backoffs(&self) -> u64 {
        *self
            .backoffs
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Drops all accounting, e.g. when a device is reconfigured.
    pub fn reset(&self) {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
        *self
            .dropped
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = 0;
        *self
            .muted_events
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = 0;
        *self
            .backoffs
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = 0;
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

    /// Capacity 1 refilling every millisecond, so a test never confuses "muted" with "out of
    /// tokens": only the mute window can reject an otherwise-admissible event.
    fn backing_off(capacity: u32) -> RateLimiter {
        RateLimiter::with_backoff(
            capacity,
            1,
            BackoffPolicy {
                threshold: 2,
                initial_millis: 100,
                max_millis: 400,
            },
        )
    }

    #[test]
    fn repeated_over_budget_drops_mute_the_source() {
        let limiter = backing_off(1);
        let t0 = Timestamp::from_millis(0);
        assert!(limiter.try_acquire("noisy", t0));
        // First over-budget drop: counted, but not muted yet.
        assert!(!limiter.try_acquire("noisy", t0));
        assert_eq!(limiter.backoffs(), 0);
        // Second consecutive drop trips the threshold and starts a 100 ms mute.
        assert!(!limiter.try_acquire("noisy", t0));
        assert_eq!(limiter.backoffs(), 1);
        assert_eq!(limiter.muted_for_ms("noisy", t0), 100);

        // Inside the mute: rejected even though the token bucket has fully refilled.
        let t1 = Timestamp::from_millis(50);
        assert!(
            limiter.try_acquire("calm", t1),
            "other sources are unaffected"
        );
        assert!(!limiter.try_acquire("noisy", t1));
        assert_eq!(limiter.muted(), 1);

        // Once the mute expires the source is admitted again with a full budget.
        let t2 = Timestamp::from_millis(100);
        assert!(limiter.try_acquire("noisy", t2));
        assert_eq!(limiter.muted_for_ms("noisy", t2), 0);
    }

    #[test]
    fn consecutive_backoffs_double_up_to_the_maximum() {
        // A very slow refill: the source never earns its way back in, so each mute must be longer.
        let limiter = RateLimiter::with_backoff(
            1,
            60_000,
            BackoffPolicy {
                threshold: 2,
                initial_millis: 100,
                max_millis: 400,
            },
        );
        let mut now = Timestamp::from_millis(0);
        // Each mute doubles until it saturates at 400 ms.
        for (round, mute) in [100u64, 200, 400, 400].iter().enumerate() {
            for _ in 0..3 {
                let _ = limiter.try_acquire("noisy", now);
            }
            assert_eq!(limiter.backoffs(), round as u64 + 1, "round {round}");
            assert_eq!(limiter.muted_for_ms("noisy", now), *mute, "round {round}");
            now = now.saturating_add_millis(*mute);
        }
    }

    #[test]
    fn a_source_that_behaves_is_never_muted() {
        let limiter = backing_off(2);
        let mut now = Timestamp::from_millis(0);
        // A source pacing itself just under the budget never trips the threshold.
        for _ in 0..100 {
            assert!(limiter.try_acquire("calm", now));
            assert!(limiter.try_acquire("calm", now));
            now = now.saturating_add_millis(1_000);
        }
        assert_eq!(limiter.backoffs(), 0);
        assert_eq!(limiter.dropped(), 0);
    }

    #[test]
    fn an_accepted_event_clears_the_strike_count() {
        let limiter = backing_off(1);
        let t0 = Timestamp::from_millis(0);
        assert!(limiter.try_acquire("noisy", t0));
        assert!(!limiter.try_acquire("noisy", t0), "one strike");
        // A quiet period refills the bucket; the accepted event clears the strike.
        let t1 = Timestamp::from_millis(1_000);
        assert!(limiter.try_acquire("noisy", t1));
        assert!(
            !limiter.try_acquire("noisy", t1),
            "strikes restarted at one"
        );
        assert!(
            !limiter.try_acquire("noisy", t1),
            "second strike trips backoff"
        );
        assert_eq!(limiter.backoffs(), 1);
    }

    #[test]
    fn backoff_is_tracked_per_source() {
        let limiter = backing_off(1);
        let t0 = Timestamp::from_millis(0);
        for _ in 0..9 {
            let _ = limiter.try_acquire("noisy", t0);
        }
        assert_eq!(limiter.backoffs(), 1, "still inside the first mute");
        assert_eq!(limiter.muted(), 6);
        assert!(
            limiter.try_acquire("quiet", t0),
            "other sources are unaffected"
        );
        assert_eq!(limiter.muted_for_ms("quiet", t0), 0);
    }

    #[test]
    fn reset_clears_backoff_state_too() {
        let limiter = backing_off(1);
        let t0 = Timestamp::from_millis(0);
        assert!(limiter.try_acquire("noisy", t0));
        let _ = limiter.try_acquire("noisy", t0);
        let _ = limiter.try_acquire("noisy", t0);
        assert_eq!(limiter.backoffs(), 1);
        limiter.reset();
        assert_eq!(limiter.backoffs(), 0);
        assert_eq!(limiter.muted(), 0);
        assert_eq!(limiter.muted_for_ms("noisy", t0), 0);
        assert!(limiter.try_acquire("noisy", t0));
    }

    #[test]
    fn extreme_timestamps_do_not_overflow_the_mute() {
        let limiter = RateLimiter::with_backoff(
            1,
            1,
            BackoffPolicy {
                threshold: 1,
                initial_millis: u64::MAX,
                max_millis: u64::MAX,
            },
        );
        let now = Timestamp::from_millis(u64::MAX - 1);
        assert!(limiter.try_acquire("x", now));
        assert!(!limiter.try_acquire("x", now));
        // The mute saturates instead of wrapping around into the past.
        assert_eq!(limiter.muted_for_ms("x", now), 1);
    }

    /// Poisons `limiter`'s shared state by panicking inside the critical section.
    ///
    /// `try_acquire` holds the state lock across its accounting update, so poisoning it the way a
    /// real panic would requires reaching into the private field; this helper does exactly that and
    /// is the only place the test module touches internals.
    fn poison(limiter: &RateLimiter) {
        let _guard = limiter.state.lock().expect("freshly locked");
        panic!("simulated panic inside the rate limiter critical section");
    }

    #[test]
    fn a_poisoned_lock_does_not_take_the_listener_down() {
        let limiter = RateLimiter::with_backoff(
            2,
            1_000,
            BackoffPolicy {
                threshold: 2,
                initial_millis: 100,
                max_millis: 400,
            },
        );
        let t0 = Timestamp::from_millis(0);

        // A panic anywhere in the process can leave the shared state poisoned.
        let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| poison(&limiter)));
        assert!(caught.is_err(), "the helper really did panic");

        // Every accessor must keep working: a panic here would kill the listener thread, and the
        // process would keep serving /health as if nothing had happened.
        assert!(limiter.try_acquire("a", t0), "still admits");
        assert!(limiter.try_acquire("a", t0), "still admits");
        assert!(!limiter.try_acquire("a", t0), "and still limits");
        assert_eq!(limiter.dropped(), 1);
        assert_eq!(limiter.muted(), 0);
        assert_eq!(limiter.backoffs(), 0);
        assert_eq!(limiter.muted_for_ms("a", t0), 0);
        assert!(limiter.available("a") < 2.0);

        // Reset must work too, or a reconfigured device could not clear its accounting.
        limiter.reset();
        assert_eq!(limiter.dropped(), 0);
        assert!(limiter.try_acquire("a", t0));
    }

    #[test]
    fn a_poisoned_counter_lock_does_not_stop_accounting() {
        let limiter = RateLimiter::new(1, 1_000);
        let t0 = Timestamp::from_millis(0);
        assert!(limiter.try_acquire("a", t0));
        assert!(!limiter.try_acquire("a", t0));
        assert_eq!(limiter.dropped(), 1);

        // The counters are behind their own locks; poison one and read them all.
        let poisoned = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = limiter.dropped.lock().expect("freshly locked");
            panic!("simulated panic while holding a counter lock");
        }));
        assert!(poisoned.is_err(), "the helper really did panic");
        let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = limiter.dropped();
            let _ = limiter.muted();
            let _ = limiter.backoffs();
        }));
        assert!(caught.is_ok(), "counters keep reporting after poisoning");
        assert_eq!(limiter.dropped(), 1);
    }

    #[test]
    fn poisoning_does_not_leak_between_sources() {
        let limiter = RateLimiter::new(1, 1_000);
        let t0 = Timestamp::from_millis(0);
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| poison(&limiter)));

        // Per-source accounting is independent, so one poisoned limiter still separates sources.
        assert!(limiter.try_acquire("calm", t0));
        assert!(!limiter.try_acquire("calm", t0));
        assert!(limiter.try_acquire("other", t0));
    }
}
