//! Chaos: hostile and misbehaving peers on the wire (spec §16, §18.4).
//!
//! OSC, Art-Net, sACN and MIDI are unauthenticated, so anything on the venue network can send
//! anything at any time. These scenarios throw malformed, truncated, oversized and adversarial
//! traffic at a running engine and check the two promises that matter: the engine does not crash or
//! hang, and it keeps working normally afterwards.
//!
//! The parsers themselves are covered by unit and fuzz tests (`triggers` crate, `fuzz/`); what is
//! added here is that a *live process*, holding real state and real sockets, survives them.

use std::net::UdpSocket;
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::Duration;

use tpt_app_av_automation_scenarios::{
    http_get, reserve_udp_port, wait_until, Engine, TempDir, PATIENCE, TOKEN,
};
use tpt_av_control_dmx::artnet::build_artdmx;
use tpt_av_control_osc::OscMessage;

/// Only one engine runs at a time, so the reserved listener ports cannot collide between scenarios.
fn exclusive() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    match LOCK.get_or_init(|| Mutex::new(())).lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// A pack triggered by OSC and Art-Net, writing to virtual devices.
const PACK: &str = r#"
format_version: 1
name: Misbehaving
revision: 1
rules:
  - id: go
    name: Go
    armed: true
    trigger: { type: osc, address: /go }
    actions:
      - { id: note, type: log.incident, severity: normal, message: go fired }
  - id: lights
    name: Lights
    armed: true
    trigger: { type: dmx, universe: 1, channel: 0, comparison: greater_than, value: 100 }
    actions:
      - { id: note2, type: log.incident, severity: normal, message: lights fired }
"#;

/// A running engine with an OSC and an Art-Net listener.
struct Fixture {
    _dir: TempDir,
    engine: Engine,
    osc_port: u16,
    artnet_port: u16,
    api: String,
}

impl Fixture {
    fn start(name: &str) -> Self {
        let dir = TempDir::new(name);
        let rules = dir.write("pack.yaml", PACK);
        let devices = dir.write("devices.yaml", "devices:\n  - { id: rig, protocol: virtual }\n");
        let osc = reserve_udp_port();
        let artnet = reserve_udp_port();
        let api = reserve_udp_port();
        let osc_port = osc.port();
        let artnet_port = artnet.port();
        let api_addr = format!("127.0.0.1:{}", api.port());
        // Release the reservations before the engine binds them.
        drop(osc);
        drop(artnet);
        drop(api);

        let config = dir.write(
            "service.yaml",
            &format!(
                "tick_ms: 25\nheartbeat_interval_ms: 200\n\
                 listeners:\n\
                 \x20 - {{ protocol: osc, bind: \"127.0.0.1:{osc_port}\" }}\n\
                 \x20 - {{ protocol: artnet, bind: \"127.0.0.1:{artnet_port}\" }}\n\
                 api:\n  enabled: true\n  bind: \"{api_addr}\"\n  token: {TOKEN:?}\n"
            ),
        );
        let state = dir.path().join("state");
        std::fs::create_dir_all(&state).unwrap();

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
        wait_until("the API to answer", PATIENCE, || {
            http_get(&api_addr, "/health", TOKEN).0 == 200
        });
        Self {
            _dir: dir,
            engine,
            osc_port,
            artnet_port,
            api: api_addr,
        }
    }

    /// Sends raw bytes to a listener.
    fn send(&self, port: u16, bytes: &[u8]) {
        let socket = UdpSocket::bind("127.0.0.1:0").expect("bind a sender");
        let _ = socket.send_to(bytes, ("127.0.0.1", port));
    }

    /// Sends raw bytes and confirms the engine is still alive and answering afterwards.
    fn abuse(&mut self, port: u16, bytes: &[u8], what: &str) {
        self.send(port, bytes);
        std::thread::sleep(Duration::from_millis(50));
        assert!(self.engine.is_running(), "the engine died after {what}");
        assert_eq!(
            http_get(&self.api, "/health", TOKEN).0,
            200,
            "the API stopped answering after {what}"
        );
    }

    /// How many incidents the API currently reports.
    fn incidents(&self) -> usize {
        http_get(&self.api, "/incidents", TOKEN)
            .1
            .matches("fired")
            .count()
    }

    /// Confirms a legitimate OSC message still fires its rule.
    fn expect_go_to_fire(&mut self, what: &str) {
        let before = self.incidents();
        self.send(
            self.osc_port,
            &OscMessage::new("/go", &[]).unwrap().encode(),
        );
        wait_until(&format!("the rule to fire after {what}"), PATIENCE, || {
            self.incidents() > before
        });
    }
}

/// A burst of malformed OSC datagrams must not crash or hang the engine.
#[test]
fn malformed_osc_cannot_bring_the_engine_down() {
    let _serialized = exclusive();
    let mut f = Fixture::start("chaos-osc-garbage");

    let cases: Vec<Vec<u8>> = vec![
        vec![],
        vec![0x00],
        // An address with no type tag and no null terminator.
        b"/go".to_vec(),
        // A type tag with no data behind it.
        [b"/go\0\0".as_slice(), &[0x2c]].concat(),
        // A type tag that does not exist.
        [b"/go\0\0".as_slice(), &[0x7f, 0x00]].concat(),
        // A string length that runs past the end of the datagram.
        [b"/go\0\0".as_slice(), &[0x2c, 0xff, 0xff, 0x00, 0x00]].concat(),
        // A 64 KiB datagram, far past the size limit.
        vec![0xff; 65_536],
        // Pseudo-random bytes.
        (0u16..1024).map(|i| (i % 251) as u8).collect(),
    ];

    for bytes in &cases {
        f.abuse(f.osc_port, bytes, "malformed OSC");
    }

    // And it still does its actual job.
    f.expect_go_to_fire("malformed OSC traffic");
}

/// Truncated and hostile Art-Net datagrams must not bring the engine down.
#[test]
fn malformed_art_net_cannot_bring_the_engine_down() {
    let _serialized = exclusive();
    let mut f = Fixture::start("chaos-artnet-garbage");

    let mut frame = [0u8; 512];
    frame[0] = 255;
    let valid = build_artdmx(1, 1, &frame);

    let mut wrong_length = valid.clone();
    wrong_length[14] = 0xff;
    wrong_length[15] = 0xff;

    let cases: Vec<Vec<u8>> = vec![
        vec![],
        valid[..4].to_vec(),
        valid[..15].to_vec(),
        // Correct header, wrong length field.
        wrong_length,
        // Not Art-Net at all.
        b"this is not art-net".to_vec(),
        vec![0u8; 2048],
        // A well-formed packet naming an enormous universe, which must be bounded.
        build_artdmx(60_000, 1, &frame),
    ];

    for bytes in &cases {
        f.abuse(f.artnet_port, bytes, "malformed Art-Net");
    }

    // A genuine Art-Net frame still triggers the DMX rule.
    let before = f.incidents();
    f.send(f.artnet_port, &build_artdmx(1, 2, &frame));
    wait_until("the Art-Net rule to fire", PATIENCE, || f.incidents() > before);
}

/// Sustained flooding from one source must be absorbed, not allowed to exhaust the engine.
#[test]
fn a_flood_from_one_source_is_absorbed_and_the_engine_stays_responsive() {
    let _serialized = exclusive();
    let mut f = Fixture::start("chaos-flood");

    let socket = UdpSocket::bind("127.0.0.1:0").expect("bind a sender");
    let packet = OscMessage::new("/go", &[]).unwrap().encode();
    for _ in 0..5_000 {
        let _ = socket.send_to(&packet, ("127.0.0.1", f.osc_port));
    }

    assert!(f.engine.is_running(), "the engine died under a flood");
    assert_eq!(
        http_get(&f.api, "/health", TOKEN).0,
        200,
        "the API should still answer after a flood"
    );

    // Health reports the rejected traffic rather than pretending it was all accepted.
    let (_, health) = http_get(&f.api, "/health", TOKEN);
    let value: serde_json::Value = serde_json::from_str(&health).expect("valid JSON");
    let rejected = &value["rejected_inbound"];
    let total = rejected["malformed"].as_u64().unwrap_or(0)
        + rejected["rate_limited"].as_u64().unwrap_or(0)
        + rejected["queue_dropped"].as_u64().unwrap_or(0);
    assert!(
        total > 0,
        "a flood should be visibly rate-limited or rejected, not silently absorbed: {value}"
    );

    // Once a source has been muted, its traffic stops reaching the engine — that is the point of the
    // adaptive backoff, so this flooder is not expected to fire a rule, and is not treated as one.
    let before = f.incidents();
    let packet = OscMessage::new("/go", &[]).unwrap().encode();
    for _ in 0..200 {
        f.send(f.osc_port, &packet);
    }
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(
        f.incidents(),
        before,
        "a muted source should be ignored rather than processed"
    );
}
