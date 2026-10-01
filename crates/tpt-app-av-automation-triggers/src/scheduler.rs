//! Schedule trigger driver (spec §7.1, §11).

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use tpt_app_av_automation_core::{Event, Timestamp};
use tpt_app_av_automation_model::{Rule, ScheduleSpec, TriggerSpec};

const MILLIS_PER_MINUTE: i64 = 60_000;
const MINUTES_PER_DAY: i64 = 1_440;

/// Converts UTC timestamps to site-local wall-clock time using a fixed UTC offset.
///
/// The standard library has no timezone database, and the deterministic core must not guess, so
/// the shell supplies the offset explicitly.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LocalClock {
    /// Minutes east of UTC, e.g. `780` for UTC+13.
    pub utc_offset_minutes: i32,
}

/// A local wall-clock instant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalMoment {
    /// Minutes since local midnight, 0-1439.
    pub minutes_since_midnight: u32,
    /// Monday-first weekday index, 0-6.
    pub weekday_index: usize,
    /// Minutes since the local epoch; unique per wall-clock minute.
    pub absolute_minute: i64,
}

impl LocalClock {
    /// A clock at the given offset.
    pub fn new(utc_offset_minutes: i32) -> Self {
        Self { utc_offset_minutes }
    }

    /// Local time for a timestamp.
    pub fn local(&self, ts: Timestamp) -> LocalMoment {
        let millis = i64::try_from(ts.as_millis()).unwrap_or(i64::MAX / 2);
        let local_millis = millis.saturating_add(i64::from(self.utc_offset_minutes) * MILLIS_PER_MINUTE);
        let absolute_minute = local_millis.div_euclid(MILLIS_PER_MINUTE);
        let day = absolute_minute.div_euclid(MINUTES_PER_DAY);
        LocalMoment {
            minutes_since_midnight: absolute_minute.rem_euclid(MINUTES_PER_DAY) as u32,
            // 1970-01-01 was a Thursday, index 3 when Monday is 0.
            weekday_index: (day + 3).rem_euclid(7) as usize,
            absolute_minute,
        }
    }
}

/// Persistable scheduler memory.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchedulerState {
    /// Labels of one-shot schedules that already fired; they never fire again.
    pub fired_once: BTreeSet<String>,
    /// Last wall-clock minute each fixed-time schedule fired in.
    pub last_fired_minute: BTreeMap<String, i64>,
    /// Last firing instant of each interval schedule, in epoch milliseconds.
    pub last_interval_fire: BTreeMap<String, u64>,
}

/// Drives schedule triggers from a polled clock.
#[derive(Debug, Clone, Default)]
pub struct Scheduler {
    entries: BTreeMap<String, ScheduleSpec>,
    state: SchedulerState,
}

impl Scheduler {
    /// An empty scheduler.
    pub fn new() -> Self {
        Self::default()
    }

    /// Builds a scheduler from the schedule triggers of the given rules.
    pub fn from_rules<'a>(rules: impl IntoIterator<Item = &'a Rule>) -> Self {
        let mut s = Self::new();
        for rule in rules {
            if let TriggerSpec::Schedule(spec) = &rule.trigger.spec {
                s.add(spec.clone());
            }
        }
        s
    }

    /// Adds a schedule. Identical schedules share one entry and fire one event.
    pub fn add(&mut self, spec: ScheduleSpec) {
        self.entries.insert(spec.label(), spec);
    }

    /// Number of distinct schedules.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether there are no schedules.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Current memory, for persistence.
    pub fn state(&self) -> &SchedulerState {
        &self.state
    }

    /// Restores memory after a restart.
    pub fn restore(&mut self, state: SchedulerState) {
        self.state = state;
    }

    /// Returns the schedule events due at `now`.
    ///
    /// Safe to call at any rate: a fixed-time schedule fires once per matching wall-clock minute no
    /// matter how often it is polled, a one-shot fires once ever, and an interval schedule fires at
    /// most once per poll (no catch-up burst after a stall). Missed firings while the engine was
    /// down are not replayed.
    pub fn poll(&mut self, now: Timestamp, clock: LocalClock) -> Vec<Event> {
        let local = clock.local(now);
        let mut events = Vec::new();
        for (label, spec) in &self.entries {
            let due = if let Some(interval) = spec.interval_ms {
                let interval = interval.max(1);
                match self.state.last_interval_fire.get(label).copied() {
                    None => {
                        // First sighting: start the period now rather than firing immediately.
                        self.state
                            .last_interval_fire
                            .insert(label.clone(), now.as_millis());
                        false
                    }
                    Some(last) if now.millis_since(Timestamp::from_millis(last)) >= interval => {
                        self.state
                            .last_interval_fire
                            .insert(label.clone(), now.as_millis());
                        true
                    }
                    Some(_) => false,
                }
            } else if spec.due_at(local.minutes_since_midnight, local.weekday_index) {
                let already_this_minute =
                    self.state.last_fired_minute.get(label) == Some(&local.absolute_minute);
                let spent_one_shot = spec.once && self.state.fired_once.contains(label);
                if already_this_minute || spent_one_shot {
                    false
                } else {
                    self.state
                        .last_fired_minute
                        .insert(label.clone(), local.absolute_minute);
                    if spec.once {
                        self.state.fired_once.insert(label.clone());
                    }
                    true
                }
            } else {
                false
            };
            if due {
                events.push(Event::Schedule {
                    spec: label.clone(),
                });
            }
        }
        events
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tpt_app_av_automation_model::{LocalTime, Weekday};

    /// 2024-01-01 was a Monday.
    const MONDAY_MIDNIGHT_UTC: u64 = 1_704_067_200_000;

    fn ts(day: u64, hour: u64, minute: u64, second: u64) -> Timestamp {
        Timestamp::from_millis(
            MONDAY_MIDNIGHT_UTC + day * 86_400_000 + hour * 3_600_000 + minute * 60_000 + second * 1_000,
        )
    }

    fn daily(h: u8, m: u8) -> ScheduleSpec {
        ScheduleSpec::daily(LocalTime::new(h, m).unwrap())
    }

    #[test]
    fn local_clock_handles_offset_and_weekday() {
        let utc = LocalClock::new(0).local(ts(0, 18, 55, 0));
        assert_eq!(utc.minutes_since_midnight, 18 * 60 + 55);
        assert_eq!(utc.weekday_index, 0, "monday");
        // UTC+13 pushes 18:55 UTC Monday into Tuesday 07:55.
        let nz = LocalClock::new(13 * 60).local(ts(0, 18, 55, 0));
        assert_eq!(nz.minutes_since_midnight, 7 * 60 + 55);
        assert_eq!(nz.weekday_index, 1);
        // Negative offsets cross back over midnight.
        let west = LocalClock::new(-5 * 60).local(ts(0, 1, 0, 0));
        assert_eq!(west.minutes_since_midnight, 20 * 60);
        assert_eq!(west.weekday_index, 6, "sunday");
    }

    #[test]
    fn fixed_schedule_fires_once_per_minute_however_often_polled() {
        let mut s = Scheduler::new();
        s.add(daily(18, 55));
        let clock = LocalClock::default();
        assert!(s.poll(ts(0, 18, 54, 59), clock).is_empty());
        assert_eq!(s.poll(ts(0, 18, 55, 0), clock).len(), 1);
        assert!(s.poll(ts(0, 18, 55, 30), clock).is_empty());
        assert!(s.poll(ts(0, 18, 55, 59), clock).is_empty());
        assert!(s.poll(ts(0, 18, 56, 0), clock).is_empty());
        assert_eq!(s.poll(ts(1, 18, 55, 0), clock).len(), 1, "fires again next day");
    }

    #[test]
    fn weekday_filter_is_applied() {
        let mut s = Scheduler::new();
        s.add(ScheduleSpec {
            days: vec![Weekday::Tue],
            ..daily(9, 0)
        });
        let clock = LocalClock::default();
        assert!(s.poll(ts(0, 9, 0, 0), clock).is_empty(), "monday");
        assert_eq!(s.poll(ts(1, 9, 0, 0), clock).len(), 1, "tuesday");
    }

    #[test]
    fn one_shot_never_refires_even_after_restart() {
        let spec = ScheduleSpec::one_shot(LocalTime::new(18, 55).unwrap(), vec![]);
        let clock = LocalClock::default();

        let mut first = Scheduler::new();
        first.add(spec.clone());
        assert_eq!(first.poll(ts(0, 18, 55, 0), clock).len(), 1);
        let saved = serde_json::to_string(first.state()).unwrap();

        // "Restart": a fresh scheduler restored from the persisted state.
        let mut second = Scheduler::new();
        second.add(spec);
        second.restore(serde_json::from_str(&saved).unwrap());
        assert!(second.poll(ts(0, 18, 55, 10), clock).is_empty(), "same minute");
        assert!(second.poll(ts(1, 18, 55, 0), clock).is_empty(), "next day");
    }

    #[test]
    fn interval_schedule_starts_a_period_then_fires_without_catch_up_burst() {
        let mut s = Scheduler::new();
        s.add(ScheduleSpec::interval(30_000));
        let clock = LocalClock::default();
        assert!(s.poll(ts(0, 0, 0, 0), clock).is_empty(), "baseline");
        assert!(s.poll(ts(0, 0, 0, 29), clock).is_empty());
        assert_eq!(s.poll(ts(0, 0, 0, 30), clock).len(), 1);
        // A long stall yields a single event, not one per missed period.
        assert_eq!(s.poll(ts(0, 1, 0, 0), clock).len(), 1);
        assert!(s.poll(ts(0, 1, 0, 1), clock).is_empty());
    }

    #[test]
    fn identical_schedules_fire_one_event() {
        let mut s = Scheduler::new();
        s.add(daily(1, 0));
        s.add(daily(1, 0));
        assert_eq!(s.len(), 1);
        assert_eq!(s.poll(ts(0, 1, 0, 0), LocalClock::default()).len(), 1);
    }

    #[test]
    fn extreme_timestamps_do_not_panic() {
        let mut s = Scheduler::new();
        s.add(daily(0, 0));
        s.add(ScheduleSpec::interval(0));
        let _ = s.poll(Timestamp::from_millis(u64::MAX), LocalClock::new(i32::MAX));
        let _ = s.poll(Timestamp::from_millis(0), LocalClock::new(i32::MIN));
    }
}
