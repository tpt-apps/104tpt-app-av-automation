//! Trigger definitions and matching (spec §6.2, §7).

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use tpt_app_av_automation_core::{DeviceHealth, Event, Result};

use crate::condition::Comparison;
use crate::cron::CronSchedule;

/// The local wall-clock fields a schedule is evaluated against.
///
/// `day_of_month` and `month` are `None` for callers that only know the time of day (the CLI's
/// `--event schedule:HH:MM` form); a [`ScheduleSpec::cron`] expression then ignores them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScheduleMoment {
    /// Minutes since local midnight, 0-1439.
    pub minute: u32,
    /// Monday-first weekday index, `0` = Monday.
    pub weekday_index: usize,
    /// Day of the local month, 1-31, when known.
    pub day_of_month: Option<u32>,
    /// Month of the local year, 1-12, when known.
    pub month: Option<u32>,
}

impl ScheduleMoment {
    /// A moment with only a time of day and a weekday.
    pub fn at(minute: u32, weekday_index: usize) -> Self {
        Self {
            minute,
            weekday_index,
            day_of_month: None,
            month: None,
        }
    }
}

/// Days of the week accepted by schedule triggers, using YAML-friendly lowercase names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Weekday {
    /// Monday.
    Mon,
    /// Tuesday.
    Tue,
    /// Wednesday.
    Wed,
    /// Thursday.
    Thu,
    /// Friday.
    Fri,
    /// Saturday.
    Sat,
    /// Sunday.
    Sun,
}

impl Weekday {
    /// All weekdays, Monday first.
    pub const ALL: [Weekday; 7] = [
        Weekday::Mon,
        Weekday::Tue,
        Weekday::Wed,
        Weekday::Thu,
        Weekday::Fri,
        Weekday::Sat,
        Weekday::Sun,
    ];

    /// Converts to a Monday-first weekday index (`std::time::Weekday` numbering).
    pub const fn index(self) -> usize {
        match self {
            Weekday::Mon => 0,
            Weekday::Tue => 1,
            Weekday::Wed => 2,
            Weekday::Thu => 3,
            Weekday::Fri => 4,
            Weekday::Sat => 5,
            Weekday::Sun => 6,
        }
    }

    /// Inverse of [`Weekday::index`].
    pub const fn from_index(index: usize) -> Option<Weekday> {
        match index {
            0 => Some(Weekday::Mon),
            1 => Some(Weekday::Tue),
            2 => Some(Weekday::Wed),
            3 => Some(Weekday::Thu),
            4 => Some(Weekday::Fri),
            5 => Some(Weekday::Sat),
            6 => Some(Weekday::Sun),
            _ => None,
        }
    }

    /// Parses a three-letter or full weekday name, case-insensitively.
    pub fn parse(text: &str) -> Option<Weekday> {
        match text.trim().to_ascii_lowercase().as_str() {
            "mon" | "monday" => Some(Weekday::Mon),
            "tue" | "tues" | "tuesday" => Some(Weekday::Tue),
            "wed" | "weds" | "wednesday" => Some(Weekday::Wed),
            "thu" | "thur" | "thurs" | "thursday" => Some(Weekday::Thu),
            "fri" | "friday" => Some(Weekday::Fri),
            "sat" | "saturday" => Some(Weekday::Sat),
            "sun" | "sunday" => Some(Weekday::Sun),
            _ => None,
        }
    }
}

impl fmt::Display for Weekday {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Weekday::Mon => "mon",
            Weekday::Tue => "tue",
            Weekday::Wed => "wed",
            Weekday::Thu => "thu",
            Weekday::Fri => "fri",
            Weekday::Sat => "sat",
            Weekday::Sun => "sun",
        };
        f.write_str(name)
    }
}

/// Wall-clock time of day in the site's local timezone, `HH:MM` in 24-hour form.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct LocalTime {
    /// Hour, 0-23.
    pub hour: u8,
    /// Minute, 0-59.
    pub minute: u8,
}

impl LocalTime {
    /// Builds a time from components, returning `None` when out of range.
    pub const fn new(hour: u8, minute: u8) -> Option<Self> {
        if hour > 23 || minute > 59 {
            return None;
        }
        Some(Self { hour, minute })
    }

    /// Minutes since local midnight.
    pub const fn minutes_since_midnight(self) -> u32 {
        (self.hour as u32) * 60 + self.minute as u32
    }
}
/// A time-based trigger specification (spec §7.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScheduleSpec {
    /// Fire at this local wall-clock time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<LocalTime>,
    /// Fire on a five-field cron expression, `minute hour day-of-month month day-of-week`.
    ///
    /// An alternative to `at` for anything a single time-of-day cannot express: several minutes per
    /// hour, hour ranges, month and weekday selection. See [`CronSchedule`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cron: Option<String>,
    /// Repeat every N milliseconds instead of at a fixed time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interval_ms: Option<u64>,
    /// Only fire on these weekdays. Empty means every day. Ignored when `cron` sets the weekday.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub days: Vec<Weekday>,
    /// Fire once at `at` or at the first `cron` match, then never again — the form that must not
    /// re-fire after a restart (spec §11).
    #[serde(default)]
    pub once: bool,
}

impl ScheduleSpec {
    /// Builds a daily schedule.
    pub fn daily(at: LocalTime) -> Self {
        Self {
            at: Some(at),
            cron: None,
            interval_ms: None,
            days: Vec::new(),
            once: false,
        }
    }

    /// Builds an interval schedule.
    pub fn interval(interval_ms: u64) -> Self {
        Self {
            at: None,
            cron: None,
            interval_ms: Some(interval_ms),
            days: Vec::new(),
            once: false,
        }
    }

    /// Builds a one-shot schedule at a fixed time on the given days.
    pub fn one_shot(at: LocalTime, days: Vec<Weekday>) -> Self {
        Self {
            at: Some(at),
            cron: None,
            interval_ms: None,
            days,
            once: true,
        }
    }

    /// Builds a schedule from a cron expression.
    ///
    /// The expression is not validated here — that is the validator's job, so a bad expression is
    /// a located diagnostic rather than a parse failure.
    pub fn cron(expression: impl Into<String>) -> Self {
        Self {
            at: None,
            cron: Some(expression.into()),
            interval_ms: None,
            days: Vec::new(),
            once: false,
        }
    }

    /// The parsed cron expression, if this schedule has a valid one.
    pub fn parsed_cron(&self) -> Option<CronSchedule> {
        CronSchedule::parse(self.cron.as_deref()?).ok()
    }

    /// Stable label used in reports and as the [`tpt_app_av_automation_core::Event::Schedule`] spec.
    pub fn label(&self) -> String {
        if let Some(interval) = self.interval_ms {
            return format!("every:{interval}ms");
        }
        if let Some(cron) = self.parsed_cron() {
            let body = format!("cron {}", cron.to_expression());
            return if self.once {
                format!("{body} (once)")
            } else {
                body
            };
        }
        // An unparseable expression is rejected by validation; label it verbatim so the offending
        // text still reaches the operator through execution records.
        if let Some(raw) = &self.cron {
            return format!("cron {raw}");
        }
        match self.at {
            Some(at) if self.days.is_empty() && !self.once => at.to_string(),
            Some(at) => {
                let days = if self.days.is_empty() {
                    "daily".to_string()
                } else {
                    self.days
                        .iter()
                        .map(|d| d.to_string())
                        .collect::<Vec<_>>()
                        .join(",")
                };
                if self.once {
                    format!("{days} {at} (once)")
                } else {
                    format!("{days} {at}")
                }
            }
            None => "unscheduled".to_string(),
        }
    }

    /// Whether `now` falls on one of the configured days, given a Monday-first weekday index.
    pub fn day_matches(&self, weekday_index: usize) -> bool {
        self.days.is_empty() || self.days.iter().any(|d| d.index() == weekday_index % 7)
    }

    /// Whether a fixed-time schedule should fire at `now`.
    ///
    /// Firing is edge-triggered and idempotent within the minute: it returns `true` only when the
    /// current local minute equals the target, so a scheduler polling every 30s fires exactly once
    /// and the answer depends only on the supplied time, never on poll timing.
    pub fn due_at(&self, now_minutes_since_midnight: u32, weekday_index: usize) -> bool {
        self.due_minute(&ScheduleMoment::at(now_minutes_since_midnight, weekday_index))
    }

    /// Whether this schedule is due at the given local wall-clock moment.
    ///
    /// Handles both fixed times and cron expressions. A cron schedule ignores the `days` list,
    /// because the expression's own day-of-week field is the authoritative one; validation rejects
    /// the two being combined.
    ///
    /// Interval schedules never report due here — they are advanced by the scheduler's own period
    /// accounting, which must not depend on the wall clock.
    pub fn due_minute(&self, moment: &ScheduleMoment) -> bool {
        if let Some(cron) = self.parsed_cron() {
            let day = moment.day_of_month.unwrap_or(1);
            let month = moment.month.unwrap_or(1);
            // `ScheduleMoment.minute` counts from midnight; cron counts from the hour.
            return cron.matches(
                moment.minute % 60,
                moment.minute / 60,
                day,
                month,
                moment.weekday_index,
            );
        }
        if !self.day_matches(moment.weekday_index) {
            return false;
        }
        match self.at {
            Some(at) => at.minutes_since_midnight() == moment.minute,
            // Interval schedules are advanced by the scheduler, not by wall-clock comparison.
            None => false,
        }
    }
}

/// Which MIDI message kinds a control trigger can match.
///
/// The first four are common to MIDI 1.0 and MIDI 2.0. The rest exist only in MIDI 2.0 (spec §7.2);
/// they never match a MIDI 1.0 observation, because that message kind cannot be produced by a
/// MIDI 1.0 byte stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MidiMessageKind {
    /// Note-on (including velocity-zero note-on used as note-off).
    NoteOn,
    /// Note-off.
    NoteOff,
    /// Control change.
    ControlChange,
    /// Program change.
    ProgramChange,
    /// Channel (aftertouch) pressure, 32-bit.
    ChannelPressure,
    /// Per-note pressure, 32-bit.
    PolyphonicPressure,
    /// Pitch bend, 32-bit and centred on `0x8000_0000`.
    PitchBend,
    /// Registered parameter number.
    Rpn,
    /// Non-registered parameter number.
    Nrpn,
    /// Relative registered parameter number (a signed delta).
    RelativeRpn,
    /// Relative non-registered parameter number (a signed delta).
    RelativeNrpn,
    /// Registered per-note controller.
    PerNoteRcc,
    /// Assignable per-note controller.
    PerNoteAcc,
    /// Per-note pitch bend.
    PerNotePitchBend,
    /// Per-note management (note attributes and controller reset flags).
    PerNoteManagement,
}

impl MidiMessageKind {
    /// The canonical string form used in normalized events.
    pub fn as_str(self) -> &'static str {
        match self {
            MidiMessageKind::NoteOn => "note_on",
            MidiMessageKind::NoteOff => "note_off",
            MidiMessageKind::ControlChange => "cc",
            MidiMessageKind::ProgramChange => "program_change",
            MidiMessageKind::ChannelPressure => "channel_pressure",
            MidiMessageKind::PolyphonicPressure => "polyphonic_pressure",
            MidiMessageKind::PitchBend => "pitch_bend",
            MidiMessageKind::Rpn => "rpn",
            MidiMessageKind::Nrpn => "nrpn",
            MidiMessageKind::RelativeRpn => "relative_rpn",
            MidiMessageKind::RelativeNrpn => "relative_nrpn",
            MidiMessageKind::PerNoteRcc => "per_note_rcc",
            MidiMessageKind::PerNoteAcc => "per_note_acc",
            MidiMessageKind::PerNotePitchBend => "per_note_pitch_bend",
            MidiMessageKind::PerNoteManagement => "per_note_management",
        }
    }

    /// Every kind, in declaration order.
    pub const ALL: [MidiMessageKind; 15] = [
        MidiMessageKind::NoteOn,
        MidiMessageKind::NoteOff,
        MidiMessageKind::ControlChange,
        MidiMessageKind::ProgramChange,
        MidiMessageKind::ChannelPressure,
        MidiMessageKind::PolyphonicPressure,
        MidiMessageKind::PitchBend,
        MidiMessageKind::Rpn,
        MidiMessageKind::Nrpn,
        MidiMessageKind::RelativeRpn,
        MidiMessageKind::RelativeNrpn,
        MidiMessageKind::PerNoteRcc,
        MidiMessageKind::PerNoteAcc,
        MidiMessageKind::PerNotePitchBend,
        MidiMessageKind::PerNoteManagement,
    ];

    /// Whether a message of this kind can only be carried by MIDI 2.0 (UMP).
    pub fn is_midi2_only(self) -> bool {
        !matches!(
            self,
            MidiMessageKind::NoteOn
                | MidiMessageKind::NoteOff
                | MidiMessageKind::ControlChange
                | MidiMessageKind::ProgramChange
        )
    }
}

/// How a DMX channel value is compared (spec §7.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DmxComparison {
    /// Value equals `value`.
    Equals,
    /// Value is strictly greater than `value`.
    GreaterThan,
    /// Value is strictly less than `value`.
    LessThan,
    /// Value crossed above `value` since the previous observation.
    CrossedAbove,
    /// Value crossed below `value` since the previous observation.
    CrossedBelow,
    /// Any change at all.
    Changed,
}

impl DmxComparison {
    /// Applies the comparison.
    ///
    /// `previous` is `None` when there is no prior observation; edge comparisons then return
    /// `false` rather than guessing, which keeps evaluation deterministic.
    pub fn evaluate(self, previous: Option<u8>, current: u8, value: u8) -> bool {
        match self {
            DmxComparison::Equals => current == value,
            DmxComparison::GreaterThan => current > value,
            DmxComparison::LessThan => current < value,
            DmxComparison::CrossedAbove => previous.is_some_and(|p| p <= value && current > value),
            DmxComparison::CrossedBelow => previous.is_some_and(|p| p >= value && current < value),
            DmxComparison::Changed => previous.is_some_and(|p| p != current),
        }
    }
}

/// A trigger specification: the declarative half of [`Trigger`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[serde(deny_unknown_fields)]
pub enum TriggerSpec {
    /// Time-based trigger (spec §7.1).
    Schedule(ScheduleSpec),
    /// OSC message matching (spec §7.2).
    #[serde(rename = "osc")]
    Osc {
        /// OSC address pattern; `*` matches one path segment.
        address: String,
        /// First argument must equal this, when present.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        arg_equals: Option<f64>,
        /// Require at least this many arguments.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        min_args: Option<usize>,
    },
    /// MIDI message matching (spec §7.2).
    #[serde(rename = "midi")]
    Midi {
        /// Message kind.
        message: MidiMessageKind,
        /// Zero-based channel; `None` means any channel.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        channel: Option<u8>,
        /// Note/CC/program number; `None` means any.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        number: Option<u8>,
        /// Value test applied to velocity/CC value.
        ///
        /// Compared against the observation's effective value: the 32-bit value for a MIDI 2.0
        /// message, the MIDI 1.0 value otherwise. MIDI 1.0 rules are unaffected, because their
        /// values are below 0x1_0000.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        value: Option<u32>,
        /// UMP port group (0-15); `None` means any group.
        ///
        /// MIDI 2.0 only: a MIDI 1.0 observation has no group, so a rule that names one can only
        /// be satisfied by a UMP source.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        group: Option<u8>,
    },
    /// DMX/Art-Net/sACN channel change (spec §7.2).
    #[serde(rename = "dmx")]
    Dmx {
        /// Universe number.
        universe: u16,
        /// Zero-based channel index.
        channel: u16,
        /// How to compare the value.
        comparison: DmxComparison,
        /// Threshold for the comparison.
        #[serde(default)]
        value: u8,
    },
    /// Device health transition (spec §7.3).
    #[serde(rename = "device_state")]
    DeviceState {
        /// Device identifier.
        device: String,
        /// Health state to match.
        state: DeviceHealth,
    },
    /// Heartbeat missed for longer than a threshold (spec §7.3).
    #[serde(rename = "heartbeat_missed")]
    HeartbeatMissed {
        /// Device identifier.
        device: String,
        /// Threshold in milliseconds.
        after_ms: u64,
    },
    /// Device-reported parameter change (spec §7.3).
    #[serde(rename = "device_parameter")]
    DeviceParameter {
        /// Device identifier.
        device: String,
        /// Parameter name.
        parameter: String,
        /// Comparison applied to the reported value.
        comparison: Comparison,
        /// Threshold for the comparison.
        value: f64,
    },
    /// Operator "run now" (spec §7.6).
    Manual,
    /// Local API or CLI invocation (spec §7.6).
    #[serde(rename = "api")]
    Api {
        /// API entry point name that must be invoked.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    },
}

impl TriggerSpec {
    /// Whether this variant legitimately accepts the given YAML key.
    ///
    /// `#[serde(deny_unknown_fields)]` covers struct variants, but a unit variant such as
    /// `type: manual` has no field list for serde to check against, so extra keys written beside it
    /// are silently dropped. `RulePack` therefore audits trigger mappings against this predicate
    /// during parsing, which keeps `trigger: { type: manual, evnt: x }` a hard error (spec §9).
    pub fn accepts_key(&self, key: &str) -> bool {
        // `type` selects the variant and `dmx_previous` belongs to the flattened `Trigger` wrapper.
        if key == "type" || key == "dmx_previous" {
            return true;
        }
        match self {
            TriggerSpec::Schedule(_) => crate::trigger::schedule_keys().contains(&key),
            TriggerSpec::Osc { .. } => ["address", "arg_equals", "min_args"].contains(&key),
            TriggerSpec::Midi { .. } => ["message", "channel", "number", "value", "group"].contains(&key),
            TriggerSpec::Dmx { .. } => {
                ["universe", "channel", "comparison", "value"].contains(&key)
            }
            TriggerSpec::DeviceState { .. } => ["device", "state"].contains(&key),
            TriggerSpec::HeartbeatMissed { .. } => ["device", "after_ms"].contains(&key),
            TriggerSpec::DeviceParameter { .. } => {
                ["device", "parameter", "comparison", "value"].contains(&key)
            }
            TriggerSpec::Manual => false,
            TriggerSpec::Api { .. } => key == "name",
        }
    }
}

/// Keys accepted inside a `schedule` trigger body.
fn schedule_keys() -> &'static [&'static str] {
    &["at", "cron", "interval_ms", "days", "once"]
}

impl fmt::Display for LocalTime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:02}:{:02}", self.hour, self.minute)
    }
}

impl<'de> Deserialize<'de> for LocalTime {
    /// Accepts the `"HH:MM"` string form used throughout the rule format.
    ///
    /// YAML authors write `at: "18:55"`, not a nested mapping, so the string form is the canonical
    /// representation. A struct form `{ hour: 18, minute: 55 }` is also accepted for completeness.
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Parts {
            hour: u8,
            minute: u8,
        }

        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Repr {
            Text(String),
            Parts(Parts),
        }

        match Repr::deserialize(deserializer)? {
            Repr::Text(text) => text.parse().map_err(|e: crate::trigger::ParseTimeError| {
                serde::de::Error::custom(e.to_string())
            }),
            Repr::Parts(Parts { hour, minute }) => LocalTime::new(hour, minute).ok_or_else(|| {
                serde::de::Error::custom(format!("time out of range: {hour}:{minute}"))
            }),
        }
    }
}

/// Error returned when a `"HH:MM"` string cannot be parsed as a [`LocalTime`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid time `{0}`: expected HH:MM with hour 0-23 and minute 0-59")]
pub struct ParseTimeError(pub String);

impl FromStr for LocalTime {
    type Err = ParseTimeError;

    fn from_str(s: &str) -> Result<Self, ParseTimeError> {
        let t = s.trim();
        let (hour, minute) = t
            .split_once(':')
            .ok_or_else(|| ParseTimeError(t.to_owned()))?;
        let hour: u8 = hour.parse().map_err(|_| ParseTimeError(t.to_owned()))?;
        let minute: u8 = minute.parse().map_err(|_| ParseTimeError(t.to_owned()))?;
        LocalTime::new(hour, minute).ok_or_else(|| ParseTimeError(t.to_owned()))
    }
}

/// A rule trigger: the declarative specification plus a matcher.
///
/// The matcher is a pure function of `(spec, event)`. No I/O, no clock reads, no device access —
/// which is what makes evaluation deterministic (spec §3.2) and unit-testable (spec §18.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Trigger {
    /// The declarative specification.
    #[serde(flatten)]
    pub spec: TriggerSpec,
    /// Extra state the matcher is allowed to consult, e.g. the previous DMX channel value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dmx_previous: Option<u8>,
}

impl Trigger {
    /// Builds a trigger from a specification with no extra context.
    pub fn new(spec: TriggerSpec) -> Self {
        Self {
            spec,
            dmx_previous: None,
        }
    }

    /// Builds an OSC address trigger.
    pub fn osc(address: impl Into<String>) -> Self {
        Self::new(TriggerSpec::Osc {
            address: address.into(),
            arg_equals: None,
            min_args: None,
        })
    }

    /// Builds a manual trigger.
    pub fn manual() -> Self {
        Self::new(TriggerSpec::Manual)
    }

    /// Stable type label, e.g. `schedule`, `osc`, `dmx`.
    pub fn type_label(&self) -> &'static str {
        match self.spec {
            TriggerSpec::Schedule(_) => "schedule",
            TriggerSpec::Osc { .. } => "osc",
            TriggerSpec::Midi { .. } => "midi",
            TriggerSpec::Dmx { .. } => "dmx",
            TriggerSpec::DeviceState { .. } => "device_state",
            TriggerSpec::HeartbeatMissed { .. } => "heartbeat_missed",
            TriggerSpec::DeviceParameter { .. } => "device_parameter",
            TriggerSpec::Manual => "manual",
            TriggerSpec::Api { .. } => "api",
        }
    }

    /// Whether this trigger requires a prior observation to evaluate (edge-triggered DMX).
    ///
    /// Such triggers cannot fire on their very first observation without a recorded previous
    /// value; callers seed `dmx_previous` from the device registry.
    pub fn needs_previous_dmx(&self) -> bool {
        match self.spec {
            TriggerSpec::Dmx {
                comparison:
                    DmxComparison::Changed | DmxComparison::CrossedAbove | DmxComparison::CrossedBelow,
                ..
            } => true,
            _ => false,
        }
    }
}

impl Trigger {
    /// Whether this trigger matches `event`.
    ///
    /// Mismatched event variants return `false` rather than erroring: the engine calls this for
    /// every rule on every event, so a rule with an OSC trigger must simply ignore DMX traffic.
    pub fn matches(&self, event: &Event) -> bool {
        match (&self.spec, event) {
            (TriggerSpec::Schedule(spec), Event::Schedule { spec: observed }) => {
                observed == &spec.label()
            }
            (
                TriggerSpec::Osc {
                    address,
                    arg_equals,
                    min_args,
                },
                Event::Osc {
                    address: observed,
                    args,
                },
            ) => {
                if !address_pattern_matches(address, observed) {
                    return false;
                }
                if let Some(min) = min_args {
                    if args.len() < *min {
                        return false;
                    }
                }
                match arg_equals {
                    Some(expected) => args.first().is_some_and(|v| approx_eq(*v, *expected)),
                    None => true,
                }
            }
            (
                TriggerSpec::Midi {
                    message,
                    channel,
                    number,
                    value,
                    group,
                },
                Event::Midi(level),
            ) => {
                level.kind == message.as_str()
                    && channel.is_none_or(|c| c == level.channel)
                    && number.is_none_or(|n| n == level.number)
                    && value.is_none_or(|v| v == level.effective_value())
                    && group.is_none_or(|g| Some(g) == level.group)
            }
            (
                TriggerSpec::Dmx {
                    universe,
                    channel,
                    comparison,
                    value,
                },
                Event::Dmx(level),
            ) => {
                level.universe == *universe
                    && level.channel == *channel
                    && comparison.evaluate(self.dmx_previous, level.value, *value)
            }
            (
                TriggerSpec::DeviceState { device, state },
                Event::DeviceState {
                    device: observed,
                    current,
                    ..
                },
            ) => device == observed && *current == *state,
            (
                TriggerSpec::HeartbeatMissed { device, after_ms },
                Event::HeartbeatMissed {
                    device: observed,
                    missed_millis,
                },
            ) => device == observed && *missed_millis >= *after_ms,
            (
                TriggerSpec::DeviceParameter {
                    device,
                    parameter,
                    comparison,
                    value,
                },
                Event::DeviceParameter {
                    device: observed,
                    parameter: observed_param,
                    value: reported,
                },
            ) => {
                device == observed
                    && parameter == observed_param
                    && comparison.evaluate_f64(*reported, *value)
            }
            (TriggerSpec::Manual, Event::Manual { rule: None }) => true,
            (TriggerSpec::Manual, Event::Manual { rule: Some(_) }) => true,
            (TriggerSpec::Api { name }, Event::Api { name: observed, .. }) => match name {
                None => true,
                Some(expected) => expected == observed,
            },
            _ => false,
        }
    }
}

/// OSC-style address pattern matching: `*` matches exactly one path segment.
///
/// `/projector/1/power` therefore matches only itself, while `/projector/*/power` matches
/// `/projector/1/power` and `/projector/2/power` but not `/projector/1/2/power`.
pub fn address_pattern_matches(pattern: &str, address: &str) -> bool {
    if pattern == address {
        return true;
    }
    if !pattern.contains('*') {
        return false;
    }
    let p: Vec<&str> = pattern.trim_start_matches('/').split('/').collect();
    let a: Vec<&str> = address.trim_start_matches('/').split('/').collect();
    if p.len() != a.len() {
        return false;
    }
    p.iter().zip(a.iter()).all(|(seg, actual)| match *seg {
        "*" | "**" => true,
        other => other.eq_ignore_ascii_case(actual),
    })
}

/// Floating-point comparison with a small tolerance, so YAML-authored thresholds behave.
pub fn approx_eq(left: f64, right: f64) -> bool {
    (left - right).abs() <= f64::EPSILON * left.abs().max(right.abs()).max(1.0) * 8.0
}
