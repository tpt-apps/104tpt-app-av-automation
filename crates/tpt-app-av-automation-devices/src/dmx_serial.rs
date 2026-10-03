//! Serial DMX512-A output **and input** (spec §5.1, §7.2, §8.1).
//!
//! A DMX512-A frame on the wire is not just 512 data bytes: it is a **break** (at least 92 us of
//! low), a mark after the break (at least 8 us), a **start** code (at least 8 us of low), the 512
//! slot values, and a mark after the data. Everything except the slot bytes is produced by holding
//! the line low or letting it idle, so this module hands the [`DmxSerialPort`] implementation the
//! two halves — break and payload — rather than pretending a 513-byte `write` is a valid frame.
//!
//! Reading a DMX512 line back is the same problem from the other side, and harder: there is no
//! framing in the byte stream itself. A receiver must time the gaps between bytes to find the
//! break that starts each frame. [`Dmx512Assembler`] does exactly that, turning
//! `(byte, microseconds since the previous byte)` pairs from a serial port into whole
//! [`Dmx512Frame`]s, and is driven by a [`DmxSerialReader`] so the framing is testable with a fake
//! port — CI has no USB-to-DMX adapter.
//!
//! The engine never merges universes across transports: an Art-Net or sACN endpoint keeps one
//! 512-slot buffer per universe, whereas a serial port *is* one universe. A `dmx512` device
//! therefore accepts only universe 1, and a command naming any other universe is rejected rather
//! than silently written to the wire.
//!
//! Opening the port is behind a [`DmxSerialOpener`] so an endpoint can reconnect after a cable is
//! unplugged, and so the framing and universe bookkeeping are testable with a fake port — CI has no
//! USB-to-DMX adapter.

use std::sync::Mutex;

use tpt_app_av_automation_model::DmxTransport;

use crate::command::Command;
use crate::config::SceneDef;
use crate::endpoint::{Endpoint, EndpointError};

/// DMX512-A slot count.
pub const DMX_CHANNELS: usize = 512;

/// Minimum break duration required by DMX512-A, in microseconds (spec minimum 92 us).
pub const BREAK_MICROS: u32 = 250;

/// Minimum mark time required between break, start and data, in microseconds (spec minimum 8 us).
pub const MARK_MICROS: u32 = 16;

/// Baud rate every DMX512-A adapter uses. There is no other rate in the standard.
pub const BAUD: u32 = 250_000;

/// One DMX512-A universe: the break/start preamble plus 512 slot values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dmx512Frame {
    /// The 512 slot values, index 0 = channel 1.
    pub slots: [u8; DMX_CHANNELS],
}

impl Dmx512Frame {
    /// An all-zero (lights out) frame.
    pub fn blackout() -> Self {
        Self {
            slots: [0u8; DMX_CHANNELS],
        }
    }

    /// Builds a frame from exactly 512 values.
    pub fn from_slots(slots: &[u8]) -> Option<Self> {
        if slots.len() != DMX_CHANNELS {
            return None;
        }
        let mut frame = Self::blackout();
        frame.slots.copy_from_slice(slots);
        Some(frame)
    }

    /// Overwrites `values` starting at the zero-based `start` channel. Out-of-range writes are
    /// rejected so a bad scene cannot be silently truncated.
    pub fn write(&mut self, start: usize, values: &[u8]) -> Result<(), EndpointError> {
        if start + values.len() > DMX_CHANNELS {
            return Err(EndpointError::Rejected(
                "values run past channel 512".into(),
            ));
        }
        self.slots[start..start + values.len()].copy_from_slice(values);
        Ok(())
    }

    /// The bytes sent after the start code: exactly one frame's worth of slot values.
    pub fn payload(&self) -> &[u8; DMX_CHANNELS] {
        &self.slots
    }
}

/// A DMX512 serial line this module can drive.
///
/// Implemented over `serialport` in production (see [`open_port`]) and by an in-memory fake in tests.
pub trait DmxSerialPort: Send {
    /// Holds the line low for `micros` to produce the break, then idles for the mark-after-break.
    fn send_break(&mut self, micros: u32) -> Result<(), String>;

    /// Writes exactly `bytes` of slot data, then idles for the mark-after-data.
    fn write_slots(&mut self, bytes: &[u8]) -> Result<(), String>;
}

/// Opens a [`DmxSerialPort`] on demand; called again after a failure so an adapter that is
/// unplugged and replugged recovers without restarting the engine.
pub type DmxSerialOpener = Box<dyn Fn() -> Result<Box<dyn DmxSerialPort>, String> + Send + Sync>;

/// An endpoint writing DMX512-A frames to one serial port.
pub struct DmxSerialEndpoint {
    port: Mutex<Option<Box<dyn DmxSerialPort>>>,
    opener: Option<DmxSerialOpener>,
    frame: Mutex<Dmx512Frame>,
    scenes: std::collections::BTreeMap<String, SceneDef>,
}

impl std::fmt::Debug for DmxSerialEndpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DmxSerialEndpoint").finish_non_exhaustive()
    }
}

impl DmxSerialEndpoint {
    /// Wraps an already-open port.
    pub fn new(
        port: Box<dyn DmxSerialPort>,
        scenes: std::collections::BTreeMap<String, SceneDef>,
    ) -> Self {
        Self {
            port: Mutex::new(Some(port)),
            opener: None,
            frame: Mutex::new(Dmx512Frame::blackout()),
            scenes,
        }
    }

    /// An endpoint with no port attached; every send reports the device unreachable.
    pub fn detached() -> Self {
        Self {
            port: Mutex::new(None),
            opener: None,
            frame: Mutex::new(Dmx512Frame::blackout()),
            scenes: std::collections::BTreeMap::new(),
        }
    }

    /// An endpoint that connects lazily through `opener` and reconnects after a failure.
    pub fn with_opener(
        opener: DmxSerialOpener,
        scenes: std::collections::BTreeMap<String, SceneDef>,
    ) -> Self {
        Self {
            port: Mutex::new(None),
            opener: Some(opener),
            frame: Mutex::new(Dmx512Frame::blackout()),
            scenes,
        }
    }

    /// The slot state that would be transmitted by the next frame.
    pub fn frame(&self) -> Dmx512Frame {
        self.frame.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Ensures a port is attached, opening one if an opener is configured.
    fn attach<'a>(
        &self,
        guard: &'a mut Option<Box<dyn DmxSerialPort>>,
    ) -> Result<&'a mut Box<dyn DmxSerialPort>, EndpointError> {
        if guard.is_none() {
            match &self.opener {
                Some(open) => *guard = Some(open().map_err(EndpointError::Unreachable)?),
                None => {
                    return Err(EndpointError::Unreachable(
                        "no DMX serial port attached".into(),
                    ))
                }
            }
        }
        Ok(guard.as_mut().expect("attached above"))
    }

    /// Merges `values` into the universe and transmits a complete frame.
    ///
    /// The new slot state is committed only once the frame has actually gone out. If the port is
    /// unreachable, the universe keeps the values the fixtures last *received* rather than values
    /// that were only ever intended — otherwise a later successful send would emit a mix of
    /// confirmed and unconfirmed state, and a caller reading `frame()` would be told the rig is in
    /// a lighting state it never actually reached.
    fn write_dmx(&self, start: usize, values: &[u8], replace: bool) -> Result<(), EndpointError> {
        let candidate = {
            let frame = self.frame.lock().unwrap_or_else(|e| e.into_inner());
            let mut next = if replace {
                Dmx512Frame::blackout()
            } else {
                frame.clone()
            };
            next.write(start, values)?;
            next
        };
        let mut guard = self.port.lock().unwrap_or_else(|e| e.into_inner());
        let port = self.attach(&mut guard)?;
        let result = transmit(port, candidate.payload());
        if result.is_err() {
            if self.opener.is_some() {
                // Drop the broken handle so the next send reopens the port.
                *guard = None;
            }
            return result;
        }
        *self.frame.lock().unwrap_or_else(|e| e.into_inner()) = candidate;
        Ok(())
    }
}

/// Sends one DMX512-A frame: break, mark, the 512 slots, mark.
fn transmit(
    port: &mut Box<dyn DmxSerialPort>,
    payload: &[u8; DMX_CHANNELS],
) -> Result<(), EndpointError> {
    port.send_break(BREAK_MICROS)
        .map_err(|e| EndpointError::Unreachable(format!("DMX512 break failed: {e}")))?;
    port.write_slots(payload)
        .map_err(|e| EndpointError::Unreachable(format!("DMX512 write failed: {e}")))?;
    Ok(())
}

impl Endpoint for DmxSerialEndpoint {
    fn send(&self, command: &Command) -> Result<(), EndpointError> {
        match command {
            Command::DmxChannels {
                universe,
                start_channel,
                values,
                transport,
            } => {
                check_universe_and_transport(*universe, Some(*transport))?;
                self.write_dmx(usize::from(*start_channel), values, false)
            }
            Command::DmxUniverse {
                universe,
                values,
                transport,
            } => {
                check_universe_and_transport(*universe, Some(*transport))?;
                self.write_dmx(0, values, true)
            }
            Command::DmxScene { scene, .. } => {
                let def = self
                    .scenes
                    .get(scene)
                    .ok_or_else(|| EndpointError::Rejected(format!("unknown scene `{scene}`")))?;
                check_universe_and_transport(def.universe, None)?;
                self.write_dmx(usize::from(def.start_channel), &def.values, false)
            }
            other => Err(EndpointError::Rejected(format!(
                "`{}` is not supported by a DMX512 serial endpoint",
                other.describe()
            ))),
        }
    }

    fn ping(&self) -> Result<(), EndpointError> {
        let mut guard = self.port.lock().unwrap_or_else(|e| e.into_inner());
        self.attach(&mut guard).map(|_| ())
    }
}

/// A serial port carries exactly one universe, so only universe 1 is addressable.
fn check_universe_and_transport(
    universe: u16,
    transport: Option<DmxTransport>,
) -> Result<(), EndpointError> {
    if universe != 1 {
        return Err(EndpointError::Rejected(format!(
            "a serial DMX512 port carries universe 1 only; action asked for universe {universe}"
        )));
    }
    if let Some(t) = transport {
        if t != DmxTransport::Dmx512 {
            return Err(EndpointError::Rejected(format!(
                "action transport {t:?} does not match this endpoint (Dmx512)"
            )));
        }
    }
    Ok(())
}

/// A [`DmxSerialPort`] over a real `serialport` port, e.g. `COM3` or `/dev/ttyUSB0`.
///
/// The port is opened at the DMX512-A baud rate with 8 data bits, no parity and one stop bit, which
/// `serialport` maps to 8N1. The break is generated by the driver, not by a byte value, because a
/// zero byte is not a break on a UART.
pub fn open_port(port_name: &str, timeout_ms: u64) -> Result<Box<dyn DmxSerialPort>, String> {
    let port = serialport::new(port_name, BAUD)
        .timeout(std::time::Duration::from_millis(timeout_ms))
        .data_bits(serialport::DataBits::Eight)
        .parity(serialport::Parity::None)
        .stop_bits(serialport::StopBits::One)
        .flow_control(serialport::FlowControl::None)
        .open()
        .map_err(|e| format!("cannot open DMX serial port `{port_name}`: {e}"))?;
    Ok(Box::new(SerialPortWriter { port }))
}

/// Names of the serial ports currently present.
pub fn available_ports() -> Vec<String> {
    serialport::available_ports()
        .map(|ports| ports.into_iter().map(|p| p.port_name).collect())
        .unwrap_or_default()
}
/// A byte read from a DMX line, with the time since the previous byte.
///
/// The inter-byte gap is what carries the framing: a DMX512 slot byte occupies 44 us (4 us per bit,
/// 11 bits) at 250 kbaud, while a break is at least 92 us. A receiver cannot tell a break from a
/// slot by content, only by how long the line was idle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimedByte {
    /// The byte value, 0-255.
    pub byte: u8,
    /// Microseconds since the previously reported byte.
    pub gap_micros: u32,
}

/// A DMX512 serial line this module can read.
///
/// Implemented over `serialport` in production (see [`open_reader`]) and by a fake in tests.
pub trait DmxSerialReader: Send {
    /// Reads whatever bytes have arrived, each with its gap since the previous one.
    ///
    /// Returns an empty vector when nothing has arrived yet; a timeout must not be reported as an
    /// error, because a DMX line is idle between refreshes.
    fn read_timed(&mut self) -> Result<Vec<TimedByte>, String>;
}

/// Opens a [`DmxSerialReader`] on demand, for reconnect after an adapter is unplugged.
pub type DmxReaderOpener = Box<dyn Fn() -> Result<Box<dyn DmxSerialReader>, String> + Send + Sync>;

/// Where the assembler is in a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum AssemblyState {
    /// Waiting for a gap long enough to be a break.
    #[default]
    Idle,
    /// A break was seen; the next byte is the start code and the 512 slots follow.
    InFrame {
        /// How many bytes have been seen since the break, including the start code.
        seen: usize,
    },
}

/// Turns a timed byte stream into whole DMX512-A frames.
///
/// DMX512 carries no length, checksum or framing in the data stream, so a receiver recovers the
/// structure purely from timing: a gap of at least [`BREAK_MICROS`] is a break, and the next
/// [`DMX_CHANNELS`] bytes after it are the slots. A frame is returned only once all 512 slots have
/// arrived, so a line that drops mid-frame can never be mistaken for a complete one.
#[derive(Debug, Default)]
pub struct Dmx512Assembler {
    state: AssemblyState,
    slots: Vec<u8>,
    /// Bytes discarded because they arrived with no break in front of them.
    dropped_bytes: u64,
    /// Frames discarded because the line stopped before all 512 slots arrived.
    dropped_frames: u64,
}

impl Dmx512Assembler {
    /// A fresh assembler waiting for the first break.
    pub fn new() -> Self {
        Self::default()
    }

    /// Bytes seen outside a frame, which is normal between frames.
    pub fn dropped_bytes(&self) -> u64 {
        self.dropped_bytes
    }

    /// Frames that began but never delivered 512 slots.
    pub fn dropped_frames(&self) -> u64 {
        self.dropped_frames
    }

    /// Feeds one timed byte, returning the frame when the 512th slot completes it.
    pub fn push(&mut self, timed: TimedByte) -> Option<Dmx512Frame> {
        match self.state {
            AssemblyState::Idle => {
                if timed.gap_micros >= BREAK_MICROS {
                    // The break is idle time, not a byte. The byte that follows it is the
                    // frame's start code, consumed here; it is not one of the 512 slots, and
                    // every byte after it is slot data until the next break.
                    self.state = AssemblyState::InFrame { seen: 0 };
                    self.slots.clear();
                } else {
                    // Idle-line noise between frames.
                    self.dropped_bytes += 1;
                }
                None
            }
            AssemblyState::InFrame { seen } => {
                if timed.gap_micros >= BREAK_MICROS {
                    // A break arrived before the frame filled: the link truncated it, so the
                    // partial slots are discarded and this byte starts the next frame. Counting
                    // the loss is what makes a failing line visible instead of it silently
                    // delivering nothing.
                    self.dropped_frames += 1;
                    self.state = AssemblyState::InFrame { seen: 0 };
                    self.slots.clear();
                    return None;
                }
                let seen = seen + 1;
                self.slots.push(timed.byte);
                if self.slots.len() < DMX_CHANNELS {
                    self.state = AssemblyState::InFrame { seen };
                    return None;
                }
                self.state = AssemblyState::Idle;
                let frame = Dmx512Frame::from_slots(&self.slots);
                self.slots.clear();
                frame
            }
        }
    }

    /// Feeds a run of timed bytes, returning every complete frame in order.
    pub fn push_all(&mut self, bytes: &[TimedByte]) -> Vec<Dmx512Frame> {
        bytes.iter().filter_map(|b| self.push(*b)).collect()
    }
}

/// An opener that opens `port_name` on each call, for reconnect-after-failure.
pub fn port_opener(port_name: String, timeout_ms: u64) -> DmxSerialOpener {
    Box::new(move || open_port(&port_name, timeout_ms))
}

/// Opens a DMX line for reading at the DMX512-A baud rate.
pub fn open_reader(port_name: &str, timeout_ms: u64) -> Result<Box<dyn DmxSerialReader>, String> {
    let port = serialport::new(port_name, BAUD)
        .timeout(std::time::Duration::from_millis(timeout_ms))
        .data_bits(serialport::DataBits::Eight)
        .parity(serialport::Parity::None)
        .stop_bits(serialport::StopBits::One)
        .flow_control(serialport::FlowControl::None)
        .open()
        .map_err(|e| format!("cannot open DMX serial port `{port_name}`: {e}"))?;
    Ok(Box::new(SerialPortReader {
        port,
        last_read: std::time::Instant::now(),
        pending: std::collections::VecDeque::new(),
    }))
}

/// An opener that opens `port_name` for reading on each call.
pub fn reader_opener(port_name: String, timeout_ms: u64) -> DmxReaderOpener {
    Box::new(move || open_reader(&port_name, timeout_ms))
}

/// Reads timed bytes from a real `serialport` port.
///
/// A serial port hands back a buffer of bytes and no per-byte timestamps, so the gaps are
/// reconstructed here: within one buffer a byte is one slot time from its predecessor, and the gap
/// before the first byte is measured from the end of the previous read. That is only approximate
/// for a break that falls inside a buffer, which is why a partial frame is discarded rather than
/// completed — the next break re-frames the stream correctly.
struct SerialPortReader {
    port: Box<dyn serialport::SerialPort>,
    last_read: std::time::Instant,
    /// Bytes read but not yet reported, kept so a partial read still carries its gaps.
    pending: std::collections::VecDeque<u8>,
}

impl SerialPortReader {
    /// The nominal time one slot byte occupies on the wire.
    const SLOT_MICROS: u32 = 44;
}

impl DmxSerialReader for SerialPortReader {
    fn read_timed(&mut self) -> Result<Vec<TimedByte>, String> {
        // A one-byte read returns as soon as a byte is available, which is what lets the gap
        // measurement follow the line rather than lag a whole buffer behind it.
        self.port
            .set_timeout(std::time::Duration::from_millis(10))
            .map_err(|e| e.to_string())?;
        let mut buffer = [0u8; 1];
        match self.port.read(&mut buffer) {
            Ok(0) => Ok(Vec::new()),
            Ok(n) => {
                let now = std::time::Instant::now();
                let gap = now
                    .saturating_duration_since(self.last_read)
                    .as_micros()
                    .min(u128::from(u32::MAX)) as u32;
                self.last_read = now;
                self.pending.extend(buffer[..n].iter().copied());
                // Report only what has a measured gap behind it; a trailing byte would have to
                // guess one.
                if self.pending.len() < 2 {
                    return Ok(Vec::new());
                }
                let first = self.pending.pop_front().expect("len checked");
                let rest: Vec<TimedByte> = self
                    .pending
                    .drain(..)
                    .map(|byte| TimedByte {
                        byte,
                        gap_micros: Self::SLOT_MICROS,
                    })
                    .collect();
                let mut out = Vec::with_capacity(rest.len() + 1);
                out.push(TimedByte {
                    byte: first,
                    gap_micros: gap,
                });
                out.extend(rest);
                Ok(out)
            }
            // A read timeout means the line is idle between refreshes, which is not a failure.
            Err(e) if e.kind() == std::io::ErrorKind::TimedOut => Ok(Vec::new()),
            Err(e) => Err(e.to_string()),
        }
    }
}

struct SerialPortWriter {
    port: Box<dyn serialport::SerialPort>,
}

impl SerialPortWriter {
    fn port_mut(&mut self) -> &mut dyn serialport::SerialPort {
        &mut *self.port
    }
}

impl DmxSerialPort for SerialPortWriter {
    fn send_break(&mut self, micros: u32) -> Result<(), String> {
        // `set_break`/`clear_break` are the cross-platform way to hold the line low: unlike
        // `TTY::send_break`, they exist on the `SerialPort` trait itself, so this compiles and
        // behaves the same on Windows and Unix. Hold for longer than the 92 us minimum and only
        // then release, because a break that is too short is ignored by receivers outright.
        self.port_mut().set_break().map_err(|e| e.to_string())?;
        let hold = std::time::Duration::from_micros(u64::from(micros));
        std::thread::sleep(hold);
        self.port_mut().clear_break().map_err(|e| e.to_string())?;
        // Mark after break: idle the line for at least the minimum mark time before the start
        // code, which the first slot byte's own start bit then supplies.
        std::thread::sleep(std::time::Duration::from_micros(u64::from(MARK_MICROS)));
        Ok(())
    }

    fn write_slots(&mut self, bytes: &[u8]) -> Result<(), String> {
        if bytes.len() != DMX_CHANNELS {
            return Err(format!(
                "expected {DMX_CHANNELS} slots, got {}",
                bytes.len()
            ));
        }
        self.port.write_all(bytes).map_err(|e| e.to_string())?;
        self.port.flush().map_err(|e| e.to_string())?;
        // Mark after data, then hold the line idle so receivers see the frame end.
        std::thread::sleep(std::time::Duration::from_micros(u64::from(MARK_MICROS)));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex as StdMutex};

    /// One recorded half of a transmitted frame.
    #[derive(Debug, Clone, PartialEq, Eq)]
    enum Step {
        Break(u32),
        Slots(Vec<u8>),
    }

    /// Records the break/slot sequence so framing can be asserted without hardware.
    #[derive(Clone)]
    struct FakePort {
        log: Arc<StdMutex<Vec<Step>>>,
        fail_next: Arc<StdMutex<u32>>,
    }

    impl FakePort {
        fn new() -> Self {
            Self {
                log: Arc::new(StdMutex::new(Vec::new())),
                fail_next: Arc::new(StdMutex::new(0)),
            }
        }

        /// Number of breaks recorded; each frame is expected to send exactly one.
        fn breaks(&self) -> usize {
            self.log
                .lock()
                .unwrap()
                .iter()
                .filter(|s| matches!(s, Step::Break(_)))
                .count()
        }

        /// Slot payloads in transmission order.
        fn frames(&self) -> Vec<Vec<u8>> {
            self.log
                .lock()
                .unwrap()
                .iter()
                .filter_map(|s| match s {
                    Step::Slots(b) => Some(b.clone()),
                    Step::Break(_) => None,
                })
                .collect()
        }

        /// Break durations in microseconds, in order.
        fn break_lengths(&self) -> Vec<u32> {
            self.log
                .lock()
                .unwrap()
                .iter()
                .filter_map(|s| match s {
                    Step::Break(us) => Some(*us),
                    Step::Slots(_) => None,
                })
                .collect()
        }
    }

    impl Default for FakePort {
        fn default() -> Self {
            Self::new()
        }
    }

    impl DmxSerialPort for FakePort {
        fn send_break(&mut self, micros: u32) -> Result<(), String> {
            self.log.lock().unwrap().push(Step::Break(micros));
            Ok(())
        }

        fn write_slots(&mut self, bytes: &[u8]) -> Result<(), String> {
            let mut fail = self.fail_next.lock().unwrap();
            if *fail > 0 {
                *fail -= 1;
                return Err("cable unplugged".into());
            }
            drop(fail);
            self.log.lock().unwrap().push(Step::Slots(bytes.to_vec()));
            Ok(())
        }
    }

    fn scenes() -> BTreeMap<String, SceneDef> {
        let mut map = BTreeMap::new();
        map.insert(
            "house".to_string(),
            SceneDef {
                universe: 1,
                start_channel: 0,
                values: vec![128, 128, 0],
            },
        );
        map
    }

    fn channels(universe: u16, start_channel: u16, values: Vec<u8>) -> Command {
        Command::DmxChannels {
            universe,
            start_channel,
            values,
            transport: DmxTransport::Dmx512,
        }
    }

    #[test]
    fn a_frame_is_512_slots_and_a_blackout_is_all_zero() {
        let frame = Dmx512Frame::blackout();
        assert_eq!(frame.payload().len(), DMX_CHANNELS);
        assert!(frame.payload().iter().all(|v| *v == 0));
        assert_eq!(
            Dmx512Frame::from_slots(&[7u8; DMX_CHANNELS])
                .unwrap()
                .payload()[0],
            7
        );
        assert!(
            Dmx512Frame::from_slots(&[0u8; 511]).is_none(),
            "511 slots is not a frame"
        );
    }

    #[test]
    fn writing_past_channel_512_is_rejected_not_truncated() {
        let mut frame = Dmx512Frame::blackout();
        assert!(frame.write(511, &[1, 2]).is_err());
        assert!(frame.write(512, &[1]).is_err());
        assert!(frame.write(511, &[1]).is_ok());
        assert_eq!(frame.payload()[511], 1);
        assert_eq!(
            frame.payload()[510],
            0,
            "the rejected write changed nothing"
        );
    }

    #[test]
    fn a_send_emits_one_break_of_at_least_the_required_length_then_512_slots() {
        let port = FakePort::new();
        let ep = DmxSerialEndpoint::new(Box::new(port.clone()), scenes());
        ep.send(&channels(1, 0, vec![200, 100])).unwrap();

        assert_eq!(port.breaks(), 1, "one break per frame");
        let break_us = port.break_lengths()[0];
        assert!(
            break_us >= 92,
            "break of {break_us}us is shorter than the DMX512-A minimum of 92us"
        );
        assert_eq!(BREAK_MICROS, break_us);
        const { assert!(MARK_MICROS >= 8) }; // the mark must meet the DMX512-A minimum

        let frames = port.frames();
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].len(), DMX_CHANNELS);
        assert_eq!(&frames[0][..2], &[200, 100]);
    }

    #[test]
    fn channel_writes_merge_into_the_universe_rather_than_replacing_it() {
        let port = FakePort::new();
        let ep = DmxSerialEndpoint::new(Box::new(port.clone()), scenes());
        ep.send(&channels(1, 0, vec![10, 20])).unwrap();
        ep.send(&channels(1, 2, vec![30])).unwrap();

        let frames = port.frames();
        assert_eq!(frames.len(), 2);
        assert_eq!(&frames[0][..4], &[10, 20, 0, 0]);
        assert_eq!(
            &frames[1][..4],
            &[10, 20, 30, 0],
            "the earlier channels survive the merge, as on Art-Net/sACN"
        );

        // A whole-universe action does replace.
        ep.send(&Command::DmxUniverse {
            universe: 1,
            values: vec![1; DMX_CHANNELS],
            transport: DmxTransport::Dmx512,
        })
        .unwrap();
        assert!(port.frames()[2].iter().all(|v| *v == 1));
    }

    #[test]
    fn a_scene_uses_the_device_scene_table() {
        let port = FakePort::new();
        let ep = DmxSerialEndpoint::new(Box::new(port.clone()), scenes());
        ep.send(&Command::DmxScene {
            scene: "house".into(),
            fade_ms: 0,
        })
        .unwrap();
        assert_eq!(&port.frames()[0][..3], &[128, 128, 0]);
        assert!(matches!(
            ep.send(&Command::DmxScene {
                scene: "missing".into(),
                fade_ms: 0
            }),
            Err(EndpointError::Rejected(_))
        ));
    }

    #[test]
    fn only_universe_one_and_the_dmx512_transport_are_accepted() {
        let ep = DmxSerialEndpoint::new(Box::new(FakePort::new()), scenes());
        for cmd in [
            channels(2, 0, vec![1]),
            Command::DmxChannels {
                universe: 1,
                start_channel: 0,
                values: vec![1],
                transport: DmxTransport::ArtNet,
            },
            Command::DmxChannels {
                universe: 1,
                start_channel: 0,
                values: vec![1],
                transport: DmxTransport::Sacn,
            },
            Command::DmxUniverse {
                universe: 3,
                values: vec![1],
                transport: DmxTransport::Dmx512,
            },
            Command::Osc {
                address: "/x".into(),
                args: vec![],
            },
        ] {
            assert!(
                matches!(ep.send(&cmd), Err(EndpointError::Rejected(_))),
                "expected rejection: {cmd:?}"
            );
        }
    }

    #[test]
    fn a_rejected_write_does_not_change_the_universe() {
        let ep = DmxSerialEndpoint::new(Box::new(FakePort::new()), scenes());
        let too_far = channels(1, 511, vec![1, 2]);
        assert!(matches!(ep.send(&too_far), Err(EndpointError::Rejected(_))));
        assert_eq!(
            ep.frame().payload()[510],
            0,
            "an over-long write is rejected before the universe is mutated"
        );
    }

    #[test]
    fn a_detached_endpoint_reports_unreachable_rather_than_pretending_to_send() {
        let ep = DmxSerialEndpoint::detached();
        assert!(ep.ping().is_err());
        assert!(matches!(
            ep.send(&channels(1, 0, vec![1])),
            Err(EndpointError::Unreachable(_))
        ));
        assert_eq!(
            ep.frame().payload()[0],
            0,
            "a failed send must not leave the universe looking updated"
        );
    }

    #[test]
    fn a_failing_port_is_reopened_on_the_next_send() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let opens = Arc::new(AtomicUsize::new(0));
        let port = FakePort::new();
        // The adapter is absent for the first send, then present.
        *port.fail_next.lock().unwrap() = 1;
        let (o, p) = (opens.clone(), port.clone());
        let ep = DmxSerialEndpoint::with_opener(
            Box::new(move || {
                o.fetch_add(1, Ordering::SeqCst);
                Ok(Box::new(p.clone()) as Box<dyn DmxSerialPort>)
            }),
            scenes(),
        );

        assert!(
            matches!(
                ep.send(&channels(1, 0, vec![9])),
                Err(EndpointError::Unreachable(_))
            ),
            "the first write fails"
        );
        assert_eq!(opens.load(Ordering::SeqCst), 1);
        assert_eq!(
            ep.frame().payload()[0],
            0,
            "the failed write is not committed to the universe"
        );
        assert!(
            ep.send(&channels(1, 0, vec![9])).is_ok(),
            "the adapter came back, so the endpoint reconnected without a restart"
        );
        assert_eq!(opens.load(Ordering::SeqCst), 2);
        assert!(ep.ping().is_ok());
        assert_eq!(
            port.frames().len(),
            1,
            "only the successful send reached the wire"
        );
        assert_eq!(
            port.frames()[0][0],
            9,
            "and it carried the value that was asked for"
        );
        assert_eq!(
            ep.frame().payload()[0],
            9,
            "now the universe reflects what was sent"
        );
    }

    #[test]
    fn an_opener_that_never_succeeds_degrades_rather_than_panicking() {
        let ep = DmxSerialEndpoint::with_opener(
            Box::new(|| Err("no such adapter".to_string())),
            scenes(),
        );
        for _ in 0..3 {
            assert!(matches!(
                ep.send(&channels(1, 0, vec![1])),
                Err(EndpointError::Unreachable(_))
            ));
        }
        assert!(ep.ping().is_err());
    }

    #[test]
    fn listing_and_opening_ports_without_hardware_is_an_error_not_a_panic() {
        let _ = available_ports();
        assert!(open_port("definitely-not-a-real-com-port-xyz", 50).is_err());
        assert!(open_port("", 50).is_err());
    }

    #[test]
    fn the_configured_baud_is_the_dmx512_rate() {
        assert_eq!(BAUD, 250_000);
    }

    #[test]
    fn the_break_precedes_the_slots_on_the_wire() {
        let port = FakePort::new();
        let ep = DmxSerialEndpoint::new(Box::new(port.clone()), scenes());
        ep.send(&channels(1, 0, vec![1])).unwrap();
        let log = port.log.lock().unwrap().clone();
        assert!(matches!(log[0], Step::Break(_)), "break first: {log:?}");
        assert!(matches!(log[1], Step::Slots(_)), "slots second: {log:?}");
    }
}

#[cfg(test)]
mod input_tests {
    use super::*;

    /// The gap between two consecutive slot bytes at 250 kbaud: 4 us per bit, 11 bits.
    const SLOT: u32 = 44;

    /// The gap before a start code: a mark after the break.
    const MARK: u32 = MARK_MICROS;

    /// One whole frame as timed bytes: a break, a start code, then 512 slots.
    ///
    /// A break is not a byte on the wire, only a length of idle, so it is modelled as the gap
    /// reported ahead of the start code. That start code is then a real byte the assembler must
    /// consume and discard, which is why the frame carries 514 timed bytes for 512 slots.
    fn frame(slots: &[u8]) -> Vec<TimedByte> {
        assert_eq!(slots.len(), DMX_CHANNELS, "a frame is 512 slots");
        let mut bytes = vec![TimedByte {
            // The break: a long idle gap, then the byte that follows it is the start code.
            byte: 0x00,
            gap_micros: BREAK_MICROS,
        }];
        for slot in slots {
            bytes.push(TimedByte {
                byte: *slot,
                gap_micros: SLOT,
            });
        }
        bytes
    }

    fn slots_with(changes: &[(usize, u8)]) -> Vec<u8> {
        let mut slots = vec![0u8; DMX_CHANNELS];
        for (channel, value) in changes {
            slots[*channel] = *value;
        }
        slots
    }

    #[test]
    fn a_whole_frame_is_reassembled_exactly() {
        let slots = slots_with(&[(0, 255), (1, 128), (511, 7)]);
        let bytes = frame(&slots);
        let mut assembler = Dmx512Assembler::new();
        let frames = assembler.push_all(&bytes);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].payload(), &slots[..]);
        assert_eq!(assembler.dropped_frames(), 0);
    }

    #[test]
    fn a_start_code_is_not_mistaken_for_a_slot() {
        // Slot 0 must read 0, not the 0x00 start code that preceded it.
        let slots = slots_with(&[(0, 0), (1, 42)]);
        let frames = Dmx512Assembler::new().push_all(&frame(&slots));
        assert_eq!(frames[0].payload()[0], 0);
        assert_eq!(frames[0].payload()[1], 42);
    }

    #[test]
    fn back_to_back_frames_are_separated_by_their_breaks() {
        let first = slots_with(&[(0, 1)]);
        let second = slots_with(&[(0, 2), (5, 9)]);
        let mut bytes = frame(&first);
        bytes.extend(frame(&second));
        let frames = Dmx512Assembler::new().push_all(&bytes);
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].payload()[0], 1);
        assert_eq!(frames[1].payload()[0], 2);
        assert_eq!(frames[1].payload()[5], 9);
    }

    #[test]
    fn a_partial_frame_is_never_reported_as_complete() {
        let slots = slots_with(&[(0, 200)]);
        let mut bytes = frame(&slots);
        bytes.truncate(bytes.len() - 100);
        let mut assembler = Dmx512Assembler::new();
        assert!(
            assembler.push_all(&bytes).is_empty(),
            "a frame missing slots must not be delivered"
        );
    }

    #[test]
    fn a_recovered_line_resyncs_on_the_next_break() {
        let mut assembler = Dmx512Assembler::new();
        let mut truncated = frame(&slots_with(&[(0, 1)]));
        truncated.truncate(truncated.len() - 50);
        assembler.push_all(&truncated);
        assert!(assembler.push_all(&truncated).is_empty());

        // The next complete frame after a desync still arrives intact.
        let good = frame(&slots_with(&[(0, 99)]));
        let frames = assembler.push_all(&good);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].payload()[0], 99);
    }

    #[test]
    fn idle_line_noise_outside_a_frame_is_dropped_and_counted() {
        let mut assembler = Dmx512Assembler::new();
        let noise: Vec<TimedByte> = (0..10)
            .map(|i| TimedByte {
                byte: i,
                gap_micros: SLOT,
            })
            .collect();
        assert!(assembler.push_all(&noise).is_empty());
        assert_eq!(assembler.dropped_bytes(), 10);
        assert_eq!(assembler.dropped_frames(), 0);
    }

    #[test]
    fn a_break_arriving_mid_frame_discards_the_partial_slots() {
        let mut assembler = Dmx512Assembler::new();
        // A frame opens but stops after a handful of slots, then the next break arrives.
        let mut truncated = frame(&slots_with(&[(0, 1)]));
        truncated.truncate(20);
        assembler.push_all(&truncated);
        assert_eq!(
            assembler.dropped_frames(),
            0,
            "not yet known to be truncated"
        );

        assembler.push_all(&frame(&slots_with(&[(0, 2)])));
        assert_eq!(
            assembler.dropped_frames(),
            1,
            "the break revealed the previous frame was cut short"
        );
    }

    #[test]
    fn an_undersized_break_does_not_start_a_frame() {
        let mut assembler = Dmx512Assembler::new();
        let mut bytes = Vec::new();
        for i in 0..DMX_CHANNELS + 8 {
            bytes.push(TimedByte {
                byte: i as u8,
                // One microsecond short of a break.
                gap_micros: BREAK_MICROS - 1,
            });
        }
        assert!(assembler.push_all(&bytes).is_empty());
        assert_eq!(assembler.dropped_bytes(), (DMX_CHANNELS + 8) as u64);
    }

    #[test]
    fn a_realistic_refresh_stream_yields_one_frame_per_break() {
        // A console refreshing at ~40 Hz for four frames, with per-slot timing throughout.
        let mut bytes = Vec::new();
        for level in 1..=4u8 {
            bytes.push(TimedByte {
                byte: 0,
                gap_micros: BREAK_MICROS + 100,
            });
            let slots = slots_with(&[(0, level * 10), (100, level)]);
            for slot in slots {
                bytes.push(TimedByte {
                    byte: slot,
                    gap_micros: if bytes.len() % 2 == 0 { MARK } else { SLOT },
                });
            }
        }
        let frames = Dmx512Assembler::new().push_all(&bytes);
        assert_eq!(frames.len(), 4);
        for (i, f) in frames.iter().enumerate() {
            let level = i as u8 + 1;
            assert_eq!(f.payload()[0], level * 10);
            assert_eq!(f.payload()[100], level);
        }
    }

    #[test]
    fn a_fake_reader_drives_the_assembler_through_its_trait() {
        /// Hands out one scripted batch of timed bytes, then nothing.
        struct FakeReader {
            batches: std::collections::VecDeque<Vec<TimedByte>>,
        }

        impl DmxSerialReader for FakeReader {
            fn read_timed(&mut self) -> Result<Vec<TimedByte>, String> {
                Ok(self.batches.pop_front().unwrap_or_default())
            }
        }

        let slots = slots_with(&[(3, 77)]);
        let mut reader = FakeReader {
            batches: std::collections::VecDeque::from(vec![
                // The frame arrives split across two reads, as a real port would deliver it.
                frame(&slots)[..100].to_vec(),
                frame(&slots)[100..].to_vec(),
            ]),
        };
        let mut assembler = Dmx512Assembler::new();
        let mut frames = Vec::new();
        for _ in 0..4 {
            let bytes = reader.read_timed().unwrap();
            frames.extend(assembler.push_all(&bytes));
        }
        assert_eq!(
            frames.len(),
            1,
            "a frame split across reads still completes"
        );
        assert_eq!(frames[0].payload()[3], 77);
    }

    #[test]
    fn opening_a_nonexistent_port_is_an_error_not_a_panic() {
        assert!(open_reader("definitely-not-a-real-com-port-xyz", 50).is_err());
        assert!(open_reader("", 50).is_err());
    }

    #[test]
    fn a_reader_opener_opens_independently_each_call() {
        let opener = reader_opener("definitely-not-a-real-com-port-xyz".into(), 50);
        assert!(opener().is_err());
        assert!(
            opener().is_err(),
            "a second call retries rather than panics"
        );
    }
}
