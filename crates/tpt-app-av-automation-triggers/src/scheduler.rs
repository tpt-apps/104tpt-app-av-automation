//! Schedule trigger driver (spec §7.1, §11).

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use tpt_app_av_automation_core::{Event, Timestamp};
use tpt_app_av_automation_model::{Rule, ScheduleMoment, ScheduleSpec, TriggerSpec};

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
    /// Day of the local month, 1-31. Needed by cron day-of-month fields.
    pub day_of_month: u32,
    /// Month of the local year, 1-12. Needed by cron month fields.
    pub month: u32,
}

/// The proleptic Gregorian calendar date for a count of days since 1970-01-01.
///
/// Howard Hinnant's `civil_from_days` algorithm: exact over the whole range, with no lookup tables
/// and no timezone database, so the cron day-of-month and month fields work offline.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // 0-146096
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365; // 0-399
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // 0-365
    let mp = (5 * doy + 2) / 153; // 0-11
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // 1-31
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // 1-12
    (if m <= 2 { y + 1 } else { y }, m, d)
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
        // 1970-01-01 was a Thursday, index 3 when Monday is 0.
        let weekday_index = (day + 3).rem_euclid(7) as usize;
        // Clamp before converting: a saturated timestamp must still produce a usable date.
        let (_, month, day_of_month) = civil_from_days(day.clamp(-1_000_000, 1_000_000));
        LocalMoment {
            minutes_since_midnight: absolute_minute.rem_euclid(MINUTES_PER_DAY) as u32,
            weekday_index,
            absolute_minute,
            day_of_month,
            month,
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
        let moment = ScheduleMoment {
            minute: local.minutes_since_midnight,
            weekday_index: local.weekday_index,
            day_of_month: Some(local.day_of_month),
            month: Some(local.month),
        };
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
            } else if spec.due_minute(&moment) {
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

    #[test]
    fn civil_dates_are_derived_from_the_local_day() {
        let clock = LocalClock::default();
        // 2024-01-01 was a Monday.
        assert_eq!((clock.local(ts(0, 12, 0, 0)).day_of_month, clock.local(ts(0, 12, 0, 0)).month), (1, 1));
        // Day 59 of 2024 is 29 February, a leap year.
        assert_eq!(
            (
                clock.local(ts(59, 12, 0, 0)).day_of_month,
                clock.local(ts(59, 12, 0, 0)).month
            ),
            (29, 2)
        );
        // Day 365 is 31 December.
        assert_eq!(
            (
                clock.local(ts(365, 12, 0, 0)).day_of_month,
                clock.local(ts(365, 12, 0, 0)).month
            ),
            (31, 12)
        );
        // A year later the same offset lands on 2025-01-01.
        assert_eq!(
            (
                clock.local(ts(366, 12, 0, 0)).day_of_month,
                clock.local(ts(366, 12, 0, 0)).month
            ),
            (1, 1)
        );
    }

    #[test]
    fn a_cron_schedule_fires_on_every_matching_minute() {
        let mut s = Scheduler::new();
        s.add(ScheduleSpec::cron("0,30 * * * *"));
        let clock = LocalClock::default();
        // Monday 09:00 and 09:30 fire; 09:01 does not.
        assert_eq!(s.poll(ts(0, 9, 0, 0), clock).len(), 1);
        assert!(s.poll(ts(0, 9, 0, 30), clock).is_empty(), "same minute, idempotent");
        assert!(s.poll(ts(0, 9, 1, 0), clock).is_empty());
        assert_eq!(s.poll(ts(0, 9, 30, 0), clock).len(), 1);
        assert_eq!(s.poll(ts(0, 9, 0, 0), clock).len(), 1, "next day");
        assert_eq!(s.len(), 1);
    }

    #[test]
    fn a_cron_schedule_can_gate_on_the_weekday() {
        let mut s = Scheduler::new();
        s.add(ScheduleSpec::cron("0 9 * * mon"));
        let clock = LocalClock::default();
        assert_eq!(s.poll(ts(0, 9, 0, 0), clock).len(), 1, "monday");
        assert!(s.poll(ts(1, 9, 0, 0), clock).is_empty(), "tuesday");
        assert!(s.poll(ts(6, 9, 0, 0), clock).is_empty(), "sunday");
        assert_eq!(s.poll(ts(7, 9, 0, 0), clock).len(), 1, "next monday");
    }

    #[test]
    fn a_cron_schedule_can_gate_on_the_day_of_month() {
        let mut s = Scheduler::new();
        s.add(ScheduleSpec::cron("0 0 1 * *"));
        let clock = LocalClock::default();
        // Day 0 is 1 January 2024, which matches.
        assert_eq!(s.poll(ts(0, 0, 0, 0), clock).len(), 1);
        // Day 1 is the 2nd, which does not.
        assert!(s.poll(ts(1, 0, 0, 0), clock).is_empty());
        // Day 31 of January is the 1st of February in a leap year (2024-02-01).
        assert_eq!(s.poll(ts(31, 0, 0, 0), clock).len(), 1);
        assert!(s.poll(ts(32, 0, 0, 0), clock).is_empty());
    }

    #[test]
    fn a_cron_one_shot_fires_exactly_once_ever() {
        let mut spec = ScheduleSpec::cron("0 * * * *");
        spec.once = true;
        let clock = LocalClock::default();
        let mut s = Scheduler::new();
        s.add(spec.clone());
        assert_eq!(s.poll(ts(0, 9, 0, 0), clock).len(), 1);
        assert!(s.poll(ts(0, 10, 0, 0), clock).is_empty(), "later the same day");
        assert!(s.poll(ts(1, 9, 0, 0), clock).is_empty(), "the next day");

        // And it survives a restart from persisted state.
        let saved = serde_json::to_string(s.state()).unwrap();
        let mut restarted = Scheduler::new();
        restarted.add(spec);
        restarted.restore(serde_json::from_str(&saved).unwrap());
        assert!(restarted.poll(ts(2, 9, 0, 0), clock).is_empty());
    }

    #[test]
    fn an_unparseable_cron_schedule_never_fires_but_still_labels_itself() {
        let mut s = Scheduler::new();
        s.add(ScheduleSpec::cron("not a cron"));
        assert!(s.poll(ts(0, 0, 0, 0), LocalClock::default()).is_empty());
        assert_eq!(s.entries.keys().next().unwrap(), "cron not a cron");
    }
}
