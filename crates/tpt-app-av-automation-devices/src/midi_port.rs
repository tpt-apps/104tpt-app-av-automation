//! Real MIDI ports through `midir`, opened by name.
//!
//! `tpt-av-control-midi` also wraps `midir`, but its `enumerate_devices`/`open_output` pair hands out
//! port ids counted across inputs *and* outputs while `open_output` indexes outputs only, so ids of
//! output ports do not line up. Resolving ports by name here sidesteps that and is what an operator
//! writes in `devices.yaml` anyway (`address: "USB MIDI"`). Encoding and parsing still use the
//! `tpt-av-control-midi` codecs.
//!
//! **Not exercised against hardware in the automated tests** — CI has no MIDI interface. The
//! reconnect logic is covered with a fake opener (see `net.rs`); opening a real port is covered only
//! for the "no such port" path.

use crate::net::{MidiOpener, MidiWriter};

const CLIENT: &str = "tpt-av-automation";

struct MidirWriter(midir::MidiOutputConnection);

impl MidiWriter for MidirWriter {
    fn write(&mut self, bytes: &[u8]) -> Result<(), String> {
        self.0.send(bytes).map_err(|e| e.to_string())
    }
}

fn matches(name: &str, fragment: &str) -> bool {
    name.to_lowercase()
        .contains(&fragment.trim().to_lowercase())
}

/// Names of the MIDI output ports currently present.
pub fn output_port_names() -> Vec<String> {
    let Ok(out) = midir::MidiOutput::new(CLIENT) else {
        return Vec::new();
    };
    out.ports()
        .iter()
        .filter_map(|p| out.port_name(p).ok())
        .collect()
}

/// Names of the MIDI input ports currently present.
pub fn input_port_names() -> Vec<String> {
    let Ok(input) = midir::MidiInput::new(CLIENT) else {
        return Vec::new();
    };
    input
        .ports()
        .iter()
        .filter_map(|p| input.port_name(p).ok())
        .collect()
}

/// Connects to the first output port whose name contains `fragment` (case-insensitive).
pub fn open_output(fragment: &str) -> Result<Box<dyn MidiWriter>, String> {
    if fragment.trim().is_empty() {
        return Err("no MIDI port name configured".into());
    }
    let out = midir::MidiOutput::new(CLIENT).map_err(|e| format!("MIDI is unavailable: {e}"))?;
    let port = out
        .ports()
        .into_iter()
        .find(|p| out.port_name(p).is_ok_and(|n| matches(&n, fragment)))
        .ok_or_else(|| format!("no MIDI output port matching `{fragment}`"))?;
    let connection = out
        .connect(&port, CLIENT)
        .map_err(|e| format!("cannot open MIDI output `{fragment}`: {e}"))?;
    Ok(Box::new(MidirWriter(connection)))
}

/// An opener that connects to the named output port each time it is called.
pub fn output_opener(fragment: String) -> MidiOpener {
    Box::new(move || open_output(&fragment))
}

/// An open MIDI input; dropping it closes the port.
pub struct MidiInputHandle {
    _connection: midir::MidiInputConnection<()>,
}

/// Connects to the first input port whose name contains `fragment` and calls `on_message` with the
/// raw bytes of every message, on midir's own thread.
pub fn open_input(
    fragment: &str,
    mut on_message: impl FnMut(&[u8]) + Send + 'static,
) -> Result<MidiInputHandle, String> {
    if fragment.trim().is_empty() {
        return Err("no MIDI port name configured".into());
    }
    let input = midir::MidiInput::new(CLIENT).map_err(|e| format!("MIDI is unavailable: {e}"))?;
    let port = input
        .ports()
        .into_iter()
        .find(|p| input.port_name(p).is_ok_and(|n| matches(&n, fragment)))
        .ok_or_else(|| format!("no MIDI input port matching `{fragment}`"))?;
    let connection = input
        .connect(&port, CLIENT, move |_, bytes, _| on_message(bytes), ())
        .map_err(|e| format!("cannot open MIDI input `{fragment}`: {e}"))?;
    Ok(MidiInputHandle {
        _connection: connection,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fragments_match_case_insensitively() {
        assert!(matches("USB MIDI Interface 1", "usb midi"));
        assert!(matches("Microsoft GS Wavetable Synth", " wavetable "));
        assert!(!matches("Keystation 49", "launchpad"));
    }

    #[test]
    fn a_missing_or_blank_port_is_an_error_not_a_panic() {
        assert!(open_output("definitely-not-a-real-midi-port-xyz").is_err());
        assert!(open_output("   ").is_err());
        assert!(open_input("definitely-not-a-real-midi-port-xyz", |_| {}).is_err());
        assert!(open_input("", |_| {}).is_err());
    }

    #[test]
    fn port_listing_never_panics() {
        let _ = output_port_names();
        let _ = input_port_names();
    }

    /// Real-port round trip. Opt in with `TPT_MIDI_LOOPBACK_OUT` / `TPT_MIDI_LOOPBACK_IN` (name
    /// fragments of two ports joined by a loopback driver) and `--ignored`; CI has no MIDI ports.
    #[test]
    #[ignore = "needs a MIDI loopback pair; set TPT_MIDI_LOOPBACK_OUT and TPT_MIDI_LOOPBACK_IN"]
    fn a_message_survives_a_real_loopback_port_pair() {
        use std::sync::mpsc;
        use std::time::Duration;
        println!("outputs: {:?}", output_port_names());
        println!("inputs:  {:?}", input_port_names());
        let (Ok(out), Ok(inp)) = (
            std::env::var("TPT_MIDI_LOOPBACK_OUT"),
            std::env::var("TPT_MIDI_LOOPBACK_IN"),
        ) else {
            panic!("set TPT_MIDI_LOOPBACK_OUT and TPT_MIDI_LOOPBACK_IN");
        };
        let (tx, rx) = mpsc::channel();
        let _input = open_input(&inp, move |bytes| {
            let _ = tx.send(bytes.to_vec());
        })
        .expect("open input");
        let mut writer = open_output(&out).expect("open output");
        writer.write(&[0x90, 60, 100]).expect("write");
        let got = rx
            .recv_timeout(Duration::from_secs(3))
            .expect("no message came back");
        assert_eq!(got, vec![0x90, 60, 100]);
    }

    /// Sends a real note to a real output port (e.g. `TPT_MIDI_OUT=Wavetable` on Windows, which
    /// sounds the built-in synth). Opt in with `--ignored`.
    #[test]
    #[ignore = "needs a MIDI output port; set TPT_MIDI_OUT to a name fragment"]
    fn a_real_output_port_accepts_messages_and_reports_a_missing_one() {
        let name = std::env::var("TPT_MIDI_OUT").expect("set TPT_MIDI_OUT");
        let mut writer = open_output(&name).expect("open output");
        writer.write(&[0x90, 60, 100]).expect("note on");
        std::thread::sleep(std::time::Duration::from_millis(300));
        writer.write(&[0x80, 60, 0]).expect("note off");
        assert!(open_output("no-such-port-xyz").is_err());
    }
}
