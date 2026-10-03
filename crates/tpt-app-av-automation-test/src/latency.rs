//! Latency statistics for the performance and profiling suites (spec §18, §24.1 step 20).
//!
//! A mean is useless for latency: what an operator feels is the slow tail, and what a show needs
//! is the worst case in the worst second of the night. [`LatencySamples`] therefore reports
//! percentiles and a maximum, and the tests gate on those rather than on an average.

use std::time::Duration;

/// Ordered percentile values, in microseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Percentiles {
    /// Smallest sample observed.
    pub min: u64,
    /// Median sample.
    pub p50: u64,
    /// Sample below which 90% of observations fell.
    pub p90: u64,
    /// Sample below which 99% of observations fell.
    pub p99: u64,
    /// Slowest sample observed — the number that matters for a missed cue.
    pub max: u64,
}

impl std::fmt::Display for Percentiles {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "min {:>7} us | p50 {:>7} us | p90 {:>7} us | p99 {:>7} us | max {:>7} us",
            self.min, self.p50, self.p90, self.p99, self.max
        )
    }
}

/// A growing set of duration samples.
#[derive(Debug, Clone, Default)]
pub struct LatencySamples {
    micros: Vec<u64>,
}

impl LatencySamples {
    /// An empty set of samples.
    pub fn new() -> Self {
        Self::default()
    }

    /// Records one observation.
    pub fn record(&mut self, elapsed: Duration) {
        // Saturating: an absurd duration is a broken clock, not a slow sample, and must not wrap
        // into a *small* number that would flatter the result.
        self.micros
            .push(u64::try_from(elapsed.as_micros()).unwrap_or(u64::MAX));
    }

    /// Number of observations.
    pub fn len(&self) -> usize {
        self.micros.len()
    }

    /// Whether anything has been observed.
    pub fn is_empty(&self) -> bool {
        self.micros.is_empty()
    }

    /// Drops every observation.
    pub fn clear(&mut self) {
        self.micros.clear();
    }

    /// The percentile set, or `None` when nothing was observed.
    ///
    /// Percentiles use the nearest-rank method on the sorted samples, so every reported value is a
    /// measurement that actually happened rather than an interpolation between two of them.
    pub fn percentiles(&self) -> Option<Percentiles> {
        if self.micros.is_empty() {
            return None;
        }
        let mut sorted = self.micros.clone();
        sorted.sort_unstable();
        Some(Percentiles {
            min: sorted[0],
            p50: percentile(&sorted, 50.0),
            p90: percentile(&sorted, 90.0),
            p99: percentile(&sorted, 99.0),
            max: sorted[sorted.len() - 1],
        })
    }

    /// The mean observation, or `None` when nothing was observed.
    ///
    /// Reported only because the throughput figures in `docs/reliability.md` are quoted as an
    /// average; never use it as a latency budget.
    pub fn mean(&self) -> Option<Duration> {
        if self.micros.is_empty() {
            return None;
        }
        // u128 so the sum of many large samples cannot overflow.
        let total: u128 = self.micros.iter().map(|v| u128::from(*v)).sum();
        let mean = total / self.micros.len() as u128;
        Some(Duration::from_micros(
            u64::try_from(mean).unwrap_or(u64::MAX),
        ))
    }
}

/// Nearest-rank percentile of an already sorted, non-empty slice.
fn percentile(sorted: &[u64], pct: f64) -> u64 {
    let n = sorted.len();
    // rank = ceil(pct/100 * n), clamped to 1..=n; the index is rank - 1.
    let rank = ((pct / 100.0) * n as f64).ceil() as usize;
    sorted[rank.clamp(1, n) - 1]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn samples_of(micros: &[u64]) -> LatencySamples {
        let mut s = LatencySamples::new();
        for value in micros {
            s.record(Duration::from_micros(*value));
        }
        s
    }

    #[test]
    fn an_empty_set_has_no_statistics() {
        let s = LatencySamples::new();
        assert!(s.is_empty());
        assert_eq!(s.len(), 0);
        assert_eq!(s.percentiles(), None);
        assert_eq!(s.mean(), None);
    }

    #[test]
    fn a_single_sample_is_every_percentile() {
        let p = samples_of(&[42]).percentiles().unwrap();
        assert_eq!(
            p,
            Percentiles {
                min: 42,
                p50: 42,
                p90: 42,
                p99: 42,
                max: 42
            }
        );
    }

    #[test]
    fn percentiles_are_ordered_and_report_real_samples() {
        // 1..=100 us, so each percentile should land exactly on its rank.
        let values: Vec<u64> = (1..=100).collect();
        let p = samples_of(&values).percentiles().unwrap();
        assert_eq!((p.min, p.p50, p.p90, p.p99, p.max), (1, 50, 90, 99, 100));
    }

    #[test]
    fn percentiles_are_insensitive_to_input_order() {
        let p = samples_of(&[9, 1, 7, 3, 5]).percentiles().unwrap();
        assert_eq!((p.min, p.p50, p.max), (1, 5, 9));
    }

    #[test]
    fn an_outlier_dominates_the_tail_but_not_the_median() {
        let mut values = vec![10u64; 99];
        values.push(5_000_000);
        let p = samples_of(&values).percentiles().unwrap();
        assert_eq!(p.p50, 10);
        assert_eq!(p.p90, 10);
        assert_eq!(p.p99, 10);
        // Nearest-rank p99 over 100 samples is the 99th, still an ordinary sample: one bad
        // observation in a hundred is *not* visible at p99 by construction. Only the maximum
        // catches it, which is why the maximum is reported alongside p99.
        assert_eq!(
            p.max, 5_000_000,
            "the outlier is only visible at the maximum"
        );
    }

    #[test]
    fn a_one_percent_tail_is_visible_at_p99() {
        // The same point the other way round: when 2% of samples are slow, p99 (the 990th of 1000)
        // lands in the slow region. That is the case that matters — a persistent tail, not a single
        // glitch. A 1%-exactly tail sits just below p99 by construction, which is why the maximum is
        // reported too.
        let mut values = vec![10u64; 980];
        values.extend(vec![9_000u64; 20]);
        let p = samples_of(&values).percentiles().unwrap();
        assert_eq!(p.p50, 10);
        assert_eq!(p.p99, 9_000, "a 2% slow tail puts p99 in the slow region");
    }

    #[test]
    fn the_mean_is_the_arithmetic_average() {
        assert_eq!(
            samples_of(&[10, 20, 30]).mean(),
            Some(Duration::from_micros(20))
        );
    }

    #[test]
    fn many_large_samples_do_not_overflow_the_mean() {
        let mut s = LatencySamples::new();
        for _ in 0..1_000 {
            s.record(Duration::from_micros(u64::MAX / 2));
        }
        assert_eq!(s.mean(), Some(Duration::from_micros(u64::MAX / 2)));
    }

    #[test]
    fn clearing_empties_the_set() {
        let mut s = samples_of(&[1, 2, 3]);
        s.clear();
        assert!(s.is_empty());
        assert_eq!(s.percentiles(), None);
    }

    #[test]
    fn the_report_is_readable() {
        let text = samples_of(&[1, 2, 3]).percentiles().unwrap().to_string();
        assert!(text.contains("p50"), "{text}");
        assert!(text.contains("max"), "{text}");
    }
}
