//! Parsing of `--event` specifications for `simulate` (spec §13).
//!
//! | Form | Meaning |
//! |------|---------|
//! | `schedule:18:55` or `schedule:18:55@mon` | every schedule rule due at that local time (on that weekday) |
//! | `osc:/cue/1=1,0.5` | an OSC message with numeric arguments |
//! | `midi:note_on:0:60:127` | `kind:channel:number[:value]` |
//! | `dmx:1:5=200` | `universe:channel=value` |
//! | `device:projector-1:offline` | a health transition |
//! | `heartbeat-missed:projector-1:6000` | a heartbeat missed for this many ms |
//! | `param:projector-1:lamp_hours=1200` | a device-reported parameter |
//! | `manual` or `manual:rule-id` | an operator "run now" |
//! | `api:name` | a local API call |

use std::collections::HashMap;

use tpt_app_av_automation_core::{DeviceHealth, DmxLevel, Error, Event, MidiLevel, Result};
use tpt_app_av_automation_model::{LocalTime, RulePack, TriggerSpec, Weekday};

/// One-line usage summary, appended to parse errors.
pub const USAGE: &str = "expected one of: schedule:HH:MM[@day], osc:/address[=v1,v2], \
midi:kind:channel:number[:value], dmx:universe:channel=value, device:id:state, \
heartbeat-missed:id:ms, param:id:name=value, manual[:rule], api:name";

fn bad(spec: &str, why: &str) -> Error {
    Error::Parse(format!("invalid event `{spec}`: {why}; {USAGE}"))
}

fn number<T: std::str::FromStr>(spec: &str, text: &str, what: &str) -> Result<T> {
    text.trim()
        .parse()
        .map_err(|_| bad(spec, &format!("`{text}` is not a valid {what}")))
}

/// The local time a schedule event refers to, for pinning the simulation clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScheduleAt {
    /// Local wall-clock time.
    pub time: LocalTime,
    /// Weekday, when given.
    pub weekday: Option<Weekday>,
}

/// Extracts the schedule time from a `schedule:` spec, if it is one.
pub fn schedule_time(spec: &str) -> Option<ScheduleAt> {
    let rest = spec.strip_prefix("schedule:")?;
    let (time, day) = match rest.split_once('@') {
        Some((t, d)) => (t, Some(d)),
        None => (rest, None),
    };
    Some(ScheduleAt {
        time: time.parse().ok()?,
        weekday: day.and_then(Weekday::parse),
    })
}

/// Parses one `--event` spec into the events it stands for.
pub fn parse_event(spec: &str, pack: &RulePack) -> Result<Vec<Event>> {
    let (kind, rest) = spec.split_once(':').unwrap_or((spec, ""));
    match kind {
        "schedule" => {
            let at = schedule_time(spec).ok_or_else(|| bad(spec, "expected schedule:HH:MM[@day]"))?;
            let minutes = at.time.minutes_since_midnight();
            let mut events: Vec<Event> = Vec::new();
            for rule in &pack.rules {
                if let TriggerSpec::Schedule(schedule) = &rule.trigger.spec {
                    let due = match at.weekday {
                        Some(day) => schedule.due_at(minutes, day.index()),
                        None => (0..7).any(|d| schedule.due_at(minutes, d)),
                    };
                    let event = Event::Schedule {
                        spec: schedule.label(),
                    };
                    if due && !events.contains(&event) {
                        events.push(event);
                    }
                }
            }
            Ok(events)
        }
        "osc" => {
            let (address, args) = rest.split_once('=').unwrap_or((rest, ""));
            if !address.starts_with('/') {
                return Err(bad(spec, "an OSC address must start with `/`"));
            }
            let args = args
                .split(',')
                .filter(|a| !a.trim().is_empty())
                .map(|a| number::<f64>(spec, a, "number"))
                .collect::<Result<Vec<_>>>()?;
            Ok(vec![Event::Osc {
                address: address.to_owned(),
                args,
            }])
        }
        "midi" => {
            let parts: Vec<&str> = rest.split(':').collect();
            if !(3..=4).contains(&parts.len()) {
                return Err(bad(spec, "expected midi:kind:channel:number[:value]"));
            }
            if !matches!(parts[0], "note_on" | "note_off" | "cc" | "program_change") {
                return Err(bad(spec, "kind must be note_on, note_off, cc or program_change"));
            }
            Ok(vec![Event::Midi(MidiLevel {
                kind: parts[0].to_owned(),
                channel: number(spec, parts[1], "channel")?,
                number: number(spec, parts[2], "number")?,
                value: match parts.get(3) {
                    Some(v) => number(spec, v, "value")?,
                    None => 0,
                },
            })])
        }
        "dmx" => {
            let (target, value) = rest
                .split_once('=')
                .ok_or_else(|| bad(spec, "expected dmx:universe:channel=value"))?;
            let (universe, channel) = target
                .split_once(':')
                .ok_or_else(|| bad(spec, "expected dmx:universe:channel=value"))?;
            Ok(vec![Event::Dmx(DmxLevel {
                universe: number(spec, universe, "universe")?,
                channel: number(spec, channel, "channel")?,
                value: number(spec, value, "value (0-255)")?,
            })])
        }
        "device" => {
            let (device, state) = rest
                .split_once(':')
                .ok_or_else(|| bad(spec, "expected device:id:state"))?;
            let current = match state {
                "online" => DeviceHealth::Online,
                "degraded" => DeviceHealth::Degraded,
                "offline" => DeviceHealth::Offline,
                "unknown" => DeviceHealth::Unknown,
                _ => return Err(bad(spec, "state must be online, degraded, offline or unknown")),
            };
            Ok(vec![Event::DeviceState {
                device: device.to_owned(),
                // The previous state is irrelevant to matching; `simulate` fills it from the registry.
                previous: DeviceHealth::Unknown,
                current,
            }])
        }
        "heartbeat-missed" => {
            let (device, ms) = rest
                .split_once(':')
                .ok_or_else(|| bad(spec, "expected heartbeat-missed:id:ms"))?;
            Ok(vec![Event::HeartbeatMissed {
                device: device.to_owned(),
                missed_millis: number(spec, ms, "number of milliseconds")?,
            }])
        }
        "param" => {
            let (target, value) = rest
                .split_once('=')
                .ok_or_else(|| bad(spec, "expected param:id:name=value"))?;
            let (device, parameter) = target
                .split_once(':')
                .ok_or_else(|| bad(spec, "expected param:id:name=value"))?;
            Ok(vec![Event::DeviceParameter {
                device: device.to_owned(),
                parameter: parameter.to_owned(),
                value: number(spec, value, "number")?,
            }])
        }
        "manual" => Ok(vec![Event::Manual {
            rule: (!rest.is_empty()).then(|| rest.to_owned()),
        }]),
        "api" => {
            if rest.is_empty() {
                return Err(bad(spec, "expected api:name"));
            }
            Ok(vec![Event::Api {
                name: rest.to_owned(),
                payload: HashMap::new(),
            }])
        }
        _ => Err(bad(spec, "unknown event kind")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PACK: &str = r#"
format_version: 1
name: P
revision: 1
rules:
  - id: weekday
    name: Weekday
    trigger: { type: schedule, at: "18:55", days: [mon, tue] }
    actions: [ { id: a, type: notify.operator, message: x } ]
  - id: daily
    name: Daily
    trigger: { type: schedule, at: "07:00" }
    actions: [ { id: a, type: notify.operator, message: x } ]
  - id: hourly
    name: Hourly
    trigger: { type: schedule, interval_ms: 60000 }
    actions: [ { id: a, type: notify.operator, message: x } ]
"#;

    fn pack() -> RulePack {
        RulePack::from_yaml_str(PACK).unwrap()
    }

    fn parse(spec: &str) -> Result<Vec<Event>> {
        parse_event(spec, &pack())
    }

    #[test]
    fn schedule_events_match_the_schedules_due_at_that_time() {
        assert_eq!(
            parse("schedule:18:55").unwrap(),
            vec![Event::Schedule {
                spec: "mon,tue 18:55".into()
            }]
        );
        assert_eq!(parse("schedule:07:00").unwrap(), vec![Event::Schedule { spec: "07:00".into() }]);
        assert!(parse("schedule:12:00").unwrap().is_empty(), "nothing is due");
        assert!(parse("schedule:18:55@wed").unwrap().is_empty(), "wrong weekday");
        assert_eq!(parse("schedule:18:55@tue").unwrap().len(), 1);
    }

    #[test]
    fn schedule_time_extraction() {
        let at = schedule_time("schedule:18:55@fri").unwrap();
        assert_eq!(at.time.minutes_since_midnight(), 18 * 60 + 55);
        assert_eq!(at.weekday, Some(Weekday::Fri));
        assert!(schedule_time("osc:/x").is_none());
        assert!(schedule_time("schedule:25:00").is_none());
    }

    #[test]
    fn osc_midi_and_dmx() {
        assert_eq!(
            parse("osc:/cue/1=1,0.5").unwrap(),
            vec![Event::Osc {
                address: "/cue/1".into(),
                args: vec![1.0, 0.5]
            }]
        );
        assert_eq!(
            parse("osc:/go").unwrap(),
            vec![Event::Osc {
                address: "/go".into(),
                args: vec![]
            }]
        );
        assert_eq!(
            parse("midi:note_on:0:60:127").unwrap(),
            vec![Event::Midi(MidiLevel {
                channel: 0,
                kind: "note_on".into(),
                number: 60,
                value: 127
            })]
        );
        assert_eq!(
            parse("dmx:1:5=200").unwrap(),
            vec![Event::Dmx(DmxLevel {
                universe: 1,
                channel: 5,
                value: 200
            })]
        );
    }

    #[test]
    fn device_and_manual_events() {
        assert!(matches!(
            parse("device:proj:offline").unwrap()[0],
            Event::DeviceState { current: DeviceHealth::Offline, .. }
        ));
        assert_eq!(
            parse("heartbeat-missed:proj:6000").unwrap(),
            vec![Event::HeartbeatMissed {
                device: "proj".into(),
                missed_millis: 6000
            }]
        );
        assert_eq!(
            parse("param:proj:lamp=12.5").unwrap(),
            vec![Event::DeviceParameter {
                device: "proj".into(),
                parameter: "lamp".into(),
                value: 12.5
            }]
        );
        assert_eq!(parse("manual").unwrap(), vec![Event::Manual { rule: None }]);
        assert_eq!(
            parse("manual:daily").unwrap(),
            vec![Event::Manual {
                rule: Some("daily".into())
            }]
        );
        assert!(matches!(parse("api:ping").unwrap()[0], Event::Api { .. }));
    }

    #[test]
    fn malformed_specs_are_errors_with_usage() {
        for bad in [
            "",
            "nonsense",
            "osc:no-slash",
            "osc:/x=abc",
            "midi:note_on:0",
            "midi:bogus:0:1:2",
            "midi:note_on:999:1:2",
            "midi:note_on:0:60:x",
            "dmx:1:5",
            "dmx:1:5=300",
            "dmx:x:5=1",
            "device:proj",
            "device:proj:sideways",
            "heartbeat-missed:proj:soon",
            "param:proj=1",
            "api:",
            "schedule:99:99",
        ] {
            let err = parse(bad).expect_err(bad);
            assert!(err.to_string().contains("expected one of"), "{bad}: {err}");
        }
    }
}
