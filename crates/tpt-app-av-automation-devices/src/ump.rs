//! MIDI 2.0 Universal MIDI Packet output (spec §5.1, §8.1).
//!
//! MIDI 2.0 replaces the MIDI 1.0 byte stream with 32-, 64- and 128-bit Universal MIDI Packets.
//! A channel-voice UMP is one 64-bit packet whose first word carries the message type (`0x4` for
//! MIDI 2.0, `0x2` for a MIDI 1.0 message carried inside a UMP) and the port group, and whose
//! remaining words carry the data. Encoding is delegated to `tpt-av-control-midi`, which already
//! implements the packet layout, so this module is concerned with the AV-automation concerns:
//! validating the operator's fields, choosing MIDI 1.0 versus 2.0 width, and sending one packet per
//! datagram.
//!
//! No desktop platform exposes a native MIDI 2.0 port to `midir` today, so the endpoint speaks UMP
//! over UDP to a gateway on the venue network — the same transport the inbound `ump` listener
//! already accepts.

use std::net::UdpSocket;

use tpt_av_control_midi::{Midi1ChannelVoice, Midi1Message, Midi2ChannelVoice, Midi2Message, Ump};

use tpt_app_av_automation_model::MidiMessageKind;

use crate::command::Command;
use crate::endpoint::{Endpoint, EndpointError};

/// The default UMP port for a MIDI 2.0 gateway. There is no ratified standard; this matches the
/// port common network gateways use.
pub const DEFAULT_UMP_PORT: u16 = 60500;

/// Message kinds that only exist in MIDI 2.0 and therefore require a `ump` device.
pub const MIDI2_ONLY_KINDS: [&str; 10] = [
    "per_note_rcc",
    "per_note_acc",
    "rpn",
    "nrpn",
    "relative_rpn",
    "relative_nrpn",
    "per_note_pitch_bend",
    "polyphonic_pressure",
    "channel_pressure",
    "pitch_bend",
];

/// The MIDI 1.0 kinds, which a `ump` device also accepts.
pub const MIDI1_KINDS: [&str; 4] = ["note_on", "note_off", "cc", "program_change"];

/// Every kind a MIDI action may name.
pub fn all_kinds() -> impl Iterator<Item = &'static str> {
    MidiMessageKind::names()
}

/// Whether `kind` is only meaningful on a MIDI 2.0 endpoint.
pub fn is_midi2_only(kind: &str) -> bool {
    MidiMessageKind::parse(kind).is_some_and(MidiMessageKind::is_midi2_only)
}

/// Whether `kind` addresses a specific controller and so carries an enumeration index.
fn carries_index(kind: &str) -> bool {
    MidiMessageKind::parse(kind).is_some_and(MidiMessageKind::carries_index)
}

/// An endpoint that sends MIDI 2.0 Universal MIDI Packets over UDP.
pub struct UmpEndpoint {
    socket: UdpSocket,
    group: Option<u8>,
}

impl std::fmt::Debug for UmpEndpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UmpEndpoint")
            .field("peer", &self.socket.peer_addr().ok())
            .field("group", &self.group)
            .finish()
    }
}

impl UmpEndpoint {
    /// Opens an ephemeral local socket "connected" to a UMP gateway at `address` (`host:port`).
    pub fn new(address: &str) -> std::result::Result<Self, EndpointError> {
        let socket = UdpSocket::bind("0.0.0.0:0")
            .map_err(|e| EndpointError::Unreachable(format!("bind failed: {e}")))?;
        socket
            .connect(address)
            .map_err(|e| EndpointError::Unreachable(format!("`{address}`: {e}")))?;
        Ok(Self {
            socket,
            group: None,
        })
    }

    /// Pins every send to one port group, so an action need not repeat it.
    pub fn with_group(mut self, group: u8) -> std::result::Result<Self, EndpointError> {
        if group > 15 {
            return Err(EndpointError::Rejected(format!(
                "UMP group {group} out of range 0-15"
            )));
        }
        self.group = Some(group);
        Ok(self)
    }
}
impl UmpEndpoint {
    /// Encodes a command to the Universal MIDI Packet bytes for one datagram.
    ///
    /// A MIDI 1.0 kind with no `value32` is carried inside a UMP as a MIDI 1.0 channel-voice
    /// message (packet type `0x2`), which is exactly what the standard prescribes for keeping a
    /// MIDI 1.0 instrument fed from a MIDI 2.0 stream. Supplying `value32` for a kind that also
    /// exists in MIDI 1.0 emits a full MIDI 2.0 message instead, so a 32-bit value is never
    /// silently truncated to its top 16 bits.
    pub fn encode(command: &Command) -> std::result::Result<Vec<u8>, EndpointError> {
        let Command::Midi {
            channel,
            kind,
            number,
            value,
            group,
            value32,
            index,
        } = command
        else {
            return Err(EndpointError::Rejected("not a MIDI command".into()));
        };
        if *channel > 15 {
            return Err(EndpointError::Rejected(format!(
                "MIDI channel {channel} out of range 0-15"
            )));
        }
        if *number > 127 {
            return Err(EndpointError::Rejected(format!(
                "MIDI number {number} out of range 0-127"
            )));
        }
        if let Some(index) = index {
            if carries_index(kind) && *index > 127 {
                return Err(EndpointError::Rejected(format!(
                    "MIDI index {index} out of range 0-127"
                )));
            }
        }
        let group = group.unwrap_or(0);
        if group > 15 {
            return Err(EndpointError::Rejected(format!(
                "UMP group {group} out of range 0-15"
            )));
        }

        let message = match (is_midi2_only(kind), *value32) {
            // A MIDI 2.0-only kind needs the 32-bit value. Widening the 16-bit one by default
            // would quietly send something the operator did not ask for, so its absence is an
            // error rather than a guess.
            (true, None) => {
                return Err(EndpointError::Rejected(format!(
                    "`{kind}` is a MIDI 2.0 message and needs a `value32`"
                )))
            }
            (_, Some(value32)) => Midi2Message::Midi2ChannelVoice(midi2_message(
                group,
                *channel,
                kind,
                *number,
                value32,
                index.unwrap_or(0),
            )?),
            (false, None) => Midi2Message::Midi1ChannelVoice(Midi1ChannelVoice {
                group,
                message: midi1_message(*channel, kind, *number, *value)?,
            }),
        };

        Ok(Ump::from_message(&message).to_bytes())
    }

    /// Sends one packet, applying the endpoint's group when the action does not name one.
    fn send_packet(&self, command: &Command) -> std::result::Result<(), EndpointError> {
        let mut command = command.clone();
        if let (Command::Midi { group, .. }, Some(endpoint_group)) = (&mut command, self.group) {
            if group.is_none() {
                *group = Some(endpoint_group);
            }
        }
        let bytes = Self::encode(&command)?;
        self.socket
            .send(&bytes)
            .map(|_| ())
            .map_err(|e| EndpointError::Unreachable(e.to_string()))
    }
}

impl Endpoint for UmpEndpoint {
    fn send(&self, command: &Command) -> std::result::Result<(), EndpointError> {
        self.send_packet(command)
    }
}
/// Builds the MIDI 1.0 message for the four MIDI 1.0 kinds, validating the 7-bit value.
fn midi1_message(
    channel: u8,
    kind: &str,
    number: u8,
    value: u16,
) -> std::result::Result<Midi1Message, EndpointError> {
    let value = u8::try_from(value)
        .ok()
        .filter(|v| *v <= 127)
        .ok_or_else(|| EndpointError::Rejected(format!("MIDI value {value} out of range 0-127")))?;
    Ok(match kind {
        "note_on" => Midi1Message::NoteOn {
            channel,
            note: number,
            velocity: value,
        },
        "note_off" => Midi1Message::NoteOff {
            channel,
            note: number,
            velocity: value,
        },
        "cc" => Midi1Message::ControlChange {
            channel,
            controller: number,
            value,
        },
        "program_change" => Midi1Message::ProgramChange {
            channel,
            program: number,
        },
        other => {
            return Err(EndpointError::Rejected(format!(
                "unknown MIDI kind `{other}`"
            )))
        }
    })
}

/// Builds the MIDI 2.0 channel-voice message for any kind, validating the field widths MIDI 2.0
/// imposes: velocity is 16-bit, every other data field is the full 32 bits.
fn midi2_message(
    group: u8,
    channel: u8,
    kind: &str,
    number: u8,
    value: u32,
    index: u8,
) -> std::result::Result<Midi2ChannelVoice, EndpointError> {
    use Midi2ChannelVoice as V;

    let velocity = |value: u32| {
        u16::try_from(value).map_err(|_| {
            EndpointError::Rejected(format!(
                "`{kind}` velocity must fit in 16 bits, got {value}"
            ))
        })
    };

    Ok(match kind {
        "note_on" => V::NoteOn {
            group,
            channel,
            note: number,
            attribute_type: 0,
            attribute: 0,
            velocity: velocity(value)?,
        },
        "note_off" => V::NoteOff {
            group,
            channel,
            note: number,
            attribute_type: 0,
            attribute: 0,
            velocity: velocity(value)?,
        },
        "cc" => V::ControlChange {
            group,
            channel,
            index: number,
            value,
        },
        "program_change" => V::ProgramChange {
            group,
            channel,
            option_flags: 0,
            program: number,
            bank_valid: false,
            bank_msb: 0,
            bank_lsb: 0,
        },
        "per_note_rcc" => V::PerNoteRcc {
            group,
            channel,
            note: number,
            index,
            value,
        },
        "per_note_acc" => V::PerNoteAcc {
            group,
            channel,
            note: number,
            index,
            value,
        },
        "rpn" => V::Rpn {
            group,
            channel,
            bank: index,
            index: number,
            value,
        },
        "nrpn" => V::Nrpn {
            group,
            channel,
            bank: index,
            index: number,
            value,
        },
        "relative_rpn" => V::RelativeRpn {
            group,
            channel,
            bank: index,
            index: number,
            value,
        },
        "relative_nrpn" => V::RelativeNrpn {
            group,
            channel,
            bank: index,
            index: number,
            value,
        },
        "per_note_pitch_bend" => V::PerNotePitchBend {
            group,
            channel,
            note: number,
            value,
        },
        "polyphonic_pressure" => V::PolyphonicKeyPressure {
            group,
            channel,
            note: number,
            pressure: value,
        },
        "channel_pressure" => V::ChannelPressure {
            group,
            channel,
            pressure: value,
        },
        "pitch_bend" => V::PitchBend {
            group,
            channel,
            value,
        },
        other => {
            return Err(EndpointError::Rejected(format!(
                "`{other}` is not a MIDI 2.0 channel-voice kind"
            )))
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::UdpSocket;
    use std::time::Duration;

    fn listener() -> (UdpSocket, String) {
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        let address = socket.local_addr().unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        (socket, address.to_string())
    }

    fn midi(
        channel: u8,
        kind: &str,
        number: u8,
        value: u16,
        group: Option<u8>,
        value32: Option<u32>,
    ) -> Command {
        Command::Midi {
            channel,
            kind: kind.into(),
            number,
            value,
            group,
            value32,
            index: None,
        }
    }

    fn with_index(mut command: Command, index: u8) -> Command {
        if let Command::Midi { index: slot, .. } = &mut command {
            *slot = Some(index);
        }
        command
    }

    #[test]
    fn a_midi1_note_arrives_as_a_type_2_ump() {
        let bytes = UmpEndpoint::encode(&midi(1, "note_on", 60, 100, Some(2), None)).unwrap();
        assert_eq!(
            bytes.len(),
            4,
            "a MIDI 1.0 message inside a UMP is one 32-bit packet"
        );
        let packet = Ump::from_bytes(&bytes).unwrap();
        assert_eq!(packet.message_type(), 0x2, "MIDI 1.0 carried in a UMP");
        assert_eq!(packet.group(), 2);
        assert_eq!(
            packet.parse().unwrap(),
            Midi2Message::Midi1ChannelVoice(Midi1ChannelVoice {
                group: 2,
                message: Midi1Message::NoteOn {
                    channel: 1,
                    note: 60,
                    velocity: 100,
                },
            })
        );
    }

    #[test]
    fn a_midi2_control_change_keeps_the_full_32_bit_value() {
        let bytes = UmpEndpoint::encode(&midi(0, "cc", 7, 0, Some(1), Some(0x1234_5678))).unwrap();
        let packet = Ump::from_bytes(&bytes).unwrap();
        assert_eq!(packet.message_type(), 0x4, "MIDI 2.0 channel voice");
        assert_eq!(packet.group(), 1);
        assert_eq!(
            packet.parse().unwrap(),
            Midi2Message::Midi2ChannelVoice(Midi2ChannelVoice::ControlChange {
                group: 1,
                channel: 0,
                index: 7,
                value: 0x1234_5678,
            })
        );
    }

    #[test]
    fn every_midi2_only_kind_encodes() {
        for kind in MIDI2_ONLY_KINDS {
            // Velocity is 16-bit in MIDI 2.0; every other data field is the full 32 bits.
            let value = Some(if matches!(kind, "note_on" | "note_off") {
                0x8000
            } else {
                0xFFFF_FFFF
            });
            let command = with_index(midi(0, kind, 60, 0, Some(0), value), 3);
            let bytes = UmpEndpoint::encode(&command).unwrap_or_else(|e| panic!("{kind}: {e}"));
            assert_eq!(bytes.len(), 8, "{kind} is one 64-bit packet");
            let packet = Ump::from_bytes(&bytes).unwrap();
            assert_eq!(packet.message_type(), 0x4, "{kind} is MIDI 2.0");
            assert_eq!(packet.group(), 0, "{kind} carries its group");
            let Midi2Message::Midi2ChannelVoice(cv) = packet.parse().unwrap() else {
                panic!("{kind} must decode as MIDI 2.0");
            };
            assert_eq!(cv.channel(), 0, "{kind} carries its channel");
        }
    }

    #[test]
    fn an_indexed_kind_reaches_the_wire() {
        let command = with_index(midi(0, "rpn", 5, 0, Some(0), Some(42)), 9);
        let bytes = UmpEndpoint::encode(&command).unwrap();
        let Midi2Message::Midi2ChannelVoice(cv) = Ump::from_bytes(&bytes).unwrap().parse().unwrap()
        else {
            panic!("expected a MIDI 2.0 message");
        };
        assert_eq!(
            cv,
            Midi2ChannelVoice::Rpn {
                group: 0,
                channel: 0,
                bank: 9,
                index: 5,
                value: 42,
            },
            "index becomes the bank, number becomes the parameter"
        );
    }

    #[test]
    fn a_midi2_kind_without_a_32_bit_value_is_rejected() {
        for kind in MIDI2_ONLY_KINDS {
            let error = UmpEndpoint::encode(&midi(0, kind, 60, 64, Some(0), None)).unwrap_err();
            assert!(
                error.to_string().contains("value32"),
                "{kind} must be rejected: {error}"
            );
        }
    }

    #[test]
    fn a_midi1_kind_with_a_32_bit_value_becomes_midi2() {
        let bytes = UmpEndpoint::encode(&midi(0, "note_on", 60, 0, Some(0), Some(0x8000))).unwrap();
        assert_eq!(
            Ump::from_bytes(&bytes).unwrap().message_type(),
            0x4,
            "a widened note-on is a MIDI 2.0 message, not a truncated MIDI 1.0 one"
        );
    }

    #[test]
    fn out_of_range_fields_are_rejected() {
        for (command, expected) in [
            (midi(16, "cc", 7, 1, None, None), "channel"),
            (midi(0, "cc", 128, 1, None, None), "number"),
            (midi(0, "cc", 7, 128, None, None), "value"),
            (midi(0, "sysex", 7, 1, None, None), "unknown MIDI kind"),
            (midi(0, "cc", 7, 1, Some(16), None), "group"),
        ] {
            let error = UmpEndpoint::encode(&command).unwrap_err();
            assert!(
                error.to_string().contains(expected),
                "expected `{expected}` in {error}"
            );
        }
    }

    #[test]
    fn an_index_out_of_range_is_rejected_only_for_indexed_kinds() {
        let error = UmpEndpoint::encode(&with_index(midi(0, "rpn", 5, 0, Some(0), Some(1)), 200))
            .unwrap_err();
        assert!(error.to_string().contains("index"), "{error}");
        // `cc` carries no index, so a stray one is inert rather than fatal.
        assert!(UmpEndpoint::encode(&with_index(midi(0, "cc", 7, 1, None, None), 200)).is_ok());
    }

    #[test]
    fn a_velocity_too_wide_for_16_bits_is_rejected() {
        let error =
            UmpEndpoint::encode(&midi(0, "note_on", 60, 0, Some(0), Some(0x1_0000))).unwrap_err();
        assert!(error.to_string().contains("16 bits"), "{error}");
    }

    #[test]
    fn a_non_midi_command_is_rejected() {
        let command = Command::Osc {
            address: "/a".into(),
            args: vec![],
        };
        assert!(UmpEndpoint::encode(&command).is_err());
    }

    #[test]
    fn a_packet_reaches_the_gateway_and_decodes_back_to_the_action() {
        let (socket, address) = listener();
        let endpoint = UmpEndpoint::new(&address).unwrap();
        endpoint
            .send(&midi(3, "cc", 74, 127, Some(5), Some(0xDEAD_BEEF)))
            .unwrap();

        let mut buf = [0u8; 16];
        let n = socket.recv(&mut buf).unwrap();
        let packet = Ump::from_bytes(&buf[..n]).unwrap();
        assert_eq!(packet.group(), 5);
        let Midi2Message::Midi2ChannelVoice(cv) = packet.parse().unwrap() else {
            panic!("expected a MIDI 2.0 channel-voice message");
        };
        assert_eq!(cv.channel(), 3);
        assert_eq!(cv.opcode(), 0xB, "control change");
    }

    #[test]
    fn an_endpoint_group_is_applied_when_the_action_omits_one() {
        let (socket, address) = listener();
        let endpoint = UmpEndpoint::new(&address).unwrap().with_group(9).unwrap();
        endpoint
            .send(&midi(0, "note_on", 60, 100, None, None))
            .unwrap();

        let mut buf = [0u8; 16];
        let n = socket.recv(&mut buf).unwrap();
        assert_eq!(Ump::from_bytes(&buf[..n]).unwrap().group(), 9);
    }

    #[test]
    fn an_action_group_wins_over_the_endpoint_group() {
        let (socket, address) = listener();
        let endpoint = UmpEndpoint::new(&address).unwrap().with_group(9).unwrap();
        endpoint
            .send(&midi(0, "note_on", 60, 100, Some(2), None))
            .unwrap();

        let mut buf = [0u8; 16];
        let n = socket.recv(&mut buf).unwrap();
        assert_eq!(Ump::from_bytes(&buf[..n]).unwrap().group(), 2);
    }

    #[test]
    fn an_endpoint_group_out_of_range_is_rejected() {
        assert!(UmpEndpoint::new("127.0.0.1:9")
            .unwrap()
            .with_group(16)
            .is_err());
    }

    #[test]
    fn an_unusable_address_is_an_error_not_a_panic() {
        assert!(UmpEndpoint::new("").is_err());
        assert!(UmpEndpoint::new("not-a-host").is_err());
    }

    #[test]
    fn pinging_a_connected_socket_succeeds() {
        let endpoint = UmpEndpoint::new("127.0.0.1:9").unwrap();
        assert!(endpoint.ping().is_ok());
    }

    #[test]
    fn the_kind_helpers_agree_with_the_model() {
        assert_eq!(all_kinds().count(), MidiMessageKind::ALL.len());
        assert!(is_midi2_only("pitch_bend"));
        assert!(!is_midi2_only("cc"));
        assert!(!is_midi2_only("not-a-kind"));
    }
}
