//! Network and MIDI endpoints built on the `tpt-av-control` codecs (spec §5.1, §8.1).

use std::collections::BTreeMap;
use std::net::UdpSocket;
use std::sync::Mutex;

use tpt_app_av_automation_core::{Error, Result};
use tpt_app_av_automation_model::DmxTransport;
use tpt_av_control_dmx::artnet::build_artdmx;
use tpt_av_control_dmx::sacn::{build_data_packet, Cid};
use tpt_av_control_midi::Midi1Message;
use tpt_av_control_osc::{OscArg, OscMessage};

use crate::command::Command;
use crate::config::{DeviceConfig, SceneDef};
use crate::dmx_serial::DmxSerialEndpoint;
use crate::endpoint::{Endpoint, EndpointError, VirtualEndpoint};

const DMX_CHANNELS: usize = 512;

/// Wire protocol of a [`UdpEndpoint`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UdpProtocol {
    /// Open Sound Control.
    Osc,
    /// Art-Net (ArtDmx).
    ArtNet,
    /// sACN / E1.31.
    Sacn,
}

struct DmxState {
    universes: BTreeMap<u16, [u8; DMX_CHANNELS]>,
    sequence: u8,
}

/// A UDP endpoint speaking OSC, Art-Net or sACN to one device.
pub struct UdpEndpoint {
    protocol: UdpProtocol,
    socket: UdpSocket,
    scenes: BTreeMap<String, SceneDef>,
    dmx: Mutex<DmxState>,
    cid: Cid,
}

impl std::fmt::Debug for UdpEndpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UdpEndpoint").field("protocol", &self.protocol).finish()
    }
}

impl UdpEndpoint {
    /// Opens an ephemeral local socket "connected" to `address` (`host:port`).
    pub fn new(
        protocol: UdpProtocol,
        address: &str,
        scenes: BTreeMap<String, SceneDef>,
    ) -> std::result::Result<Self, EndpointError> {
        let socket = UdpSocket::bind("0.0.0.0:0")
            .map_err(|e| EndpointError::Unreachable(format!("bind failed: {e}")))?;
        socket
            .connect(address)
            .map_err(|e| EndpointError::Unreachable(format!("`{address}`: {e}")))?;
        Ok(Self {
            protocol,
            socket,
            scenes,
            dmx: Mutex::new(DmxState {
                universes: BTreeMap::new(),
                sequence: 0,
            }),
            cid: Cid::from_bytes(b"tpt-av-automation"),
        })
    }

    fn transmit(&self, bytes: &[u8]) -> std::result::Result<(), EndpointError> {
        self.socket
            .send(bytes)
            .map(|_| ())
            .map_err(|e| EndpointError::Unreachable(e.to_string()))
    }

    fn write_dmx(
        &self,
        transport: Option<DmxTransport>,
        universe: u16,
        start: usize,
        values: &[u8],
        replace: bool,
    ) -> std::result::Result<(), EndpointError> {
        let expected = match self.protocol {
            UdpProtocol::ArtNet => DmxTransport::ArtNet,
            UdpProtocol::Sacn => DmxTransport::Sacn,
            UdpProtocol::Osc => {
                return Err(EndpointError::Rejected("OSC endpoints do not carry DMX".into()))
            }
        };
        if let Some(t) = transport {
            if t != expected {
                return Err(EndpointError::Rejected(format!(
                    "action transport {t:?} does not match this endpoint ({expected:?})"
                )));
            }
        }
        if start + values.len() > DMX_CHANNELS {
            return Err(EndpointError::Rejected("values run past channel 512".into()));
        }
        let (frame, sequence) = {
            let mut state = self.dmx.lock().unwrap_or_else(|e| e.into_inner());
            state.sequence = state.sequence.wrapping_add(1);
            let sequence = state.sequence;
            let slots = state.universes.entry(universe).or_insert([0u8; DMX_CHANNELS]);
            if replace {
                *slots = [0u8; DMX_CHANNELS];
            }
            slots[start..start + values.len()].copy_from_slice(values);
            (*slots, sequence)
        };
        let packet = match self.protocol {
            UdpProtocol::ArtNet => build_artdmx(universe, sequence, &frame),
            _ => build_data_packet(&self.cid, "TPT AV Automation", universe, 100, sequence, &frame),
        };
        self.transmit(&packet)
    }
}

/// OSC carries integral numbers as `i` and everything else as `f`, which is what most show-control
/// receivers expect for switches and faders respectively.
fn osc_arg(value: f64) -> std::result::Result<OscArg, EndpointError> {
    if !value.is_finite() {
        return Err(EndpointError::Rejected("OSC argument must be finite".into()));
    }
    if value.fract() == 0.0 && value.abs() <= f64::from(i32::MAX) {
        Ok(OscArg::Int(value as i32))
    } else {
        Ok(OscArg::Float(value as f32))
    }
}

impl Endpoint for UdpEndpoint {
    fn send(&self, command: &Command) -> std::result::Result<(), EndpointError> {
        match command {
            Command::Osc { address, args } => {
                if self.protocol != UdpProtocol::Osc {
                    return Err(EndpointError::Rejected("not an OSC endpoint".into()));
                }
                let args = args
                    .iter()
                    .map(|a| osc_arg(*a))
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                let message = OscMessage::new(address.clone(), &args)
                    .map_err(|e| EndpointError::Rejected(format!("{e:?}")))?;
                self.transmit(&message.encode())
            }
            Command::DmxChannels {
                universe,
                start_channel,
                values,
                transport,
            } => self.write_dmx(Some(*transport), *universe, usize::from(*start_channel), values, false),
            Command::DmxUniverse {
                universe,
                values,
                transport,
            } => self.write_dmx(Some(*transport), *universe, 0, values, true),
            Command::DmxScene { scene, .. } => {
                let def = self
                    .scenes
                    .get(scene)
                    .ok_or_else(|| EndpointError::Rejected(format!("unknown scene `{scene}`")))?;
                self.write_dmx(None, def.universe, usize::from(def.start_channel), &def.values, false)
            }
            other => Err(EndpointError::Rejected(format!(
                "`{}` is not supported by a {:?} endpoint",
                other.describe(),
                self.protocol
            ))),
        }
    }
}

/// Sink for encoded MIDI bytes, e.g. a `midir` output connection.
pub trait MidiWriter: Send {
    /// Writes one encoded MIDI message.
    fn write(&mut self, bytes: &[u8]) -> std::result::Result<(), String>;
}

/// Opens a [`MidiWriter`] on demand. Called again after a failure, so an unplugged-and-replugged
/// interface recovers without restarting the engine.
pub type MidiOpener = Box<dyn Fn() -> std::result::Result<Box<dyn MidiWriter>, String> + Send + Sync>;

/// An endpoint that encodes MIDI 1.0 messages and writes them to a [`MidiWriter`].
pub struct MidiEndpoint {
    writer: Mutex<Option<Box<dyn MidiWriter>>>,
    opener: Option<MidiOpener>,
}

impl MidiEndpoint {
    /// Wraps a writer.
    pub fn new(writer: Box<dyn MidiWriter>) -> Self {
        Self {
            writer: Mutex::new(Some(writer)),
            opener: None,
        }
    }

    /// An endpoint with no output attached; every send reports the device unreachable.
    pub fn detached() -> Self {
        Self {
            writer: Mutex::new(None),
            opener: None,
        }
    }

    /// An endpoint that connects lazily through `opener` and reconnects after a failure.
    pub fn with_opener(opener: MidiOpener) -> Self {
        Self {
            writer: Mutex::new(None),
            opener: Some(opener),
        }
    }

    /// Ensures a writer is attached, opening one if an opener is configured.
    fn attach<'a>(
        &self,
        guard: &'a mut Option<Box<dyn MidiWriter>>,
    ) -> std::result::Result<&'a mut Box<dyn MidiWriter>, EndpointError> {
        if guard.is_none() {
            match &self.opener {
                Some(open) => *guard = Some(open().map_err(EndpointError::Unreachable)?),
                None => return Err(EndpointError::Unreachable("no MIDI output attached".into())),
            }
        }
        Ok(guard.as_mut().expect("attached above"))
    }

    /// Encodes a command to MIDI 1.0 bytes, validating every range.
    pub fn encode(command: &Command) -> std::result::Result<Vec<u8>, EndpointError> {
        let Command::Midi {
            channel,
            kind,
            number,
            value,
        } = command
        else {
            return Err(EndpointError::Rejected("not a MIDI command".into()));
        };
        if *channel > 15 {
            return Err(EndpointError::Rejected(format!("MIDI channel {channel} out of range 0-15")));
        }
        if *number > 127 {
            return Err(EndpointError::Rejected(format!("MIDI number {number} out of range 0-127")));
        }
        let value = u8::try_from(*value)
            .ok()
            .filter(|v| *v <= 127)
            .ok_or_else(|| EndpointError::Rejected(format!("MIDI value {value} out of range 0-127")))?;
        let message = match kind.as_str() {
            "note_on" => Midi1Message::NoteOn {
                channel: *channel,
                note: *number,
                velocity: value,
            },
            "note_off" => Midi1Message::NoteOff {
                channel: *channel,
                note: *number,
                velocity: value,
            },
            "cc" => Midi1Message::ControlChange {
                channel: *channel,
                controller: *number,
                value,
            },
            "program_change" => Midi1Message::ProgramChange {
                channel: *channel,
                program: *number,
            },
            other => return Err(EndpointError::Rejected(format!("unknown MIDI kind `{other}`"))),
        };
        Ok(message.to_bytes())
    }
}

impl Endpoint for MidiEndpoint {
    fn send(&self, command: &Command) -> std::result::Result<(), EndpointError> {
        let bytes = Self::encode(command)?;
        let mut guard = self.writer.lock().unwrap_or_else(|e| e.into_inner());
        let result = self.attach(&mut guard)?.write(&bytes);
        if result.is_err() && self.opener.is_some() {
            // Drop the broken connection so the next send reopens the port.
            *guard = None;
        }
        result.map_err(EndpointError::Unreachable)
    }

    fn ping(&self) -> std::result::Result<(), EndpointError> {
        let mut guard = self.writer.lock().unwrap_or_else(|e| e.into_inner());
        self.attach(&mut guard).map(|_| ())
    }
}

/// Builds the endpoint described by a device config.
///
/// `virtual` devices get a [`VirtualEndpoint`] (used for dry runs against a real pack). MIDI devices
/// connect lazily to the output port whose name contains `address`, and reconnect after a failure;
/// until a matching port exists, sends report the device unreachable. `dmx512` devices open the
/// named serial port lazily on the same terms.
pub fn build_endpoint(config: &DeviceConfig) -> Result<Box<dyn Endpoint>> {
    let address = config.address.clone().unwrap_or_default();
    let udp = |protocol| {
        UdpEndpoint::new(protocol, &address, config.scenes.clone())
            .map(|e| Box::new(e) as Box<dyn Endpoint>)
            .map_err(|e| Error::Control(format!("device `{}`: {e}", config.id)))
    };
    match config.protocol.as_str() {
        "osc" => udp(UdpProtocol::Osc),
        "artnet" => udp(UdpProtocol::ArtNet),
        "sacn" => udp(UdpProtocol::Sacn),
        "midi" => Ok(Box::new(MidiEndpoint::with_opener(crate::midi_port::output_opener(
            address,
        )))),
        "dmx512" => {
            if address.trim().is_empty() {
                return Err(Error::Control(format!(
                    "device `{}`: a dmx512 device needs a serial port name in `address`",
                    config.id
                )));
            }
            Ok(Box::new(DmxSerialEndpoint::with_opener(
                crate::dmx_serial::port_opener(address, 1000),
                config.scenes.clone(),
            )))
        }
        "virtual" => Ok(Box::new(VirtualEndpoint::new())),
        other => Err(Error::Control(format!(
            "device `{}`: unknown protocol `{other}`",
            config.id
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::UdpSocket;
    use std::time::Duration;
    use tpt_av_control_dmx::artnet::{parse_packet as parse_artnet, ArtNetPacket};

    fn listener() -> (UdpSocket, String) {
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        socket.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let addr = socket.local_addr().unwrap().to_string();
        (socket, addr)
    }

    #[test]
    fn osc_round_trips_over_udp() {
        let (rx, addr) = listener();
        let ep = UdpEndpoint::new(UdpProtocol::Osc, &addr, BTreeMap::new()).unwrap();
        ep.send(&Command::Osc {
            address: "/projector/1/power".into(),
            args: vec![1.0, 0.5],
        })
        .unwrap();
        let mut buf = [0u8; 512];
        let n = rx.recv(&mut buf).unwrap();
        let msg = OscMessage::decode(&buf[..n]).unwrap();
        assert_eq!(msg.address, "/projector/1/power");
        assert_eq!(msg.arguments, vec![OscArg::Int(1), OscArg::Float(0.5)]);
    }

    #[test]
    fn artnet_dmx_merges_channels_into_universe_state() {
        let (rx, addr) = listener();
        let ep = UdpEndpoint::new(UdpProtocol::ArtNet, &addr, BTreeMap::new()).unwrap();
        let cmd = |start, values: Vec<u8>| Command::DmxChannels {
            universe: 2,
            start_channel: start,
            values,
            transport: DmxTransport::ArtNet,
        };
        ep.send(&cmd(0, vec![10, 20])).unwrap();
        ep.send(&cmd(4, vec![99])).unwrap();
        let mut buf = [0u8; 1024];
        rx.recv(&mut buf).unwrap();
        let n = rx.recv(&mut buf).unwrap();
        match parse_artnet(&buf[..n]).unwrap() {
            ArtNetPacket::Dmx { universe, slots } => {
                assert_eq!(universe, 2);
                assert_eq!(&slots[..5], &[10, 20, 0, 0, 99]);
            }
            other => panic!("unexpected packet {other:?}"),
        }
    }

    #[test]
    fn scene_recall_uses_the_device_scene_table() {
        let (rx, addr) = listener();
        let mut scenes = BTreeMap::new();
        scenes.insert(
            "half".to_string(),
            SceneDef {
                universe: 1,
                start_channel: 1,
                values: vec![128, 128],
            },
        );
        let ep = UdpEndpoint::new(UdpProtocol::ArtNet, &addr, scenes).unwrap();
        ep.send(&Command::DmxScene {
            scene: "half".into(),
            fade_ms: 0,
        })
        .unwrap();
        let mut buf = [0u8; 1024];
        let n = rx.recv(&mut buf).unwrap();
        assert!(matches!(parse_artnet(&buf[..n]).unwrap(), ArtNetPacket::Dmx { .. }));
        assert!(matches!(
            ep.send(&Command::DmxScene {
                scene: "missing".into(),
                fade_ms: 0
            }),
            Err(EndpointError::Rejected(_))
        ));
    }

    #[test]
    fn sacn_packets_carry_the_universe() {
        let (rx, addr) = listener();
        let ep = UdpEndpoint::new(UdpProtocol::Sacn, &addr, BTreeMap::new()).unwrap();
        ep.send(&Command::DmxUniverse {
            universe: 7,
            values: vec![1, 2, 3],
            transport: DmxTransport::Sacn,
        })
        .unwrap();
        let mut buf = [0u8; 1024];
        let n = rx.recv(&mut buf).unwrap();
        let packet = tpt_av_control_dmx::sacn::parse_packet(&buf[..n]).unwrap().unwrap();
        assert_eq!(packet.universe, 7);
        assert_eq!(&packet.slots[..3], &[1, 2, 3]);
    }

    #[test]
    fn dmx_boundary_and_transport_mismatch_are_rejected() {
        let (_rx, addr) = listener();
        let ep = UdpEndpoint::new(UdpProtocol::ArtNet, &addr, BTreeMap::new()).unwrap();
        let too_far = Command::DmxChannels {
            universe: 1,
            start_channel: 511,
            values: vec![1, 2],
            transport: DmxTransport::ArtNet,
        };
        assert!(matches!(ep.send(&too_far), Err(EndpointError::Rejected(_))));
        let wrong = Command::DmxChannels {
            universe: 1,
            start_channel: 0,
            values: vec![1],
            transport: DmxTransport::Sacn,
        };
        assert!(matches!(ep.send(&wrong), Err(EndpointError::Rejected(_))));
        let last_ok = Command::DmxChannels {
            universe: 1,
            start_channel: 511,
            values: vec![1],
            transport: DmxTransport::ArtNet,
        };
        assert!(ep.send(&last_ok).is_ok());
    }

    #[test]
    fn non_finite_and_malformed_osc_is_rejected() {
        let (_rx, addr) = listener();
        let ep = UdpEndpoint::new(UdpProtocol::Osc, &addr, BTreeMap::new()).unwrap();
        for cmd in [
            Command::Osc {
                address: "/x".into(),
                args: vec![f64::NAN],
            },
            Command::Osc {
                address: "no-slash".into(),
                args: vec![],
            },
        ] {
            assert!(matches!(ep.send(&cmd), Err(EndpointError::Rejected(_))), "{cmd:?}");
        }
    }

    struct Capture(std::sync::Arc<Mutex<Vec<Vec<u8>>>>);
    impl MidiWriter for Capture {
        fn write(&mut self, bytes: &[u8]) -> std::result::Result<(), String> {
            self.0.lock().unwrap().push(bytes.to_vec());
            Ok(())
        }
    }

    #[test]
    fn midi_encoding_validates_ranges() {
        let store = std::sync::Arc::new(Mutex::new(Vec::new()));
        let ep = MidiEndpoint::new(Box::new(Capture(store.clone())));
        let midi = |channel, kind: &str, number, value| Command::Midi {
            channel,
            kind: kind.into(),
            number,
            value,
        };
        ep.send(&midi(1, "note_on", 60, 127)).unwrap();
        ep.send(&midi(0, "cc", 7, 64)).unwrap();
        ep.send(&midi(15, "program_change", 5, 0)).unwrap();
        assert_eq!(
            *store.lock().unwrap(),
            vec![vec![0x91, 60, 127], vec![0xB0, 7, 64], vec![0xCF, 5]]
        );
        for bad in [
            midi(16, "note_on", 60, 1),
            midi(0, "note_on", 128, 1),
            midi(0, "note_on", 60, 128),
            midi(0, "sysex", 1, 1),
        ] {
            assert!(matches!(ep.send(&bad), Err(EndpointError::Rejected(_))), "{bad:?}");
        }
        assert_eq!(store.lock().unwrap().len(), 3);
    }

    #[test]
    fn detached_midi_reports_unreachable() {
        let ep = MidiEndpoint::detached();
        assert!(ep.ping().is_err());
        assert!(matches!(
            ep.send(&Command::Midi {
                channel: 0,
                kind: "cc".into(),
                number: 1,
                value: 1
            }),
            Err(EndpointError::Unreachable(_))
        ));
    }

    #[test]
    fn a_failing_output_is_reopened_on_the_next_send() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let opens = std::sync::Arc::new(AtomicUsize::new(0));
        let store = std::sync::Arc::new(Mutex::new(Vec::new()));
        let (o, st) = (opens.clone(), store.clone());
        let ep = MidiEndpoint::with_opener(Box::new(move || {
            let n = o.fetch_add(1, Ordering::SeqCst);
            if n == 0 {
                Err("port not present yet".to_string())
            } else {
                Ok(Box::new(Capture(st.clone())) as Box<dyn MidiWriter>)
            }
        }));
        let cmd = Command::Midi {
            channel: 0,
            kind: "cc".into(),
            number: 1,
            value: 2,
        };
        assert!(matches!(ep.send(&cmd), Err(EndpointError::Unreachable(_))), "port missing at first");
        assert!(ep.send(&cmd).is_ok(), "the port appeared; the endpoint reconnected");
        assert_eq!(store.lock().unwrap().len(), 1);
        assert_eq!(opens.load(Ordering::SeqCst), 2);
        assert!(ep.ping().is_ok());
    }

    #[test]
    fn build_endpoint_honours_protocol() {
        let cfg = |protocol: &str| DeviceConfig {
            id: "d".into(),
            name: None,
            kind: Default::default(),
            protocol: protocol.into(),
            address: Some("127.0.0.1:9".into()),
            heartbeat_ms: None,
            scenes: BTreeMap::new(),
        };
        for p in ["osc", "artnet", "sacn", "midi", "virtual"] {
            assert!(build_endpoint(&cfg(p)).is_ok(), "{p}");
        }
        assert!(build_endpoint(&cfg("bogus")).is_err());
    }

    #[test]
    fn a_dmx512_device_builds_a_serial_endpoint_that_needs_a_port_name() {
        let base = DeviceConfig {
            id: "rig".into(),
            name: None,
            kind: Default::default(),
            protocol: "dmx512".into(),
            address: Some("COM3".into()),
            heartbeat_ms: None,
            scenes: BTreeMap::new(),
        };
        assert!(
            build_endpoint(&base).is_ok(),
            "the endpoint is built lazily, so an absent adapter is not a load error"
        );
        let unnamed = DeviceConfig {
            address: None,
            ..base.clone()
        };
        assert!(build_endpoint(&unnamed).is_err(), "no port name is a configuration error");
        let blank = DeviceConfig {
            address: Some("   ".into()),
            ..base
        };
        assert!(build_endpoint(&blank).is_err(), "a blank port name is a configuration error");
    }
}
