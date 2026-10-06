//! The service running for real: sockets, persistence and restarts (spec §11, §14, §16).

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream, UdpSocket};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde_json::Value;
use tpt_app_av_automation_core::{Clock, FixedClock, SystemClock, Timestamp};
use tpt_app_av_automation_devices::DeviceFile;
use tpt_app_av_automation_model::RulePack;
use tpt_app_av_automation_report::Filter;
use tpt_app_av_automation_service::{ApiConfig, Service, ServiceConfig, ServiceHandle, Store};
use tpt_av_control_midi::{Midi2ChannelVoice, Midi2Message, Ump};
use tpt_av_control_osc::{OscArg, OscMessage};

const TOKEN: &str = "0123456789abcdef-test-token";
const T0: u64 = 1_704_067_200_000; // Monday 2024-01-01 00:00 UTC

const PACK: &str = r#"
format_version: 1
name: Service Test
revision: 1
rules:
  - id: show
    name: Show
    armed: false
    trigger: { type: osc, address: /go }
    actions:
      - { id: power, type: control.osc, device: proj, address: /power, args: [1] }
      - { id: note, type: notify.operator, message: hello }
"#;

const DEVICES: &str = "devices:\n  - { id: proj, protocol: virtual }\n";

fn config() -> ServiceConfig {
    ServiceConfig {
        tick_ms: 10,
        heartbeat_interval_ms: 50,
        api: ApiConfig {
            enabled: true,
            bind: "127.0.0.1:0".to_string(),
            token: Some(TOKEN.to_string()),
        },
        ..ServiceConfig::default()
    }
}

struct Running {
    handle: ServiceHandle,
    thread: Option<JoinHandle<()>>,
}

impl Running {
    fn start(service: Service) -> Self {
        let handle = service.handle();
        let thread = std::thread::spawn(move || service.run().expect("service run"));
        assert!(
            handle.wait_ready(Duration::from_secs(10)),
            "service did not become ready"
        );
        Self {
            handle,
            thread: Some(thread),
        }
    }

    fn stop(&mut self) {
        self.handle.shutdown();
        if let Some(t) = self.thread.take() {
            t.join().unwrap();
        }
    }

    fn api(&self) -> SocketAddr {
        self.handle.api_addr().expect("API should be running")
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        self.stop();
    }
}

fn build(
    pack: &str,
    config: ServiceConfig,
    store: Option<Store>,
    clock: Arc<dyn Clock>,
) -> Service {
    Service::build(
        RulePack::from_yaml_str(pack).unwrap(),
        DeviceFile::from_yaml_str(DEVICES).unwrap(),
        config,
        store,
        clock,
    )
    .expect("service should build")
}

fn wait_until(what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if condition() {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("timed out waiting for {what}");
}

fn http(addr: SocketAddr, method: &str, path: &str, headers: &[(&str, &str)]) -> (u16, Value) {
    let mut stream = TcpStream::connect(addr).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut request = format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\n");
    for (k, v) in headers {
        request.push_str(&format!("{k}: {v}\r\n"));
    }
    request.push_str("\r\n");
    stream.write_all(request.as_bytes()).unwrap();
    let mut raw = String::new();
    stream.read_to_string(&mut raw).unwrap();
    let status: u16 = raw.split(' ').nth(1).unwrap().parse().unwrap();
    let body = raw.split("\r\n\r\n").nth(1).unwrap_or("");
    (status, serde_json::from_str(body).unwrap_or(Value::Null))
}

fn authed(addr: SocketAddr, method: &str, path: &str) -> (u16, Value) {
    http(
        addr,
        method,
        path,
        &[("Authorization", &format!("Bearer {TOKEN}"))],
    )
}

#[test]
fn health_reports_the_listener_threads_it_starts() {
    let mut c = config();
    let osc_port = free_udp_port();
    c.listeners
        .push(tpt_app_av_automation_service::ListenerConfig {
            protocol: "osc".into(),
            bind: format!("127.0.0.1:{osc_port}"),
        });
    let running = Running::start(build(PACK, c, None, Arc::new(SystemClock)));
    let addr = running.handle.api_addr().expect("api address");

    // The OSC listener and the API worker are both counted, and neither has been lost.
    let (_, body) = authed(addr, "GET", "/health");
    assert_eq!(body["listeners"]["expected"], 2);
    assert_eq!(body["listeners"]["live"], 2);
    assert_eq!(body["listeners"]["lost"], 0);
    assert_eq!(body["status"], "nominal");
}

fn osc_packet(address: &str, args: &[OscArg]) -> Vec<u8> {
    OscMessage::new(address, args).unwrap().encode()
}

fn free_udp_port() -> u16 {
    UdpSocket::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

#[test]
fn the_api_is_off_unless_enabled() {
    let mut c = config();
    c.api.enabled = false;
    let running = Running::start(build(PACK, c, None, Arc::new(SystemClock)));
    assert!(running.handle.api_addr().is_none());
}

#[test]
fn every_request_needs_the_token() {
    let running = Running::start(build(PACK, config(), None, Arc::new(SystemClock)));
    let addr = running.api();
    assert_eq!(http(addr, "GET", "/rules", &[]).0, 401);
    assert_eq!(
        http(
            addr,
            "GET",
            "/rules",
            &[("Authorization", "Bearer wrong-token-value")]
        )
        .0,
        401
    );
    assert_eq!(
        http(addr, "GET", "/rules", &[("Authorization", TOKEN)]).0,
        401,
        "scheme is required"
    );
    for path in ["/health", "/rules", "/devices", "/executions"] {
        assert_eq!(authed(addr, "GET", path).0, 200, "{path}");
    }
    assert_eq!(authed(addr, "GET", "/nope").0, 404);
    assert_eq!(authed(addr, "DELETE", "/rules").0, 405);
    assert_eq!(authed(addr, "GET", "/rules/show/arm").0, 405);
}

#[test]
fn foreign_hosts_and_origins_are_refused_even_with_a_valid_token() {
    let running = Running::start(build(PACK, config(), None, Arc::new(SystemClock)));
    let addr = running.api();
    let auth = format!("Bearer {TOKEN}");
    let mut stream = TcpStream::connect(addr).unwrap();
    stream
        .write_all(
            format!("GET /rules HTTP/1.1\r\nHost: evil.example\r\nAuthorization: {auth}\r\n\r\n")
                .as_bytes(),
        )
        .unwrap();
    let mut raw = String::new();
    stream.read_to_string(&mut raw).unwrap();
    assert!(raw.starts_with("HTTP/1.1 403"), "{raw}");
    let (status, _) = http(
        addr,
        "GET",
        "/rules",
        &[("Authorization", &auth), ("Origin", "https://evil.example")],
    );
    assert_eq!(status, 403);
}

#[test]
fn malformed_http_gets_an_error_not_a_crash() {
    let running = Running::start(build(PACK, config(), None, Arc::new(SystemClock)));
    let addr = running.api();
    for garbage in [
        &b"\x00\x01\x02\r\n\r\n"[..],
        b"GET\r\n\r\n",
        b"POST / HTTP/1.1\r\nContent-Length: 99999999\r\n\r\n",
        &[0xff; 5000],
    ] {
        let mut stream = TcpStream::connect(addr).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let _ = stream.write_all(garbage);
        let mut raw = Vec::new();
        let _ = stream.read_to_end(&mut raw);
    }
    assert_eq!(authed(addr, "GET", "/health").0, 200, "still serving");
}

#[test]
fn rules_show_their_armed_state_and_can_be_armed_disarmed_and_run() {
    let running = Running::start(build(PACK, config(), None, Arc::new(SystemClock)));
    let addr = running.api();

    let (_, rules) = authed(addr, "GET", "/rules");
    assert_eq!(rules["rules"][0]["id"], "show");
    assert_eq!(rules["rules"][0]["armed"], false);

    // Disarmed: running it simulates.
    let (status, run) = authed(addr, "POST", "/rules/show/run");
    assert_eq!(status, 200);
    assert_eq!(run["executions"][0]["simulated"], true);
    assert_eq!(run["executions"][0]["status"], "success");

    assert_eq!(authed(addr, "POST", "/rules/show/arm").1["armed"], true);
    wait_until("armed state to be visible", || {
        running.handle.snapshot().rules[0].armed
    });
    let (_, run) = authed(addr, "POST", "/rules/show/run");
    assert_eq!(run["executions"][0]["simulated"], false);
    assert_eq!(run["executions"][0]["actions_executed"], 2);

    assert_eq!(authed(addr, "POST", "/rules/show/disarm").1["armed"], false);
    assert_eq!(authed(addr, "POST", "/rules/ghost/run").0, 404);
    assert_eq!(authed(addr, "POST", "/rules/ghost/arm").0, 404);

    let (_, all) = authed(addr, "GET", "/executions");
    assert_eq!(all["executions"].as_array().unwrap().len(), 2);
    let (_, live) = authed(addr, "GET", "/executions?simulated=false");
    assert_eq!(live["executions"].as_array().unwrap().len(), 1);
    assert_eq!(authed(addr, "GET", "/executions?status=bogus").0, 400);
}

#[test]
fn health_reports_mode_devices_and_rejected_traffic() {
    let running = Running::start(build(PACK, config(), None, Arc::new(SystemClock)));
    wait_until("the device to come online", || {
        authed(running.api(), "GET", "/health").1["devices"]["online"] == 1
    });
    let (_, health) = authed(running.api(), "GET", "/health");
    assert_eq!(health["status"], "nominal");
    assert_eq!(health["mode"], "live");
    assert_eq!(health["rules"], 1);
    assert_eq!(health["armed_rules"], 0);
}

#[test]
fn simulate_mode_services_never_run_live() {
    let mut c = config();
    c.simulate = true;
    let running = Running::start(build(
        &PACK.replace("armed: false", "armed: true"),
        c,
        None,
        Arc::new(SystemClock),
    ));
    let (_, run) = authed(running.api(), "POST", "/rules/show/run");
    assert_eq!(run["executions"][0]["simulated"], true);
    assert_eq!(
        authed(running.api(), "GET", "/health").1["mode"],
        "simulation"
    );
}

#[test]
fn the_event_stream_delivers_executions_over_websocket() {
    let running = Running::start(build(PACK, config(), None, Arc::new(SystemClock)));
    let addr = running.api();
    let mut stream = TcpStream::connect(addr).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    stream
        .write_all(
            format!(
                "GET /events HTTP/1.1\r\nHost: 127.0.0.1\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nAuthorization: Bearer {TOKEN}\r\n\r\n"
            )
            .as_bytes(),
        )
        .unwrap();
    // Read the handshake response.
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        stream.read_exact(&mut byte).unwrap();
        head.push(byte[0]);
    }
    let head = String::from_utf8(head).unwrap();
    assert!(head.starts_with("HTTP/1.1 101"), "{head}");
    assert!(head.contains("s3pPLMBiTxaQ9kYGzzhZRbK+xOo="), "{head}");

    authed(addr, "POST", "/rules/show/run");
    let mut header = [0u8; 2];
    stream.read_exact(&mut header).unwrap();
    assert_eq!(header[0], 0x81);
    let len = match header[1] {
        126 => {
            let mut l = [0u8; 2];
            stream.read_exact(&mut l).unwrap();
            usize::from(u16::from_be_bytes(l))
        }
        n => usize::from(n),
    };
    let mut payload = vec![0u8; len];
    stream.read_exact(&mut payload).unwrap();
    let record: Value = serde_json::from_slice(&payload).unwrap();
    assert_eq!(record["rule_id"], "show");
    assert_eq!(record["overall_status"], "success");

    // Without the token the upgrade is refused.
    let (status, _) = http(
        addr,
        "GET",
        "/events",
        &[
            ("Upgrade", "websocket"),
            ("Connection", "Upgrade"),
            ("Sec-WebSocket-Version", "13"),
            ("Sec-WebSocket-Key", "dGhlIHNhbXBsZSBub25jZQ=="),
        ],
    );
    assert_eq!(status, 401);
}

#[test]
fn an_inbound_osc_message_triggers_a_rule_end_to_end() {
    let port = free_udp_port();
    let mut c = config();
    c.listeners
        .push(tpt_app_av_automation_service::ListenerConfig {
            protocol: "osc".into(),
            bind: format!("127.0.0.1:{port}"),
        });
    let running = Running::start(build(
        &PACK.replace("armed: false", "armed: true"),
        c,
        None,
        Arc::new(SystemClock),
    ));
    let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
    sender
        .send_to(&osc_packet("/go", &[OscArg::Int(1)]), ("127.0.0.1", port))
        .unwrap();
    wait_until("the OSC message to fire the rule", || {
        !running.handle.snapshot().recent.is_empty()
    });
    let record = running.handle.snapshot().recent[0].clone();
    assert_eq!(record.rule_id.as_str(), "show");
    assert!(!record.simulated);
    assert_eq!(record.actions_succeeded(), 2);
}

#[test]
fn malformed_inbound_traffic_cannot_crash_the_engine() {
    let port = free_udp_port();
    let mut c = config();
    c.listeners
        .push(tpt_app_av_automation_service::ListenerConfig {
            protocol: "osc".into(),
            bind: format!("127.0.0.1:{port}"),
        });
    let running = Running::start(build(
        &PACK.replace("armed: false", "armed: true"),
        c,
        None,
        Arc::new(SystemClock),
    ));
    let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
    let target = ("127.0.0.1", port);
    let junk: Vec<Vec<u8>> = vec![
        vec![],
        vec![0],
        b"not osc".to_vec(),
        b"#bundle\0\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff".to_vec(),
        vec![b'/'; 6000],
        (0..=255u8).collect(),
    ];
    for packet in &junk {
        let _ = sender.send_to(packet, target);
    }
    wait_until("garbage to be counted", || {
        running.handle.rejected_counts().0 >= 4
    });
    // The engine is alive and still reacts to a valid message afterwards.
    sender.send_to(&osc_packet("/go", &[]), target).unwrap();
    wait_until("a valid message to fire after the garbage", || {
        !running.handle.snapshot().recent.is_empty()
    });
    assert_eq!(authed(running.api(), "GET", "/health").0, 200);
}

#[test]
fn flooding_sources_are_rate_limited() {
    let port = free_udp_port();
    let mut c = config();
    c.listeners
        .push(tpt_app_av_automation_service::ListenerConfig {
            protocol: "osc".into(),
            bind: format!("127.0.0.1:{port}"),
        });
    let running = Running::start(build(PACK, c, None, Arc::new(SystemClock)));
    let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
    let packet = osc_packet("/flood", &[]);
    // Keep flooding until the limiter engages: the listener binds on its own thread, so a single
    // burst sent before it is ready is lost to the kernel and would never be counted.
    wait_until("the limiter to engage", || {
        for _ in 0..3000 {
            let _ = sender.send_to(&packet, ("127.0.0.1", port));
        }
        running.handle.rejected_counts().1 > 0
    });
}

#[test]
fn a_one_shot_schedule_does_not_refire_after_a_restart() {
    let dir = std::env::temp_dir().join(format!("tpt-av-restart-{}", std::process::id()));
    let db = dir.join("state.db");
    let pack = r#"
format_version: 1
name: Once
revision: 1
rules:
  - id: opening
    name: Opening
    armed: true
    trigger: { type: schedule, at: "18:55", once: true }
    actions:
      - { id: note, type: notify.operator, message: doors open }
"#;
    let clock = FixedClock::new(Timestamp::from_millis(T0));
    let mut c = config();
    c.api.enabled = false;

    clock.advance_millis((18 * 60 + 55) * 60_000);
    let mut first = Running::start(build(
        pack,
        c.clone(),
        Some(Store::open(&db).unwrap()),
        Arc::new(clock.clone()),
    ));
    wait_until("the one-shot to fire", || {
        !first.handle.snapshot().recent.is_empty()
    });
    first.stop(); // "crash": the process ends, state lives only in SQLite

    // A new process starts during the same minute...
    clock.advance_millis(20_000);
    let mut second = Running::start(build(
        pack,
        c.clone(),
        Some(Store::open(&db).unwrap()),
        Arc::new(clock.clone()),
    ));
    std::thread::sleep(Duration::from_millis(400));
    assert!(
        second.handle.snapshot().recent.is_empty(),
        "the one-shot must not fire twice"
    );
    second.stop();

    let store = Store::open(&db).unwrap();
    assert_eq!(
        store.executions(&Filter::default(), 10).unwrap().len(),
        1,
        "exactly one execution was recorded"
    );
    drop(store);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn executions_and_pack_versions_are_persisted() {
    let dir = std::env::temp_dir().join(format!("tpt-av-persist-{}", std::process::id()));
    let db = dir.join("state.db");
    let mut c = config();
    c.api.enabled = false;
    let mut running = Running::start(build(
        PACK,
        c,
        Some(Store::open(&db).unwrap()),
        Arc::new(SystemClock),
    ));
    running
        .handle
        .control(tpt_app_av_automation_service::ControlRequest::Run(
            "show".into(),
        ))
        .unwrap();
    running
        .handle
        .control(tpt_app_av_automation_service::ControlRequest::Arm(
            "show".into(),
        ))
        .unwrap();
    running.stop();

    let store = Store::open(&db).unwrap();
    assert_eq!(store.executions(&Filter::default(), 10).unwrap().len(), 1);
    assert_eq!(store.pack_history().unwrap().len(), 1);
    assert_eq!(store.load_devices().unwrap().devices.len(), 1);
    let state = store
        .load_state()
        .unwrap()
        .expect("engine state saved on shutdown");
    assert_eq!(
        state.armed.get("show"),
        Some(&true),
        "arming survives a restart"
    );
    assert_eq!(state.next_execution, 2);
    drop(store);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn unsafe_or_inconsistent_configuration_is_refused_at_build_time() {
    let devices = || DeviceFile::from_yaml_str(DEVICES).unwrap();
    let pack = || RulePack::from_yaml_str(PACK).unwrap();
    let clock = || -> Arc<dyn Clock> { Arc::new(SystemClock) };

    let mut no_token = config();
    no_token.api.token = None;
    assert!(Service::build(pack(), devices(), no_token, None, clock()).is_err());

    let mut external = config();
    external.api.bind = "0.0.0.0:8787".into();
    assert!(Service::build(pack(), devices(), external, None, clock()).is_err());

    let no_devices = DeviceFile::default();
    assert!(
        Service::build(pack(), no_devices, config(), None, clock()).is_err(),
        "a rule targeting an undefined device is rejected before anything runs"
    );
}

fn build_with_devices(pack: &str, devices: &str, config: ServiceConfig) -> Service {
    Service::build(
        RulePack::from_yaml_str(pack).unwrap(),
        DeviceFile::from_yaml_str(devices).unwrap(),
        config,
        None,
        Arc::new(SystemClock),
    )
    .expect("service should build")
}

fn listener(protocol: &str, bind: String) -> tpt_app_av_automation_service::ListenerConfig {
    tpt_app_av_automation_service::ListenerConfig {
        protocol: protocol.into(),
        bind,
    }
}

#[test]
fn incoming_dmx_over_artnet_triggers_an_outbound_osc_message() {
    let receiver = UdpSocket::bind("127.0.0.1:0").unwrap();
    receiver
        .set_read_timeout(Some(Duration::from_millis(200)))
        .unwrap();
    let device_port = receiver.local_addr().unwrap().port();
    let artnet_port = free_udp_port();
    let pack = r#"
format_version: 1
name: Dmx To Osc
revision: 1
rules:
  - id: fader
    name: Fader to cue
    armed: true
    trigger: { type: dmx, universe: 1, channel: 3, comparison: equals, value: 200 }
    actions:
      - { id: cue, type: control.osc, device: show, address: /cue/go, args: [1] }
"#;
    let mut c = config();
    c.api.enabled = false;
    c.listeners
        .push(listener("artnet", format!("127.0.0.1:{artnet_port}")));
    let running = Running::start(build_with_devices(
        pack,
        &format!(
            "devices:\n  - {{ id: show, protocol: osc, address: '127.0.0.1:{device_port}' }}\n"
        ),
        c,
    ));

    let mut frame = [0u8; 512];
    frame[3] = 200;
    let datagram = tpt_av_control_dmx::artnet::build_artdmx(1, 0, &frame);
    let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
    let mut received = None;
    let mut buf = [0u8; 512];
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline && received.is_none() {
        let _ = sender.send_to(&datagram, ("127.0.0.1", artnet_port));
        if let Ok(n) = receiver.recv(&mut buf) {
            received = Some(buf[..n].to_vec());
        }
        // Repeating the identical frame must not re-trigger: only the first change counts.
    }
    let message = OscMessage::decode(&received.expect("no OSC message arrived")).unwrap();
    assert_eq!(message.address, "/cue/go");
    drop(running);
}

#[test]
fn an_inbound_osc_message_drives_an_outbound_artnet_universe() {
    let receiver = UdpSocket::bind("127.0.0.1:0").unwrap();
    receiver
        .set_read_timeout(Some(Duration::from_millis(200)))
        .unwrap();
    let node_port = receiver.local_addr().unwrap().port();
    let osc_port = free_udp_port();
    let pack = r#"
format_version: 1
name: Osc To Dmx
revision: 1
rules:
  - id: blackout
    name: Blackout
    armed: true
    trigger: { type: osc, address: /blackout }
    actions:
      - { id: levels, type: control.dmx_channels, device: node, universe: 4, start_channel: 0, values: [9, 8, 7], transport: art_net }
"#;
    let mut c = config();
    c.api.enabled = false;
    c.listeners
        .push(listener("osc", format!("127.0.0.1:{osc_port}")));
    let running = Running::start(build_with_devices(
        pack,
        &format!(
            "devices:\n  - {{ id: node, protocol: artnet, address: '127.0.0.1:{node_port}' }}\n"
        ),
        c,
    ));

    let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
    let go = osc_packet("/blackout", &[]);
    let mut slots = None;
    let mut buf = [0u8; 1024];
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline && slots.is_none() {
        let _ = sender.send_to(&go, ("127.0.0.1", osc_port));
        if let Ok(n) = receiver.recv(&mut buf) {
            if let Ok(tpt_av_control_dmx::artnet::ArtNetPacket::Dmx { universe, slots: s }) =
                tpt_av_control_dmx::artnet::parse_packet(&buf[..n])
            {
                assert_eq!(universe, 4);
                slots = Some(s);
            }
        }
    }
    assert_eq!(&slots.expect("no Art-Net frame arrived")[..3], &[9, 8, 7]);
    drop(running);
}

#[test]
fn a_midi_listener_for_an_absent_port_does_not_stop_the_service() {
    let mut c = config();
    c.api.enabled = false;
    c.listeners.push(listener(
        "midi",
        "definitely-not-a-real-midi-port-xyz".into(),
    ));
    let mut running = Running::start(build(PACK, c, None, Arc::new(SystemClock)));
    std::thread::sleep(Duration::from_millis(300));
    assert!(running
        .handle
        .control(tpt_app_av_automation_service::ControlRequest::Run(
            "show".into()
        ))
        .is_ok());
    running.stop();
}

#[test]
fn incidents_are_persisted_and_exposed_over_the_api() {
    let dir = std::env::temp_dir().join(format!("tpt-av-incidents-{}", std::process::id()));
    let db = dir.join("state.db");
    let pack = r#"
format_version: 1
name: Incidents
revision: 1
rules:
  - id: alarm
    name: Alarm
    armed: true
    trigger: { type: manual }
    actions:
      - { id: log, type: log.incident, severity: critical, message: "lamp failure" }
"#;
    let mut running = Running::start(
        Service::build(
            RulePack::from_yaml_str(pack).unwrap(),
            DeviceFile::default(),
            config(),
            Some(Store::open(&db).unwrap()),
            Arc::new(SystemClock),
        )
        .unwrap(),
    );
    authed(running.api(), "POST", "/rules/alarm/run");
    let (status, body) = authed(running.api(), "GET", "/incidents");
    assert_eq!(status, 200);
    assert_eq!(body["incidents"][0]["message"], "lamp failure");
    assert_eq!(body["incidents"][0]["severity"], "critical");
    assert_eq!(body["incidents"][0]["rule_id"], "alarm");
    running.stop();

    // Still there after the process is gone.
    let store = Store::open(&db).unwrap();
    assert_eq!(store.incidents(10).unwrap().len(), 1);
    drop(store);
    let _ = std::fs::remove_dir_all(dir);
}

/// A pack triggered by a full 32-bit MIDI 2.0 control change on UMP group 2.
const UMP_PACK: &str = r#"
format_version: 1
name: UMP
revision: 1
rules:
  - id: cc-7
    name: CC 7 on group 2
    armed: true
    trigger:
      type: midi
      message: control_change
      channel: 1
      number: 7
      group: 2
      value: 305419896
    actions:
      - { id: note, type: notify.operator, message: "midi2 cue" }
"#;

#[test]
fn midi_2_ump_over_the_network_triggers_a_rule() {
    let port = free_udp_port();
    let mut c = config();
    c.listeners
        .push(tpt_app_av_automation_service::ListenerConfig {
            protocol: "ump".into(),
            bind: format!("127.0.0.1:{port}"),
        });
    let running = Running::start(build(UMP_PACK, c, None, Arc::new(SystemClock)));
    let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
    let target = ("127.0.0.1", port);

    // A MIDI 2.0 control change: group 2, channel 1, index 7, value 0x12345678.
    let packet = Ump::from_message(&Midi2Message::Midi2ChannelVoice(
        Midi2ChannelVoice::ControlChange {
            group: 2,
            channel: 1,
            index: 7,
            value: 0x1234_5678,
        },
    ))
    .to_bytes();
    sender.send_to(&packet, target).unwrap();

    wait_until("the MIDI 2.0 cue to fire", || {
        !running.handle.snapshot().recent.is_empty()
    });

    // A message from the wrong group must not fire the rule.
    let before = running.handle.snapshot().recent.len();
    let wrong_group = Ump::from_message(&Midi2Message::Midi2ChannelVoice(
        Midi2ChannelVoice::ControlChange {
            group: 3,
            channel: 1,
            index: 7,
            value: 0x1234_5678,
        },
    ))
    .to_bytes();
    for _ in 0..20 {
        sender.send_to(&wrong_group, target).unwrap();
    }
    std::thread::sleep(std::time::Duration::from_millis(200));
    assert_eq!(
        running.handle.snapshot().recent.len(),
        before,
        "a different UMP group must not fire this rule"
    );

    // Malformed UMP is counted and does not kill the listener.
    sender.send_to(&[0x40, 0x90, 0x3C], target).unwrap();
    sender.send_to(&[0u8; 9], target).unwrap();
    wait_until("the malformed UMP to be counted", || {
        running.handle.rejected_counts().0 > 0
    });
    let before = running.handle.snapshot().recent.len();
    sender.send_to(&packet, target).unwrap();
    wait_until("a valid packet to still work after garbage", || {
        running.handle.snapshot().recent.len() > before
    });
}
