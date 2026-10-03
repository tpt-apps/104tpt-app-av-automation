//! Integration: real control traffic, end to end, over real sockets (spec §5.1, §7, §11, §19).
//!
//! These scenarios start the shipped binary as a service, then talk to it the way a venue's gear
//! does: OSC over UDP, Art-Net over UDP, and a MIDI 2.0 UMP stream. Nothing is stubbed — the
//! datagrams are encoded by the real codecs and parsed by the running engine — which is the only
//! way to prove a trigger fires end to end and produces an execution record.

use std::net::UdpSocket;

use tpt_app_av_automation_scenarios::{
    http_get, reserve_udp_port, wait_until, Engine, TempDir, PATIENCE, TOKEN,
};
use tpt_av_control_dmx::artnet::build_artdmx;
use tpt_av_control_midi::{Midi2ChannelVoice, Midi2Message, Ump};
use tpt_av_control_osc::OscMessage;

/// A pack triggered by OSC, Art-Net and MIDI 2.0, writing to virtual devices.
const PACK: &str = r#"
format_version: 1
name: End To End
revision: 1
rules:
  - id: osc_go
    name: OSC go
    armed: true
    trigger: { type: osc, address: /go }
    actions:
      - { id: light, type: control.dmx_channels, device: rig, universe: 1, start_channel: 0, values: [255], transport: art_net }
  - id: dimmer
    name: Art-Net level
    armed: true
    trigger: { type: dmx, universe: 1, channel: 4, comparison: greater_than, value: 100 }
    actions:
      - { id: mute, type: control.osc, device: mixer, address: /mute, args: [1] }
  - id: knob
    name: MIDI 2.0 CC
    armed: true
    trigger: { type: midi, message: control_change, channel: 0, number: 7, value: 123456 }
    actions:
      - { id: note, type: log.incident, severity: high, message: knob moved }
"#;

const DEVICES: &str = "devices:\n  - { id: rig, protocol: virtual }\n  - { id: mixer, protocol: virtual }\n";

/// Polls the API until it answers `200` for `/health`, or the timeout expires.
fn wait_for_api(addr: &str, timeout: std::time::Duration) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    while std::time::Instant::now() < deadline {
        if http_get(addr, "/health", TOKEN).0 == 200 {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    false
}

/// A running service plus the ports its listeners and API are on.
struct Fixture<'a> {
    /// Kept alive so the temporary directory outlives the process.
    _dir: &'a TempDir,
    engine: Engine,
    osc_port: u16,
    artnet_port: u16,
    ump_port: u16,
    api: String,
    state: &'a std::path::Path,
}

impl<'a> Fixture<'a> {
    /// Starts the service, retrying with fresh ports if the engine loses a port race and exits.
    fn start(name: &str) -> Fixture<'a> {
        // Leaked deliberately: the fixture is consumed by several attempts, each of which would
        // otherwise take ownership of the directory. One small directory per scenario is fine.
        let dir: &'a TempDir = Box::leak(Box::new(TempDir::new(name)));
        let rules = dir.write("pack.yaml", PACK);
        let devices = dir.write("devices.yaml", DEVICES);
        let state_dir = dir.path().join("state");
        std::fs::create_dir_all(&state_dir).unwrap();
        let state: &'a std::path::Path = Box::leak(Box::new(state_dir));

        let mut last_error = String::new();
        for attempt in 0..4 {
            // Hold all four reservations at once so the numbers are distinct from any other
            // scenario running concurrently, then release them before the engine binds them.
            let guards = [
                reserve_udp_port(),
                reserve_udp_port(),
                reserve_udp_port(),
                reserve_udp_port(),
            ];
            let osc_port = guards[0].port();
            let artnet_port = guards[1].port();
            let ump_port = guards[2].port();
            let api_port = guards[3].port();
            drop(guards);

            let config = dir.write(
                &format!("service-{attempt}.yaml"),
                &format!(
                    "tick_ms: 25\nheartbeat_interval_ms: 200\n\
                     listeners:\n\
                     \x20 - {{ protocol: osc, bind: \"127.0.0.1:{osc_port}\" }}\n\
                     \x20 - {{ protocol: artnet, bind: \"127.0.0.1:{artnet_port}\" }}\n\
                     \x20 - {{ protocol: ump, bind: \"127.0.0.1:{ump_port}\" }}\n\
                     api:\n  enabled: true\n  bind: \"127.0.0.1:{api_port}\"\n\
                     \x20 token: {TOKEN:?}\n"
                ),
            );

            let engine = Engine::start(&[
                "run",
                "--service",
                "--rules",
                rules.to_str().unwrap(),
                "--devices",
                devices.to_str().unwrap(),
                "--config",
                config.to_str().unwrap(),
                "--state-dir",
                state.to_str().unwrap(),
            ]);
            let api = format!("127.0.0.1:{api_port}");

            let fixture = Self {
                _dir: dir,
                engine,
                osc_port,
                artnet_port,
                ump_port,
                api,
                state,
            };
            // A few seconds is plenty for a local process to bind its listeners; anything slower
            // means it exited, most likely because a port was taken between reservation and bind.
            if wait_for_api(&fixture.api, std::time::Duration::from_secs(5)) {
                return fixture;
            }
            last_error = "the API never answered".into();
        }
        panic!("the engine never became ready after 4 attempts: {last_error}");
    }

    /// Sends `bytes` to `port` and waits until at least one execution has been recorded.
    fn send_and_wait(&self, port: u16, bytes: &[u8], what: &str) {
        let socket = UdpSocket::bind("127.0.0.1:0").expect("bind a sender socket");
        socket
            .send_to(bytes, ("127.0.0.1", port))
            .expect("send the datagram");
        wait_until(what, PATIENCE, || self.executions() > 0);
    }

    /// The parsed `/executions` body.
    fn executions_json(&self) -> serde_json::Value {
        let (status, body) = http_get(&self.api, "/executions", TOKEN);
        assert_eq!(
            status,
            200,
            "the API refused /executions: {}",
            body
        );
        serde_json::from_str(&body).expect("the API returned valid JSON")
    }

    /// How many executions the API currently reports.
    fn executions(&self) -> usize {
        self.executions_json()["executions"]
            .as_array()
            .map(|a| a.len())
            .unwrap_or(0)
    }

    /// The ids of the rules that have run.
    fn fired_rules(&self) -> Vec<String> {
        self.executions_json()["executions"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|e| e["rule_id"].as_str().map(str::to_string))
            .collect()
    }
}

#[test]
fn an_osc_message_arriving_over_udp_triggers_a_rule_and_is_logged() {
    let mut f = Fixture::start("e2e-osc");
    assert!(f.engine.is_running(), "the service exited before the trigger");

    let packet = OscMessage::new("/go", &[]).unwrap().encode();
    f.send_and_wait(f.osc_port, &packet, "the OSC rule to execute");

    let fired = f.fired_rules();
    assert!(
        fired.iter().any(|id| id == "osc_go"),
        "expected osc_go among {fired:?}"
    );

    // The record explains itself: trigger type, status, and that this was live, not simulated.
    let (_, body) = http_get(&f.api, "/executions?rule=osc_go", TOKEN);
    let value: serde_json::Value = serde_json::from_str(&body).expect("valid JSON");
    let record = &value["executions"][0];
    assert_eq!(record["trigger_type"], "osc");
    assert_eq!(record["overall_status"], "success");
    assert_eq!(record["simulated"], false, "the rule is armed, so this must be live");
    let action = &record["action_results"][0];
    assert_eq!(action["action_id"], "light");
    assert_eq!(action["action_type"], "control.dmx_channels");
    assert_eq!(action["outcome"]["status"], "success");
    assert_eq!(
        action["detail"], "dmx u1 from ch0 (1 values) -> rig",
        "the record should say exactly what was sent and where"
    );
    assert!(
        f.state.join("automation.db").exists(),
        "a state database should exist under {:?}",
        f.state
    );
}

#[test]
fn art_net_channel_changes_over_udp_become_dmx_events() {
    let f = Fixture::start("e2e-artnet");
    let mut frame = [0u8; 512];
    frame[4] = 200; // channel 4 (zero-based), above the rule's threshold of 100
    let packet = build_artdmx(1, 1, &frame);

    f.send_and_wait(f.artnet_port, &packet, "the Art-Net rule to execute");
    let fired = f.fired_rules();
    assert!(
        fired.iter().any(|id| id == "dimmer"),
        "expected dimmer among {fired:?}"
    );
}

#[test]
fn a_midi_2_0_ump_stream_over_udp_matches_a_32_bit_rule() {
    let f = Fixture::start("e2e-ump");
    // A 32-bit CC value; the rule names the full value, which only a UMP source can satisfy.
    let message = Midi2Message::Midi2ChannelVoice(Midi2ChannelVoice::ControlChange {
        group: 0,
        channel: 0,
        index: 7,
        value: 123_456,
    });
    let ump = Ump::from_message(&message);

    f.send_and_wait(f.ump_port, &ump.to_bytes(), "the MIDI 2.0 rule to execute");
    let fired = f.fired_rules();
    assert!(
        fired.iter().any(|id| id == "knob"),
        "expected knob among {fired:?}"
    );

    // The record carries the MIDI 2.0 detail: the group and the full 32-bit value, which is what
    // distinguishes it from a MIDI 1.0 message.
    let (_, body) = http_get(&f.api, "/executions?rule=knob", TOKEN);
    assert!(
        body.contains("\"message\":\"cc\""),
        "the trigger detail should name the MIDI message: {body}"
    );
    assert!(
        body.contains("\"value32\":123456"),
        "the record should carry the full 32-bit value: {body}"
    );
    assert!(
        body.contains("\"group\":0"),
        "the record should carry the UMP group: {body}"
    );

    // The incident it logged is persisted and exposed through the API.
    let (status, body) = http_get(&f.api, "/incidents", TOKEN);
    assert_eq!(status, 200);
    assert!(
        body.contains("knob moved"),
        "the incident should be persisted and exposed: {body}"
    );
}

#[test]
fn the_api_refuses_requests_without_a_token_even_while_traffic_flows() {
    let f = Fixture::start("e2e-auth");
    assert_eq!(
        http_get(&f.api, "/health", "not-the-token").0,
        401,
        "a wrong token must be refused"
    );
    // And the engine is unharmed by the rejected request.
    let packet = OscMessage::new("/go", &[]).unwrap().encode();
    f.send_and_wait(f.osc_port, &packet, "the rule to still fire after a rejected request");
    assert!(f.fired_rules().iter().any(|id| id == "osc_go"));
}

