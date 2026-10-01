//! The protocol-neutral outbound command an endpoint understands.

use tpt_app_av_automation_model::{ActionSpec, DmxTransport, MediaOperation};

/// A concrete outbound command, derived from an [`ActionSpec`].
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// Send an OSC message.
    Osc {
        /// Address.
        address: String,
        /// Numeric arguments.
        args: Vec<f64>,
    },
    /// Send a MIDI 1.0 message.
    Midi {
        /// Zero-based channel.
        channel: u8,
        /// `note_on`, `note_off`, `cc` or `program_change`.
        kind: String,
        /// Note/CC/program number.
        number: u8,
        /// Velocity or CC value.
        value: u16,
    },
    /// Write consecutive DMX channels.
    DmxChannels {
        /// Universe.
        universe: u16,
        /// Zero-based first channel.
        start_channel: u16,
        /// Values.
        values: Vec<u8>,
        /// Transport.
        transport: DmxTransport,
    },
    /// Recall a named scene defined on the device.
    DmxScene {
        /// Scene name.
        scene: String,
        /// Fade time in ms.
        fade_ms: u64,
    },
    /// Write a whole universe.
    DmxUniverse {
        /// Universe.
        universe: u16,
        /// Up to 512 values.
        values: Vec<u8>,
        /// Transport.
        transport: DmxTransport,
    },
    /// Start/stop/switch a video source.
    VideoSource {
        /// Source name.
        source: String,
        /// Operation.
        operation: MediaOperation,
    },
    /// Start/stop/switch an audio route.
    AudioRoute {
        /// Source endpoint.
        from: String,
        /// Destination endpoint.
        to: String,
        /// Operation.
        operation: MediaOperation,
    },
}

impl Command {
    /// Converts an endpoint-writing action into a command; other actions return `None`.
    pub fn from_spec(spec: &ActionSpec) -> Option<Command> {
        Some(match spec {
            ActionSpec::Osc { address, args } => Command::Osc {
                address: address.clone(),
                args: args.clone(),
            },
            ActionSpec::Midi {
                channel,
                kind,
                number,
                value,
            } => Command::Midi {
                channel: *channel,
                kind: kind.clone(),
                number: *number,
                value: *value,
            },
            ActionSpec::DmxChannels {
                universe,
                start_channel,
                values,
                transport,
            } => Command::DmxChannels {
                universe: *universe,
                start_channel: *start_channel,
                values: values.clone(),
                transport: *transport,
            },
            ActionSpec::DmxScene { scene, fade_ms } => Command::DmxScene {
                scene: scene.clone(),
                fade_ms: *fade_ms,
            },
            ActionSpec::DmxUniverse {
                universe,
                values,
                transport,
            } => Command::DmxUniverse {
                universe: *universe,
                values: values.clone(),
                transport: *transport,
            },
            ActionSpec::MediaVideoSource { source, operation } => Command::VideoSource {
                source: source.clone(),
                operation: *operation,
            },
            ActionSpec::MediaAudioRoute {
                from,
                to,
                operation,
            } => Command::AudioRoute {
                from: from.clone(),
                to: to.clone(),
                operation: *operation,
            },
            _ => return None,
        })
    }

    /// One-line human description, used by simulation output and logs.
    pub fn describe(&self) -> String {
        match self {
            Command::Osc { address, args } => {
                if args.is_empty() {
                    format!("osc {address}")
                } else {
                    let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
                    format!("osc {address} = {}", args.join(" "))
                }
            }
            Command::Midi {
                channel,
                kind,
                number,
                value,
            } => format!("midi ch{channel} {kind} {number} {value}"),
            Command::DmxChannels {
                universe,
                start_channel,
                values,
                ..
            } => format!(
                "dmx u{universe} from ch{start_channel} ({} values)",
                values.len()
            ),
            Command::DmxScene { scene, fade_ms } => {
                format!("dmx scene `{scene}` fade {fade_ms}ms")
            }
            Command::DmxUniverse {
                universe, values, ..
            } => format!("dmx universe {universe} ({} values)", values.len()),
            Command::VideoSource { source, operation } => {
                format!("video {} `{source}`", operation_label(*operation))
            }
            Command::AudioRoute {
                from,
                to,
                operation,
            } => format!("audio {} `{from}` -> `{to}`", operation_label(*operation)),
        }
    }
}

fn operation_label(op: MediaOperation) -> &'static str {
    match op {
        MediaOperation::Start => "start",
        MediaOperation::Stop => "stop",
        MediaOperation::Switch => "switch",
    }
}
