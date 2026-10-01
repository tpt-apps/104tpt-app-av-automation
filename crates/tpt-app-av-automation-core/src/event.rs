//! The normalized inbound event and the protocol/health vocabulary around it.

use std::collections::HashMap;
use std::fmt;

use serde::{Deserialize, Serialize};

/// Which transport a control message arrived on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Protocol {
    /// Open Sound Control over UDP.
    Osc,
    /// MIDI 1.0 / 2.0.
    Midi,
    /// DMX512 over Art-Net.
    ArtNet,
    /// DMX512 over sACN (E1.31).
    Sacn,
    /// Plain DMX512 serial.
    Dmx512,
}

impl fmt::Display for Protocol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Protocol::Osc => "osc",
            Protocol::Midi => "midi",
            Protocol::ArtNet => "artnet",
            Protocol::Sacn => "sacn",
            Protocol::Dmx512 => "dmx512",
        };
        f.write_str(name)
    }
}

/// A DMX universe/channel observation, normalized from Art-Net, sACN or DMX512.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DmxLevel {
    /// Universe number.
    pub universe: u16,
    /// Zero-based channel index within the universe.
    pub channel: u16,
    /// Channel value, 0-255.
    pub value: u8,
}

/// A MIDI observation, normalized from MIDI 1.0 or MIDI 2.0/UMP.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct MidiLevel {
    /// Zero-based MIDI channel.
    pub channel: u8,
    /// Message kind discriminator, e.g. `note_on`, `cc`, `program_change`.
    ///
    /// Serialized as `message`: `Event` is internally tagged with `kind`, and a second `kind` key
    /// would make every persisted MIDI execution unreadable.
    #[serde(rename = "message")]
    pub kind: String,
    /// Message number: note number, CC number, program number or controller index.
    pub number: u8,
    /// Primary value: velocity, CC value or program value.
    ///
    /// MIDI 1.0 is 7- or 16-bit wide, so this is the natural place to compare against a rule's
    /// `value`. MIDI 2.0 carries a full 32-bit value, which is also reported in `value32`.
    pub value: u16,
    /// The full 32-bit value of a MIDI 2.0 message, when the source was a UMP packet.
    ///
    /// `None` for MIDI 1.0, which has no 32-bit data. Rules can compare against it for 16-bit
    /// velocity or per-note resolution that MIDI 1.0 cannot express.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value32: Option<u32>,
    /// The UMP port group (0-15) a MIDI 2.0 message arrived on.
    ///
    /// `None` for MIDI 1.0 and for a UMP message whose group was not meaningful. Rules may select
    /// on it to separate independent MIDI 2. sources sharing one cable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<u8>,
}

impl MidiLevel {
    /// A MIDI 1.0 observation: no 32-bit value and no UMP group.
    pub fn midi1(channel: u8, kind: impl Into<String>, number: u8, value: u16) -> Self {
        Self {
            channel,
            kind: kind.into(),
            number,
            value,
            value32: None,
            group: None,
        }
    }

    /// The value a rule compares against: the 32-bit value when there is one, else the 1.0 value.
    pub fn effective_value(&self) -> u32 {
        self.value32.unwrap_or(u32::from(self.value))
    }
}

/// Health of a monitored device/endpoint (spec §6.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceHealth {
    /// No state has been reported yet.
    Unknown,
    /// Recently seen and behaving normally.
    Online,
    /// Seen but reporting errors or partial functionality.
    Degraded,
    /// Heartbeat missed; considered unreachable.
    Offline,
}

impl DeviceHealth {
    /// Whether the health state is bad enough to require operator attention.
    pub fn is_problem(self) -> bool {
        matches!(self, DeviceHealth::Degraded | DeviceHealth::Offline)
    }
}

impl fmt::Display for DeviceHealth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            DeviceHealth::Unknown => "unknown",
            DeviceHealth::Online => "online",
            DeviceHealth::Degraded => "degraded",
            DeviceHealth::Offline => "offline",
        };
        f.write_str(name)
    }
}
/// The normalized event every trigger matches against (spec §10).
///
/// Triggers never see raw wire bytes: raw OSC/MIDI/DMX payloads are parsed and validated at the
/// edge, then normalized into this type. That keeps inbound traffic treated as untrusted
/// (spec §16) and keeps rule matching deterministic.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Event {
    /// A schedule trigger fired.
    Schedule {
        /// Human-readable schedule identifier, e.g. `18:55` or `every:30000ms`.
        spec: String,
    },
    /// An OSC message arrived.
    Osc {
        /// OSC address.
        address: String,
        /// Decoded numeric arguments.
        args: Vec<f64>,
    },
    /// A MIDI message arrived.
    Midi(MidiLevel),
    /// A DMX channel changed.
    Dmx(DmxLevel),
    /// A device transitioned to a new health state.
    DeviceState {
        /// Device identifier.
        device: String,
        /// Health before the transition.
        previous: DeviceHealth,
        /// Health after the transition.
        current: DeviceHealth,
    },
    /// A device heartbeat was missed for longer than its configured threshold.
    HeartbeatMissed {
        /// Device identifier.
        device: String,
        /// Milliseconds since the last heartbeat.
        missed_millis: u64,
    },
    /// A device reported a parameter value.
    DeviceParameter {
        /// Device identifier.
        device: String,
        /// Parameter name.
        parameter: String,
        /// Reported value.
        value: f64,
    },
    /// An operator pressed "run now".
    Manual {
        /// Optional rule id the operator targeted.
        rule: Option<String>,
    },
    /// The local API (or CLI) invoked a rule.
    Api {
        /// API entry point name.
        name: String,
        /// Free-form payload.
        payload: HashMap<String, String>,
    },
}

impl Event {
    /// Short, stable label used in logs and reports.
    pub fn kind(&self) -> &'static str {
        match self {
            Event::Schedule { .. } => "schedule",
            Event::Osc { .. } => "osc",
            Event::Midi(_) => "midi",
            Event::Dmx(_) => "dmx",
            Event::DeviceState { .. } => "device_state",
            Event::HeartbeatMissed { .. } => "heartbeat_missed",
            Event::DeviceParameter { .. } => "device_parameter",
            Event::Manual { .. } => "manual",
            Event::Api { .. } => "api",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_kind_is_stable() {
        let e = Event::Schedule {
            spec: "18:55".into(),
        };
        assert_eq!(e.kind(), "schedule");
        assert_eq!(
            serde_json::to_value(&e).unwrap(),
            serde_json::json!({"kind": "schedule", "spec": "18:55"})
        );
    }

    /// Regression: `Event::Midi` used to serialize two `kind` keys (the tag and the MIDI message
    /// kind), so a persisted MIDI execution could not be read back.
    #[test]
    fn every_event_variant_round_trips_through_json() {
        let events = vec![
            Event::Schedule { spec: "18:55".into() },
            Event::Osc {
                address: "/a".into(),
                args: vec![1.0, 2.5],
            },
            Event::Midi(MidiLevel {
                channel: 1,
                kind: "note_on".into(),
                number: 60,
                value: 127,
                ..Default::default()
            }),
            Event::Dmx(DmxLevel {
                universe: 1,
                channel: 2,
                value: 3,
            }),
            Event::DeviceState {
                device: "d".into(),
                previous: DeviceHealth::Online,
                current: DeviceHealth::Offline,
            },
            Event::HeartbeatMissed {
                device: "d".into(),
                missed_millis: 5,
            },
            Event::DeviceParameter {
                device: "d".into(),
                parameter: "p".into(),
                value: 1.5,
            },
            Event::Manual { rule: None },
            Event::Manual {
                rule: Some("r".into()),
            },
            Event::Api {
                name: "n".into(),
                payload: HashMap::new(),
            },
        ];
        for event in events {
            let json = serde_json::to_string(&event).unwrap();
            assert_eq!(json.matches("\"kind\"").count(), 1, "duplicate kind key in {json}");
            let back: Event = serde_json::from_str(&json).unwrap_or_else(|e| panic!("{json}: {e}"));
            assert_eq!(back, event);
        }
    }

    #[test]
    fn health_problem_classification() {
        assert!(DeviceHealth::Degraded.is_problem());
        assert!(DeviceHealth::Offline.is_problem());
        assert!(!DeviceHealth::Online.is_problem());
        assert!(!DeviceHealth::Unknown.is_problem());
    }

    #[test]
    fn protocols_render_lowercase() {
        assert_eq!(Protocol::ArtNet.to_string(), "artnet");
        assert_eq!(Protocol::Osc.to_string(), "osc");
    }
}