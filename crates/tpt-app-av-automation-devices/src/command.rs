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
    /// Send a MIDI message, in whichever protocol version the target endpoint speaks.
    Midi {
        /// Zero-based channel.
        channel: u8,
        /// Message kind; MIDI 1.0 kinds are `note_on`, `note_off`, `cc` and `program_change`, and a
        /// MIDI 2.0 endpoint additionally accepts the kinds listed on
        /// [`tpt_app_av_automation_model::ActionSpec::Midi`].
        kind: String,
        /// Note/CC/program number, 0-127.
        number: u8,
        /// Velocity or CC value.
        value: u16,
        /// UMP port group (0-15). `None` for a MIDI 1.0 port.
        group: Option<u8>,
        /// Full 32-bit MIDI 2.0 value; `None` means the value is MIDI 1.0 width.
        value32: Option<u32>,
        /// Enumeration index for the MIDI 2.0 kinds that carry one; `None` means index 0.
        index: Option<u8>,
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
                group,
                value32,
                index,
            } => Command::Midi {
                channel: *channel,
                kind: kind.clone(),
                number: *number,
                value: *value,
                group: *group,
                value32: *value32,
                index: *index,
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
                group,
                value32,
                ..
            } => match value32 {
                Some(v32) => format!(
                    "midi2 ch{channel} group{} {kind} {number} {v32}",
                    group.unwrap_or(0)
                ),
                None => format!("midi ch{channel} {kind} {number} {value}"),
            },
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
