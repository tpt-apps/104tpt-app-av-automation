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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MidiLevel {
    /// Zero-based MIDI channel.
    pub channel: u8,
    /// Message kind discriminator, e.g. `note_on`, `cc`, `program_change`.
    pub kind: String,
    /// Message number: note number, CC number or program number.
    pub number: u8,
    /// Primary value: velocity, CC value or program value.
    pub value: u16,
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