//! Inbound control-message validation, normalization and rate limiting (spec §16).
//!
//! Everything arriving over OSC, MIDI, Art-Net or sACN is treated as hostile. Parsing is delegated
//! to the `tpt-av-control` codecs, which are bounds-checked; this layer adds size limits, event
//! count limits, bounded state and per-source rate limiting, and guarantees that no input can make
//! the functions here panic (they return [`Error::MalformedMessage`] instead).

use std::collections::BTreeMap;
use std::sync::Mutex;

use tpt_app_av_automation_core::{
    BackoffPolicy, DmxLevel, Error, Event, MidiLevel, RateLimiter, Result, Timestamp,
};
use tpt_av_control_dmx::artnet::{parse_packet as parse_artnet_packet, ArtNetPacket};
use tpt_av_control_dmx::sacn::parse_packet as parse_sacn_packet;
use tpt_av_control_midi::{parse_midi1, Midi1Message, Midi2ChannelVoice, Midi2Message, Ump};
use tpt_av_control_osc::bundle::{parse_packet as parse_osc_packet, OscPacket};

/// Hard limits applied to inbound traffic.
#[derive(Debug, Clone, Copy)]
pub struct InboundLimits {
    /// Largest datagram accepted, in bytes.
    pub max_packet_bytes: usize,
    /// Most events one datagram may expand to (bundles, DMX frames).
    pub max_events_per_packet: usize,
    /// Most distinct DMX universes tracked, bounding memory under hostile traffic.
    pub max_universes: usize,
    /// Events allowed per source per window.
    pub rate_capacity: u32,
    /// Window over which the rate budget refills, in milliseconds.
    pub rate_window_ms: u64,
    /// Consecutive over-budget drops tolerated before the source is muted.
    pub backoff_threshold: u32,
    /// First mute window, in milliseconds; it doubles on each further backoff.
    pub backoff_initial_ms: u64,
    /// Longest mute, in milliseconds.
    pub backoff_max_ms: u64,
}

impl Default for InboundLimits {
    fn default() -> Self {
        Self {
            max_packet_bytes: 4096,
            max_events_per_packet: 1024,
            max_universes: 256,
            rate_capacity: 1000,
            rate_window_ms: 1000,
            backoff_threshold: BackoffPolicy::default().threshold,
            backoff_initial_ms: BackoffPolicy::default().initial_millis,
            backoff_max_ms: BackoffPolicy::default().max_millis,
        }
    }
}

impl InboundLimits {
    /// The adaptive-backoff policy implied by these limits.
    pub fn backoff_policy(&self) -> BackoffPolicy {
        BackoffPolicy {
            threshold: self.backoff_threshold,
            initial_millis: self.backoff_initial_ms,
            max_millis: self.backoff_max_ms,
        }
    }
}

/// Remembers the last frame per universe so DMX traffic can be reduced to channel changes.
#[derive(Debug, Default)]
pub struct DmxTracker {
    last: BTreeMap<u16, Vec<u8>>,
}

impl DmxTracker {
    /// Diffs a frame against the previous one. A universe seen for the first time is compared with
    /// an all-zero baseline, so only non-zero channels are reported.
    fn diff(
        &mut self,
        universe: u16,
        slots: &[u8],
        max_universes: usize,
        max_events: usize,
    ) -> Result<Vec<Event>> {
        if !self.last.contains_key(&universe) && self.last.len() >= max_universes {
            return Err(Error::MalformedMessage(format!(
                "too many DMX universes (limit {max_universes})"
            )));
        }
        let slots = &slots[..slots.len().min(512)];
        let previous = self.last.entry(universe).or_default();
        let mut events = Vec::new();
        for (channel, value) in slots.iter().enumerate() {
            let before = previous.get(channel).copied().unwrap_or(0);
            if before != *value {
                if events.len() >= max_events {
                    break;
                }
                events.push(Event::Dmx(DmxLevel {
                    universe,
                    channel: channel as u16,
                    value: *value,
                }));
            }
        }
        *previous = slots.to_vec();
        Ok(events)
    }
}

/// The validating, rate-limited front door for raw control traffic.
#[derive(Debug)]
pub struct Inbound {
    limits: InboundLimits,
    limiter: RateLimiter,
    dmx: Mutex<DmxTracker>,
}

impl Default for Inbound {
    fn default() -> Self {
        Self::new(InboundLimits::default())
    }
}

impl Inbound {
    /// Creates a gate with explicit limits.
    pub fn new(limits: InboundLimits) -> Self {
        Self {
            limiter: RateLimiter::with_backoff(
                limits.rate_capacity,
                limits.rate_window_ms,
                limits.backoff_policy(),
            ),
            limits,
            dmx: Mutex::new(DmxTracker::default()),
        }
    }

    /// Datagrams dropped because their source exceeded the rate budget or was muted.
    pub fn rate_limited(&self) -> u64 {
        self.limiter.dropped()
    }

    /// Datagrams rejected because their source was already in an adaptive-backoff mute.
    pub fn backed_off(&self) -> u64 {
        self.limiter.muted()
    }

    /// Number of backoff episodes, i.e. distinct times a source was muted.
    pub fn backoff_episodes(&self) -> u64 {
        self.limiter.backoffs()
    }

    /// How much longer `source` stays muted at `now`, in milliseconds.
    pub fn muted_for_ms(&self, source: &str, now: Timestamp) -> u64 {
        self.limiter.muted_for_ms(source, now)
    }

    fn admit(&self, source: &str, bytes: &[u8], now: Timestamp) -> Result<bool> {
        if bytes.is_empty() {
            return Err(Error::MalformedMessage("empty datagram".into()));
        }
        if bytes.len() > self.limits.max_packet_bytes {
            return Err(Error::MalformedMessage(format!(
                "datagram of {} bytes exceeds the {} byte limit",
                bytes.len(),
                self.limits.max_packet_bytes
            )));
        }
        Ok(self.limiter.try_acquire(source, now))
    }

    /// Parses an OSC datagram (message or bundle) into events.
    ///
    /// Returns an empty list when the source is over its rate budget; the drop is counted in
    /// [`Inbound::rate_limited`].
    pub fn osc(&self, source: &str, bytes: &[u8], now: Timestamp) -> Result<Vec<Event>> {
        if !self.admit(source, bytes, now)? {
            return Ok(Vec::new());
        }
        let packet = parse_osc_packet(bytes)
            .map_err(|e| Error::MalformedMessage(format!("osc: {e:?}")))?;
        let mut events = Vec::new();
        flatten_osc(&packet, &mut events, self.limits.max_events_per_packet)?;
        Ok(events)
    }

    /// Parses a raw MIDI 1.0 message into events. Non-trigger messages (clock, sysex, ...) yield none.
    /// Parses a MIDI 1.0 message into events. Non-trigger messages (clock, sysex, ...) yield none.
    pub fn midi(&self, source: &str, bytes: &[u8], now: Timestamp) -> Result<Vec<Event>> {
        if !self.admit(source, bytes, now)? {
            return Ok(Vec::new());
        }
        let message =
            parse_midi1(bytes).map_err(|e| Error::MalformedMessage(format!("midi: {e:?}")))?;
        Ok(midi1_level(&message).map_or_else(Vec::new, into_event))
    }

    /// Parses a MIDI 2.0 Universal MIDI Packet (UMP) into events (spec §7.2).
    ///
    /// A UMP packet is 4, 8 or 16 bytes; several packets may be concatenated in one datagram, as the
    /// MIDI 2.0 spec permits for SysEx streams, so every packet in the buffer is decoded in order.
    /// Utility, system common, SysEx, data and flex-data packets are accepted and produce no trigger
    /// events; MIDI 1.0 and MIDI 2.0 channel-voice messages normalize to the same [`MidiLevel`]
    /// shape, so one rule can match either protocol version.
    pub fn ump(&self, source: &str, bytes: &[u8], now: Timestamp) -> Result<Vec<Event>> {
        if !self.admit(source, bytes, now)? {
            return Ok(Vec::new());
        }
        if bytes.len() % 4 != 0 {
            return Err(Error::MalformedMessage(format!(
                "ump: byte length {} is not a multiple of 4",
                bytes.len()
            )));
        }
        let mut events = Vec::new();
        let mut offset = 0usize;
        while offset < bytes.len() {
            // The first word carries the message type, which is what determines the packet length.
            let header = u32::from_be_bytes([
                bytes[offset],
                bytes[offset + 1],
                bytes[offset + 2],
                bytes[offset + 3],
            ]);
            let message_type = (header >> 28) as u8;
            let size = match message_type {
                0x0..=0x2 => 4,
                0x3 | 0x4 | 0xD => 8,
                0x5 | 0xE | 0xF => 16,
                other => {
                    return Err(Error::MalformedMessage(format!(
                        "ump: reserved message type 0x{other:X}"
                    )))
                }
            };
            let end = offset + size;
            if end > bytes.len() {
                return Err(Error::MalformedMessage(format!(
                    "ump: truncated packet: message type 0x{message_type:X} needs {size} bytes, {} remain",
                    bytes.len() - offset
                )));
            }
            let packet = Ump::from_bytes(&bytes[offset..end])
                .map_err(|e| Error::MalformedMessage(format!("ump: {e}")))?;
            let message = packet
                .parse()
                .map_err(|e| Error::MalformedMessage(format!("ump: {e}")))?;
            offset = end;
            if let Some(level) = ump_level(&message) {
                if events.len() >= self.limits.max_events_per_packet {
                    break;
                }
                events.push(Event::Midi(level));
            }
        }
        Ok(events)
    }

    /// Parses an Art-Net datagram into DMX channel-change events.
    pub fn artnet(&self, source: &str, bytes: &[u8], now: Timestamp) -> Result<Vec<Event>> {
        if !self.admit(source, bytes, now)? {
            return Ok(Vec::new());
        }
        match parse_artnet_packet(bytes)
            .map_err(|e| Error::MalformedMessage(format!("artnet: {e:?}")))?
        {
            ArtNetPacket::Dmx { universe, slots } => self.track_dmx(universe, &slots),
            _ => Ok(Vec::new()),
        }
    }

    /// Parses an sACN datagram into DMX channel-change events.
    pub fn sacn(&self, source: &str, bytes: &[u8], now: Timestamp) -> Result<Vec<Event>> {
        if !self.admit(source, bytes, now)? {
            return Ok(Vec::new());
        }
        match parse_sacn_packet(bytes)
            .map_err(|e| Error::MalformedMessage(format!("sacn: {e:?}")))?
        {
            Some(packet) => self.track_dmx(packet.universe, &packet.slots),
            None => Ok(Vec::new()),
        }
    }

    fn track_dmx(&self, universe: u16, slots: &[u8]) -> Result<Vec<Event>> {
        let mut tracker = self.dmx.lock().unwrap_or_else(|e| e.into_inner());
        tracker.diff(
            universe,
            slots,
            self.limits.max_universes,
            self.limits.max_events_per_packet,
        )
    }
}

/// Wraps one normalized observation in an event.
fn into_event(level: MidiLevel) -> Vec<Event> {
    vec![Event::Midi(level)]
}

/// Normalizes a MIDI 1.0 channel-voice message; non-trigger messages return `None`.
fn midi1_level(message: &Midi1Message) -> Option<MidiLevel> {
    let level = match *message {
        Midi1Message::NoteOn {
            channel,
            note,
            velocity,
        } => MidiLevel::midi1(channel, "note_on", note, u16::from(velocity)),
        Midi1Message::NoteOff {
            channel,
            note,
            velocity,
        } => MidiLevel::midi1(channel, "note_off", note, u16::from(velocity)),
        Midi1Message::ControlChange {
            channel,
            controller,
            value,
        } => MidiLevel::midi1(channel, "cc", controller, u16::from(value)),
        Midi1Message::ProgramChange { channel, program } => {
            MidiLevel::midi1(channel, "program_change", program, 0)
        }
        // Clock, sysex, aftertouch and the rest are not control triggers.
        _ => return None,
    };
    Some(level)
}

/// Normalizes a UMP message; non-channel-voice messages return `None`.
///
/// MIDI 2.0 data is 32-bit, so it is reported in both `value` (the high 16 bits, the resolution a
/// MIDI 1.0 rule can compare against) and `value32` (the full value).
fn ump_level(message: &Midi2Message) -> Option<MidiLevel> {
    match message {
        // A MIDI 1.0 message inside a UMP keeps 1.0 semantics, but still reports its group.
        Midi2Message::Midi1ChannelVoice(cv) => {
            let mut level = midi1_level(&cv.message)?;
            level.group = Some(cv.group);
            Some(level)
        }
        Midi2Message::Midi2ChannelVoice(cv) => Some(midi2_level(cv)),
        _ => None,
    }
}

/// Projects a full 32-bit MIDI 2.0 value onto the 16-bit field MIDI 1.0 rules compare against.
///
/// Quantities that MIDI 2.0 already defines as 16-bit (note velocity, for example) pass through
/// unchanged; only the genuinely 32-bit ones (CC value, pressure, pitch bend) are scaled.
fn narrow(value: u32) -> u16 {
    if value <= u32::from(u16::MAX) {
        value as u16
    } else {
        (value >> 16) as u16
    }
}

fn midi2_level(cv: &Midi2ChannelVoice) -> MidiLevel {
    use Midi2ChannelVoice as V;
    let (kind, channel, number, value32) = match cv {
        V::PerNoteRcc {
            channel,
            note,
            value,
            ..
        } => ("per_note_rcc", *channel, *note, *value),
        V::PerNoteAcc {
            channel,
            note,
            value,
            ..
        } => ("per_note_acc", *channel, *note, *value),
        V::Rpn {
            channel,
            bank,
            value,
            ..
        } => ("rpn", *channel, *bank, *value),
        V::Nrpn {
            channel,
            bank,
            value,
            ..
        } => ("nrpn", *channel, *bank, *value),
        V::RelativeRpn {
            channel,
            bank,
            value,
            ..
        } => ("relative_rpn", *channel, *bank, *value),
        V::RelativeNrpn {
            channel,
            bank,
            value,
            ..
        } => ("relative_nrpn", *channel, *bank, *value),
        V::PerNotePitchBend {
            channel,
            note,
            value,
            ..
        } => ("per_note_pitch_bend", *channel, *note, *value),
        V::NoteOff {
            channel,
            note,
            velocity,
            ..
        } => ("note_off", *channel, *note, u32::from(*velocity)),
        V::NoteOn {
            channel,
            note,
            velocity,
            ..
        } => ("note_on", *channel, *note, u32::from(*velocity)),
        V::PolyphonicKeyPressure {
            channel,
            note,
            pressure,
            ..
        } => ("polyphonic_pressure", *channel, *note, *pressure),
        V::ControlChange {
            channel,
            index,
            value,
            ..
        } => ("cc", *channel, *index, *value),
        V::ProgramChange {
            channel,
            bank_msb,
            bank_lsb,
            program,
            ..
        } => (
            "program_change",
            *channel,
            *program,
            (u32::from(*bank_msb) << 16) | (u32::from(*bank_lsb) << 8) | u32::from(*program),
        ),
        V::ChannelPressure {
            channel,
            pressure,
            ..
        } => ("channel_pressure", *channel, 0, *pressure),
        V::PitchBend { channel, value, .. } => ("pitch_bend", *channel, 0, *value),
        V::PerNoteManagement { channel, note, .. } => ("per_note_management", *channel, *note, 0),
    };
    MidiLevel {
        channel,
        kind: kind.to_string(),
        number,
        value: narrow(value32),
        value32: Some(value32),
        group: Some(cv.group()),
    }
}

fn flatten_osc(packet: &OscPacket, out: &mut Vec<Event>, limit: usize) -> Result<()> {
    match packet {
        OscPacket::Message(message) => {
            if out.len() >= limit {
                return Err(Error::MalformedMessage("osc bundle expands to too many messages".into()));
            }
            // Only numeric arguments are exposed to rules; strings, blobs and the like are skipped.
            let args = message
                .arguments
                .iter()
                .filter_map(|a| a.as_f32())
                .map(f64::from)
                .collect();
            out.push(Event::Osc {
                address: message.address.clone(),
                args,
            });
        }
        OscPacket::Bundle(bundle) => {
            for element in &bundle.elements {
                flatten_osc(element, out, limit)?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tpt_av_control_dmx::artnet::build_artdmx;
    use tpt_av_control_dmx::sacn::{build_data_packet, Cid};
    use tpt_av_control_midi::{Midi1ChannelVoice, SystemCommonMessage, UtilityMessage};
    use tpt_av_control_osc::{OscArg, OscBundle, OscMessage};

    fn now() -> Timestamp {
        Timestamp::from_millis(0)
    }

    fn osc_bytes(address: &str, args: &[OscArg]) -> Vec<u8> {
        OscMessage::new(address, args).unwrap().encode()
    }

    #[test]
    fn osc_message_becomes_numeric_event() {
        let gate = Inbound::default();
        let events = gate
            .osc("a", &osc_bytes("/cue/1", &[OscArg::Int(1), OscArg::Float(0.5)]), now())
            .unwrap();
        assert_eq!(
            events,
            vec![Event::Osc {
                address: "/cue/1".into(),
                args: vec![1.0, 0.5]
            }]
        );
    }

    #[test]
    fn osc_non_numeric_arguments_are_skipped() {
        let gate = Inbound::default();
        let events = gate
            .osc("a", &osc_bytes("/x", &[OscArg::String("go".into()), OscArg::Int(2)]), now())
            .unwrap();
        assert_eq!(
            events,
            vec![Event::Osc {
                address: "/x".into(),
                args: vec![2.0]
            }]
        );
    }

    #[test]
    fn osc_bundles_are_flattened_in_order() {
        let gate = Inbound::default();
        let bundle = OscBundle::new(
            None,
            vec![
                OscPacket::Message(OscMessage::new("/a", &[]).unwrap()),
                OscPacket::Message(OscMessage::new("/b", &[]).unwrap()),
            ],
        );
        let events = gate.osc("a", &bundle.encode(), now()).unwrap();
        let addresses: Vec<_> = events
            .iter()
            .map(|e| match e {
                Event::Osc { address, .. } => address.as_str(),
                _ => "",
            })
            .collect();
        assert_eq!(addresses, ["/a", "/b"]);
    }

    #[test]
    fn malformed_and_oversized_traffic_is_rejected_not_fatal() {
        let gate = Inbound::default();
        for bad in [&b""[..], b"\x00", b"not osc at all", b"#bundle\0\xff\xff\xff\xff"] {
            assert!(gate.osc("a", bad, now()).is_err(), "{bad:?}");
        }
        let huge = vec![b'/'; 10_000];
        assert!(matches!(
            gate.osc("a", &huge, now()),
            Err(Error::MalformedMessage(_))
        ));
        assert!(gate.midi("a", b"\xff\xff", now()).is_ok() || gate.midi("a", b"\xff\xff", now()).is_err());
        assert!(gate.artnet("a", b"Art-Net\0\x00\x50", now()).is_err());
        assert!(gate.sacn("a", &[0u8; 40], now()).is_err());
    }

    #[test]
    fn arbitrary_bytes_never_panic() {
        // A cheap deterministic stand-in for fuzzing; the real corpus runs in the chaos suite.
        let gate = Inbound::new(InboundLimits {
            rate_capacity: u32::MAX,
            ..InboundLimits::default()
        });
        let mut x: u32 = 0x1234_5678;
        for len in 0..200usize {
            let data: Vec<u8> = (0..len)
                .map(|_| {
                    x ^= x << 13;
                    x ^= x >> 17;
                    x ^= x << 5;
                    x as u8
                })
                .collect();
            let _ = gate.osc("f", &data, now());
            let _ = gate.midi("f", &data, now());
            let _ = gate.artnet("f", &data, now());
            let _ = gate.sacn("f", &data, now());
        }
    }

    /// Seeded mutation fuzzing (spec §18.5): corrupt *valid* packets, which reach deeper into the
    /// parsers than random bytes do. A coverage-guided fuzzer is tracked separately in `todo.md`.
    #[test]
    fn mutated_valid_packets_never_panic() {
        let gate = Inbound::new(InboundLimits {
            rate_capacity: u32::MAX,
            max_universes: usize::MAX,
            ..InboundLimits::default()
        });
        let mut slots = [0u8; 512];
        slots[7] = 99;
        let bundle = OscBundle::new(
            Some(5),
            vec![
                OscPacket::Message(
                    OscMessage::new("/a/b", &[OscArg::Int(1), OscArg::Float(2.5), OscArg::String("x".into())]).unwrap(),
                ),
                OscPacket::Bundle(OscBundle::new(None, vec![OscPacket::Message(OscMessage::new("/c", &[]).unwrap())])),
            ],
        )
        .encode();
        let seeds: Vec<Vec<u8>> = vec![
            osc_bytes("/cue/1", &[OscArg::Int(1), OscArg::Blob(vec![1, 2, 3])]),
            bundle,
            vec![0x93, 60, 100],
            vec![0xB0, 7, 64],
            vec![0xF0, 1, 2, 3, 0xF7],
            build_artdmx(3, 1, &slots),
            build_data_packet(&Cid::from_bytes(b"seed"), "seed", 5, 100, 1, &slots),
        ];
        let mut x: u64 = 0x1234_5678_9ABC_DEF1;
        let mut next = move || {
            x ^= x >> 12;
            x ^= x << 25;
            x ^= x >> 27;
            x.wrapping_mul(0x2545_F491_4F6C_DD1D)
        };
        for round in 0..6000usize {
            let mut data = seeds[round % seeds.len()].clone();
            for _ in 0..=(next() % 4) {
                if data.is_empty() {
                    break;
                }
                let i = (next() as usize) % data.len();
                match next() % 4 {
                    0 => data[i] ^= 1 << (next() % 8),
                    1 => data[i] = next() as u8,
                    2 => data.truncate(i),
                    _ => data.insert(i, next() as u8),
                }
            }
            let _ = gate.osc("fuzz", &data, now());
            let _ = gate.midi("fuzz", &data, now());
            let _ = gate.artnet("fuzz", &data, now());
            let _ = gate.sacn("fuzz", &data, now());
        }
    }

    #[test]
    fn midi_messages_normalize() {
        let gate = Inbound::default();
        assert_eq!(
            gate.midi("m", &[0x93, 60, 100], now()).unwrap(),
            vec![Event::Midi(MidiLevel {
                channel: 3,
                kind: "note_on".into(),
                number: 60,
                value: 100,
                ..Default::default()
            })]
        );
        assert_eq!(
            gate.midi("m", &[0xB0, 7, 64], now()).unwrap(),
            vec![Event::Midi(MidiLevel {
                channel: 0,
                kind: "cc".into(),
                number: 7,
                value: 64,
                ..Default::default()
            })]
        );
        assert_eq!(
            gate.midi("m", &[0xC2, 9], now()).unwrap(),
            vec![Event::Midi(MidiLevel {
                channel: 2,
                kind: "program_change".into(),
                number: 9,
                value: 0,
                ..Default::default()
            })]
        );
        assert!(gate.midi("m", &[0xF8], now()).unwrap().is_empty(), "clock is not a trigger");
    }

    #[test]
    fn artnet_reports_only_changed_channels() {
        let gate = Inbound::default();
        let mut frame = [0u8; 512];
        frame[4] = 200;
        let first = gate.artnet("n", &build_artdmx(1, 0, &frame), now()).unwrap();
        assert_eq!(
            first,
            vec![Event::Dmx(DmxLevel {
                universe: 1,
                channel: 4,
                value: 200
            })]
        );
        assert!(gate.artnet("n", &build_artdmx(1, 1, &frame), now()).unwrap().is_empty());
        frame[4] = 10;
        frame[5] = 1;
        assert_eq!(gate.artnet("n", &build_artdmx(1, 2, &frame), now()).unwrap().len(), 2);
    }

    #[test]
    fn sacn_frames_normalize_to_dmx_events() {
        let gate = Inbound::default();
        let mut slots = [0u8; 512];
        slots[0] = 255;
        let packet = build_data_packet(&Cid::from_bytes(b"t"), "t", 3, 100, 0, &slots);
        let events = gate.sacn("s", &packet, now()).unwrap();
        assert_eq!(
            events,
            vec![Event::Dmx(DmxLevel {
                universe: 3,
                channel: 0,
                value: 255
            })]
        );
    }

    #[test]
    fn universe_tracking_is_bounded() {
        let gate = Inbound::new(InboundLimits {
            max_universes: 2,
            ..InboundLimits::default()
        });
        let frame = [1u8; 512];
        assert!(gate.artnet("n", &build_artdmx(1, 0, &frame), now()).is_ok());
        assert!(gate.artnet("n", &build_artdmx(2, 0, &frame), now()).is_ok());
        assert!(gate.artnet("n", &build_artdmx(3, 0, &frame), now()).is_err());
        assert!(gate.artnet("n", &build_artdmx(1, 1, &frame), now()).is_ok(), "known universes still work");
    }

    /// Builds a UMP packet from a MIDI 2.0 message.
    fn ump_bytes(message: &Midi2Message) -> Vec<u8> {
        Ump::from_message(message).to_bytes()
    }

    fn note_on_2(group: u8, channel: u8, note: u8, velocity: u16) -> Midi2Message {
        Midi2Message::Midi2ChannelVoice(Midi2ChannelVoice::NoteOn {
            group,
            channel,
            note,
            attribute_type: 0,
            attribute: 0,
            velocity,
        })
    }

    #[test]
    fn midi_2_channel_voice_messages_normalize() {
        let gate = Inbound::default();
        let events = gate
            .ump("m", &ump_bytes(&note_on_2(2, 5, 60, 0x8000)), now())
            .unwrap();
        assert_eq!(
            events,
            vec![Event::Midi(MidiLevel {
                channel: 5,
                kind: "note_on".into(),
                number: 60,
                value: 0x8000,
                value32: Some(0x8000),
                group: Some(2),
            })]
        );
    }

    #[test]
    fn a_full_32_bit_control_change_keeps_its_high_resolution_value() {
        let gate = Inbound::default();
        let bytes = ump_bytes(&Midi2Message::Midi2ChannelVoice(
            Midi2ChannelVoice::ControlChange {
                group: 0,
                channel: 1,
                index: 7,
                value: 0x1234_5678,
            },
        ));
        let events = gate.ump("m", &bytes, now()).unwrap();
        let Event::Midi(level) = &events[0] else {
            panic!("expected a MIDI event, got {events:?}");
        };
        assert_eq!(level.kind, "cc");
        assert_eq!(level.number, 7);
        assert_eq!(level.value32, Some(0x1234_5678), "full 32-bit value");
        assert_eq!(level.value, 0x1234, "narrowed for MIDI 1.0 rules");
        assert_eq!(level.effective_value(), 0x1234_5678);
    }

    #[test]
    fn a_midi1_message_inside_ump_keeps_its_own_value_but_reports_the_group() {
        let gate = Inbound::default();
        let bytes = ump_bytes(&Midi2Message::Midi1ChannelVoice(Midi1ChannelVoice {
            group: 3,
            message: Midi1Message::ControlChange {
                channel: 0,
                controller: 7,
                value: 100,
            },
        }));
        assert_eq!(
            gate.ump("m", &bytes, now()).unwrap(),
            vec![Event::Midi(MidiLevel {
                channel: 0,
                kind: "cc".into(),
                number: 7,
                value: 100,
                value32: None,
                group: Some(3),
            })]
        );
    }

    #[test]
    fn non_channel_voice_ump_packets_are_accepted_but_are_not_triggers() {
        let gate = Inbound::default();
        assert!(gate
            .ump(
                "m",
                &ump_bytes(&Midi2Message::Utility(UtilityMessage::Clock { clock: 1 })),
                now()
            )
            .unwrap()
            .is_empty());
        assert!(gate
            .ump(
                "m",
                &ump_bytes(&Midi2Message::SystemCommon(SystemCommonMessage::ActiveSensing {
                    group: 0,
                })),
                now()
            )
            .unwrap()
            .is_empty());
    }

    #[test]
    fn several_ump_packets_in_one_datagram_are_all_decoded() {
        let gate = Inbound::default();
        let mut buffer = ump_bytes(&note_on_2(0, 0, 60, 100));
        buffer.extend(ump_bytes(&Midi2Message::Midi2ChannelVoice(
            Midi2ChannelVoice::NoteOff {
                group: 0,
                channel: 0,
                note: 60,
                attribute_type: 0,
                attribute: 0,
                velocity: 0,
            },
        )));
        let events = gate.ump("m", &buffer, now()).unwrap();
        assert_eq!(events.len(), 2);
        assert!(matches!(&events[0], Event::Midi(l) if l.kind == "note_on"));
        assert!(matches!(&events[1], Event::Midi(l) if l.kind == "note_off"));
    }

    #[test]
    fn malformed_ump_is_rejected_not_fatal() {
        let gate = Inbound::default();
        // Not a multiple of four bytes.
        assert!(gate.ump("m", &[0x40, 0x90, 0x3C], now()).is_err());
        assert!(gate.ump("m", &[], now()).is_err());
        // A well-formed 32-bit utility packet is fine.
        assert!(gate.ump("m", &[0x00, 0x00, 0x00, 0x00], now()).is_ok());
        // A 64-bit header delivered in only four bytes.
        assert!(gate.ump("m", &[0x40, 0x90, 0x00, 0x00], now()).is_err());
    }

    #[test]
    fn arbitrary_ump_bytes_never_panic() {
        let gate = Inbound::new(InboundLimits {
            rate_capacity: u32::MAX,
            ..InboundLimits::default()
        });
        let mut x: u32 = 0x9E37_79B9;
        for len in 0..80usize {
            let data: Vec<u8> = (0..len)
                .map(|_| {
                    x ^= x << 13;
                    x ^= x >> 17;
                    x ^= x << 5;
                    x as u8
                })
                .collect();
            let _ = gate.ump("fuzz", &data, now());
        }
    }

    #[test]
    fn mutated_ump_packets_never_panic() {
        let gate = Inbound::new(InboundLimits {
            rate_capacity: u32::MAX,
            ..InboundLimits::default()
        });
        let seeds: Vec<Vec<u8>> = vec![
            ump_bytes(&note_on_2(0, 0, 60, 0x8000)),
            ump_bytes(&Midi2Message::Midi2ChannelVoice(
                Midi2ChannelVoice::ControlChange {
                    group: 1,
                    channel: 2,
                    index: 7,
                    value: 0xDEAD_BEEF,
                },
            )),
            ump_bytes(&Midi2Message::Utility(UtilityMessage::Timestamp {
                timestamp: 42,
            })),
        ];
        let mut x: u64 = 0x1234_5678_9ABC_DEF1;
        let mut next = move || {
            x ^= x >> 12;
            x ^= x << 25;
            x ^= x >> 27;
            x.wrapping_mul(0x2545_F491_4F6C_DD1D)
        };
        for round in 0..4000usize {
            let mut data = seeds[round % seeds.len()].clone();
            for _ in 0..=(next() % 3) {
                if data.is_empty() {
                    break;
                }
                let i = (next() as usize) % data.len();
                data[i] ^= 1 << (next() % 8);
            }
            let _ = gate.ump("fuzz", &data, now());
        }
    }

    #[test]
    fn floods_are_rate_limited_per_source_and_counted() {
        let gate = Inbound::new(InboundLimits {
            rate_capacity: 3,
            rate_window_ms: 1_000,
            // Backoff is covered separately; isolate the token bucket here.
            backoff_threshold: 0,
            ..InboundLimits::default()
        });
        let msg = osc_bytes("/flood", &[]);
        let delivered: usize = (0..10)
            .map(|_| gate.osc("noisy", &msg, now()).unwrap().len())
            .sum();
        assert_eq!(delivered, 3);
        assert_eq!(gate.rate_limited(), 7);
        assert_eq!(gate.osc("quiet", &msg, now()).unwrap().len(), 1, "other sources unaffected");
        // The budget refills with time.
        assert_eq!(
            gate.osc("noisy", &msg, Timestamp::from_millis(2_000)).unwrap().len(),
            1
        );
    }

    #[test]
    fn a_flooding_source_is_muted_by_adaptive_backoff() {
        let gate = Inbound::new(InboundLimits {
            rate_capacity: 1,
            rate_window_ms: 1,
            backoff_threshold: 2,
            backoff_initial_ms: 100,
            backoff_max_ms: 400,
            ..InboundLimits::default()
        });
        let msg = osc_bytes("/flood", &[]);
        assert_eq!(gate.osc("noisy", &msg, now()).unwrap().len(), 1);
        assert!(gate.osc("noisy", &msg, now()).unwrap().is_empty());
        // Second consecutive drop trips the threshold and mutes the source for 100 ms.
        assert!(gate.osc("noisy", &msg, now()).unwrap().is_empty());
        assert_eq!(gate.backoff_episodes(), 1);
        assert_eq!(gate.muted_for_ms("noisy", now()), 100);

        // Inside the mute the source is rejected before the packet is even parsed, and a
        // different source is unaffected.
        assert!(gate.osc("noisy", &msg, Timestamp::from_millis(50)).unwrap().is_empty());
        assert_eq!(gate.backed_off(), 1);
        assert_eq!(gate.osc("quiet", &msg, Timestamp::from_millis(50)).unwrap().len(), 1);

        // Once the mute expires the source is admitted again.
        assert_eq!(
            gate.osc("noisy", &msg, Timestamp::from_millis(100)).unwrap().len(),
            1
        );
        assert_eq!(gate.muted_for_ms("noisy", Timestamp::from_millis(100)), 0);
    }
}
