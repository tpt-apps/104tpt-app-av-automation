//! Injectable time source.

use std::fmt;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// Milliseconds since the Unix epoch.
///
/// The deterministic rule core never calls [`SystemTime::now`] directly: time is supplied through a
/// [`Clock`]. This keeps rule evaluation reproducible in simulation, golden tests and chaos tests
/// (spec §3.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Timestamp {
    millis: u64,
}

impl Timestamp {
    /// Builds a timestamp from milliseconds since the Unix epoch.
    pub const fn from_millis(millis: u64) -> Self {
        Self { millis }
    }

    /// Milliseconds since the Unix epoch.
    pub const fn as_millis(self) -> u64 {
        self.millis
    }

    /// Adds a duration expressed in milliseconds, saturating at `u64::MAX`.
    pub fn saturating_add_millis(self, millis: u64) -> Self {
        Self {
            millis: self.millis.saturating_add(millis),
        }
    }

    /// Milliseconds elapsed from `earlier` to `self`, or `0` if `earlier` is in the future.
    pub fn millis_since(self, earlier: Timestamp) -> u64 {
        self.millis.saturating_sub(earlier.millis)
    }

    /// Reads the current wall-clock time.
    ///
    /// Only shells (CLI/service) should call this; the deterministic core receives time from a
    /// [`Clock`] instead.
    pub fn now() -> Self {
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
            .unwrap_or(0);
        Self { millis }
    }
}

impl fmt::Display for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.millis)
    }
}

/// Injectable time source.
pub trait Clock: Send + Sync {
    /// Current time.
    fn now(&self) -> Timestamp;
}

/// A [`Clock`] whose value only changes when the caller advances it.
#[derive(Debug, Clone)]
pub struct FixedClock {
    now: Arc<Mutex<Timestamp>>,
}

impl FixedClock {
    /// Creates a clock pinned at `start`.
    pub fn new(start: Timestamp) -> Self {
        Self {
            now: Arc::new(Mutex::new(start)),
        }
    }

    /// Advances the clock by `millis`.
    pub fn advance_millis(&self, millis: u64) {
        let mut guard = self.now.lock().expect("fixed clock mutex poisoned");
        *guard = guard.saturating_add_millis(millis);
    }

    /// Sets the clock to an absolute instant.
    pub fn set(&self, at: Timestamp) {
        let mut guard = self.now.lock().expect("fixed clock mutex poisoned");
        *guard = at;
    }
}

impl Default for FixedClock {
    fn default() -> Self {
        Self::new(Timestamp::from_millis(0))
    }
}

impl Clock for FixedClock {
    fn now(&self) -> Timestamp {
        *self.now.lock().expect("fixed clock mutex poisoned")
    }
}

/// A [`Clock`] backed by the operating system wall clock, used only by shells.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Timestamp {
        Timestamp::now()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_clock_advances_deterministically() {
        let clock = FixedClock::new(Timestamp::from_millis(1_000));
        assert_eq!(clock.now().as_millis(), 1_000);
        clock.advance_millis(500);
        assert_eq!(clock.now().as_millis(), 1_500);
        clock.advance_millis(u64::MAX);
        assert_eq!(clock.now().as_millis(), u64::MAX);
    }

    #[test]
    fn timestamp_arithmetic_saturates() {
        let t = Timestamp::from_millis(10);
        assert_eq!(t.millis_since(Timestamp::from_millis(4)), 6);
        assert_eq!(t.millis_since(Timestamp::from_millis(20)), 0);
        assert_eq!(
            Timestamp::from_millis(u64::MAX)
                .saturating_add_millis(10)
                .as_millis(),
            u64::MAX
        );
    }

    #[test]
    fn setting_an_absolute_instant_is_supported() {
        let clock = FixedClock::default();
        clock.set(Timestamp::from_millis(42));
        assert_eq!(clock.now().as_millis(), 42);
    }
}