//! The CLI as a user (or a script, or a supervisor) meets it (spec §13, §18.4, §24).

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream, UdpSocket};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;
use tpt_av_control_osc::{OscArg, OscMessage};

const BIN: &str = env!("CARGO_BIN_EXE_tpt-av-automation");
const TOKEN: &str = "chaos-test-token-0123456789";

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("tpt-av-cli-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn write(&self, name: &str, content: &str) -> PathBuf {
        let path = self.0.join(name);
        std::fs::write(&path, content).unwrap();
        path
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn cli(args: &[&str]) -> Output {
    Command::new(BIN)
        .args(args)
        .output()
        .expect("failed to run the CLI")
}

fn code(output: &Output) -> i32 {
    output
        .status
        .code()
        .expect("process was killed by a signal")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn wait_until(what: &str, timeout: Duration, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if condition() {
            return;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    panic!("timed out waiting for {what}");
}

fn free_udp_port() -> u16 {
    UdpSocket::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn free_tcp_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn kill_pid(pid: u32) {
    #[cfg(windows)]
    let _ = Command::new("taskkill")
        .args(["/F", "/PID", &pid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    #[cfg(not(windows))]
    let _ = Command::new("kill")
        .args(["-9", &pid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// Kills a child process on drop so a failing test cannot leave engines running.
struct Guard(Child);

/// Kills the watchdog first (so it cannot restart anything), then the engine it left behind.
struct ChaosGuard {
    watchdog: Child,
    pid_file: PathBuf,
}

impl Drop for ChaosGuard {
    fn drop(&mut self) {
        let _ = self.watchdog.kill();
        let _ = self.watchdog.wait();
        if let Some(pid) = read_pid(&self.pid_file) {
            kill_pid(pid);
        }
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

const PACK: &str = r#"
format_version: 1
name: CLI Test
revision: 1
rules:
  - id: cue
    name: Cue
    armed: ARMED
    trigger: { type: osc, address: /go }
    conditions:
      - { id: proj-up, type: device_health, device: proj, equals: online, fire_on_unknown: true }
    actions:
      - { id: power, type: control.osc, device: proj, address: /projector/power, args: [1] }
      - { id: note, type: notify.operator, message: hello }
"#;

fn pack(armed: bool) -> String {
    PACK.replace("ARMED", if armed { "true" } else { "false" })
}

fn devices(port: u16) -> String {
    format!("devices:\n  - {{ id: proj, protocol: osc, address: '127.0.0.1:{port}' }}\n")
}

// --- validate -----------------------------------------------------------------------------------

#[test]
fn validate_accepts_the_shipped_example_packs() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../rules");
    let devices = root.join("examples/devices.yaml");
    for pack in [
        "examples/hello.yaml",
        "live-event/main-hall-event-start.yaml",
        "failover/main-display-failover.yaml",
    ] {
        let out = cli(&[
            "validate",
            "--rules",
            root.join(pack).to_str().unwrap(),
            "--devices",
            devices.to_str().unwrap(),
        ]);
        assert_eq!(code(&out), 0, "{pack}: {}{}", stdout(&out), stderr(&out));
        assert!(stdout(&out).starts_with("OK:"), "{pack}");
    }
}

#[test]
fn validate_rejects_bad_configuration_with_exit_code_4() {
    let dir = TempDir::new("validate");
    let duplicate = dir.write(
        "dup.yaml",
        "format_version: 1\nname: D\nrevision: 1\nrules:\n  - {id: a, name: A, trigger: {type: manual}, actions: [{id: x, type: notify.operator, message: m}]}\n  - {id: a, name: B, trigger: {type: manual}, actions: [{id: x, type: notify.operator, message: m}]}\n",
    );
    let out = cli(&["validate", "--rules", duplicate.to_str().unwrap()]);
    assert_eq!(code(&out), 4);
    assert!(stdout(&out).starts_with("INVALID"));
    assert!(stderr(&out).contains("duplicate"), "{}", stderr(&out));

    let garbage = dir.write("garbage.yaml", ": : [");
    assert_eq!(
        code(&cli(&["validate", "--rules", garbage.to_str().unwrap()])),
        4
    );
    assert_eq!(
        code(&cli(&[
            "validate",
            "--rules",
            dir.path().join("missing.yaml").to_str().unwrap()
        ])),
        4
    );

    // A rule that targets a device the device file does not define.
    let pack_file = dir.write("pack.yaml", &pack(false));
    let empty_devices = dir.write("devices.yaml", "devices: []\n");
    let out = cli(&[
        "validate",
        "--rules",
        pack_file.to_str().unwrap(),
        "--devices",
        empty_devices.to_str().unwrap(),
    ]);
    assert_eq!(code(&out), 4);
    assert!(stderr(&out).contains("proj"), "{}", stderr(&out));
}

#[test]
fn validate_json_is_machine_readable() {
    let dir = TempDir::new("validate-json");
    let pack_file = dir.write("pack.yaml", &pack(false));
    let out = cli(&[
        "validate",
        "--rules",
        pack_file.to_str().unwrap(),
        "--format",
        "json",
    ]);
    assert_eq!(code(&out), 0);
    let v: Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!(v["valid"], true);
    assert_eq!(v["errors"], 0);
}

#[test]
fn validate_checks_the_service_config_too() {
    let dir = TempDir::new("validate-config");
    let pack_file = dir.write("pack.yaml", &pack(false));
    let config = dir.write(
        "service.yaml",
        "api:\n  enabled: true\n  bind: 0.0.0.0:8787\n",
    );
    let out = cli(&[
        "validate",
        "--rules",
        pack_file.to_str().unwrap(),
        "--config",
        config.to_str().unwrap(),
    ]);
    assert_eq!(code(&out), 4);
    assert!(
        stderr(&out).contains("api.token") && stderr(&out).contains("api.bind"),
        "{}",
        stderr(&out)
    );
}

// --- simulate -----------------------------------------------------------------------------------

#[test]
fn simulate_reports_what_would_happen_and_exits_zero() {
    let dir = TempDir::new("simulate");
    let pack_file = dir.write("pack.yaml", &pack(false));
    let out = cli(&[
        "simulate",
        "--rules",
        pack_file.to_str().unwrap(),
        "--event",
        "osc:/go",
    ]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let text = stdout(&out);
    assert!(
        text.starts_with("SIMULATION — no live actions were sent"),
        "{text}"
    );
    assert!(text.contains("Would execute:"), "{text}");
    assert!(text.contains("osc /projector/power = 1 -> proj"), "{text}");
}

#[test]
fn simulate_exit_codes_distinguish_skipped_and_bad_input() {
    let dir = TempDir::new("simulate-codes");
    let pack_file = dir.write("pack.yaml", &pack(false));
    let rules = pack_file.to_str().unwrap();
    assert_eq!(
        code(&cli(&[
            "simulate",
            "--rules",
            rules,
            "--event",
            "osc:/other"
        ])),
        3,
        "nothing matched"
    );
    assert_eq!(
        code(&cli(&["simulate", "--rules", rules, "--event", "bogus"])),
        4
    );
    assert_eq!(
        code(&cli(&[
            "simulate",
            "--rules",
            rules,
            "--event",
            "osc:/go",
            "--health",
            "proj=sideways"
        ])),
        4
    );
    // `fire_on_unknown` only opts in to *unknown* state: a device known to be offline still blocks.
    let blocked = cli(&[
        "simulate",
        "--rules",
        rules,
        "--event",
        "osc:/go",
        "--health",
        "proj=offline",
    ]);
    assert_eq!(code(&blocked), 3);
    let allowed = cli(&[
        "simulate",
        "--rules",
        rules,
        "--event",
        "osc:/go",
        "--health",
        "proj=online",
    ]);
    assert_eq!(code(&allowed), 0);
}

#[test]
fn simulate_json_matches_the_documented_shape() {
    let dir = TempDir::new("simulate-json");
    let pack_file = dir.write("pack.yaml", &pack(false));
    let out = cli(&[
        "simulate",
        "--rules",
        pack_file.to_str().unwrap(),
        "--event",
        "osc:/go",
        "--format",
        "json",
    ]);
    let v: Value = serde_json::from_str(&stdout(&out)).unwrap();
    let first = &v[0];
    assert_eq!(first["rule"], "Cue");
    assert_eq!(first["status"], "success");
    assert_eq!(first["actions_executed"], 2);
    assert_eq!(first["actions_failed"], 0);
    assert_eq!(first["simulated"], true);

    let trace = cli(&[
        "simulate",
        "--rules",
        pack_file.to_str().unwrap(),
        "--event",
        "osc:/go",
        "--format",
        "trace",
    ]);
    let full: Value = serde_json::from_str(&stdout(&trace)).unwrap();
    assert_eq!(
        full[0]["action_results"][0]["detail"],
        "osc /projector/power = 1 -> proj"
    );
}

#[test]
fn simulation_never_sends_a_real_control_message_even_for_armed_rules() {
    let receiver = UdpSocket::bind("127.0.0.1:0").unwrap();
    receiver
        .set_read_timeout(Some(Duration::from_millis(600)))
        .unwrap();
    let port = receiver.local_addr().unwrap().port();

    let dir = TempDir::new("simulate-silent");
    let pack_file = dir.write("pack.yaml", &pack(true));
    let devices_file = dir.write("devices.yaml", &devices(port));
    let out = cli(&[
        "simulate",
        "--rules",
        pack_file.to_str().unwrap(),
        "--devices",
        devices_file.to_str().unwrap(),
        "--event",
        "osc:/go",
    ]);
    assert_eq!(code(&out), 0);
    let mut buf = [0u8; 512];
    assert!(
        receiver.recv(&mut buf).is_err(),
        "a datagram reached the device during a simulation"
    );
}

#[test]
fn simulate_sequences_share_state_between_events() {
    let dir = TempDir::new("simulate-seq");
    let pack_file = dir.write(
        "pack.yaml",
        "format_version: 1\nname: S\nrevision: 1\nrules:\n  - id: up\n    name: Up\n    trigger: { type: dmx, universe: 1, channel: 4, comparison: crossed_above, value: 100 }\n    actions: [ { id: a, type: notify.operator, message: up } ]\n",
    );
    let out = cli(&[
        "simulate",
        "--rules",
        pack_file.to_str().unwrap(),
        "--event",
        "dmx:1:4=50",
        "--event",
        "dmx:1:4=150",
        "--event",
        "dmx:1:4=200",
        "--format",
        "json",
    ]);
    let v: Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!(v.as_array().unwrap().len(), 1, "only the crossing fires");
}

// --- run, end to end ----------------------------------------------------------------------------

#[test]
fn an_armed_rule_turns_an_inbound_osc_message_into_an_outbound_one() {
    let receiver = UdpSocket::bind("127.0.0.1:0").unwrap();
    receiver
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let device_port = receiver.local_addr().unwrap().port();
    let listen_port = free_udp_port();

    let dir = TempDir::new("run-e2e");
    let pack_file = dir.write("pack.yaml", &pack(true));
    let devices_file = dir.write("devices.yaml", &devices(device_port));
    let config = dir.write(
        "service.yaml",
        &format!(
            "tick_ms: 20\nlisteners:\n  - {{ protocol: osc, bind: '127.0.0.1:{listen_port}' }}\n"
        ),
    );
    let state = dir.path().join("state");

    let engine = Guard(
        Command::new(BIN)
            .args([
                "run",
                "--service",
                "--rules",
                pack_file.to_str().unwrap(),
                "--devices",
                devices_file.to_str().unwrap(),
                "--config",
                config.to_str().unwrap(),
                "--state-dir",
                state.to_str().unwrap(),
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );

    // Keep poking the engine until it is listening; the first datagrams may precede the bind.
    let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
    let go = OscMessage::new("/go", &[OscArg::Int(1)]).unwrap().encode();
    let mut received = None;
    let mut buf = [0u8; 512];
    let deadline = Instant::now() + Duration::from_secs(20);
    receiver
        .set_read_timeout(Some(Duration::from_millis(200)))
        .unwrap();
    while Instant::now() < deadline && received.is_none() {
        let _ = sender.send_to(&go, ("127.0.0.1", listen_port));
        if let Ok(n) = receiver.recv(&mut buf) {
            received = Some(buf[..n].to_vec());
        }
    }
    let packet = received.expect("the engine never sent the outbound OSC message");
    let message = OscMessage::decode(&packet).unwrap();
    assert_eq!(message.address, "/projector/power");
    assert_eq!(message.arguments, vec![OscArg::Int(1)]);

    // The execution was logged with its detail, and `history` can show it.
    wait_until(
        "the execution to be persisted",
        Duration::from_secs(10),
        || {
            let out = cli(&[
                "history",
                "--state-dir",
                state.to_str().unwrap(),
                "--format",
                "json",
            ]);
            code(&out) == 0 && stdout(&out).contains("\"status\":\"success\"")
        },
    );
    let out = cli(&[
        "history",
        "--state-dir",
        state.to_str().unwrap(),
        "--format",
        "trace",
    ]);
    let records: Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!(records[0]["rule_id"], "cue");
    assert_eq!(records[0]["simulated"], false);
    assert_eq!(records[0]["trigger_detail"]["kind"], "osc");
    drop(engine);
}

#[test]
fn run_refuses_a_pack_whose_devices_are_undefined() {
    let dir = TempDir::new("run-undefined");
    let pack_file = dir.write("pack.yaml", &pack(false));
    let out = cli(&["run", "--service", "--rules", pack_file.to_str().unwrap()]);
    assert_eq!(code(&out), 4);
    assert!(stderr(&out).contains("proj"), "{}", stderr(&out));
}

#[test]
fn history_without_a_database_is_a_configuration_error() {
    let dir = TempDir::new("history-missing");
    assert_eq!(
        code(&cli(&[
            "history",
            "--state-dir",
            dir.path().to_str().unwrap()
        ])),
        4
    );
}

// --- chaos: watchdog ----------------------------------------------------------------------------

fn health(port: u16) -> Option<Value> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(2))).ok()?;
    stream
        .write_all(
            format!(
                "GET /health HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {TOKEN}\r\n\r\n"
            )
            .as_bytes(),
        )
        .ok()?;
    let mut raw = String::new();
    stream.read_to_string(&mut raw).ok()?;
    serde_json::from_str(raw.split("\r\n\r\n").nth(1)?).ok()
}

fn read_pid(path: &Path) -> Option<u32> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

#[test]
fn the_watchdog_restarts_a_forcibly_killed_engine() {
    let dir = TempDir::new("chaos-watchdog");
    let api_port = free_tcp_port();
    let pack_file = dir.write(
        "pack.yaml",
        "format_version: 1\nname: Chaos\nrevision: 1\nrules:\n  - id: r\n    name: R\n    trigger: { type: manual }\n    actions: [ { id: a, type: notify.operator, message: hi } ]\n",
    );
    let config = dir.write(
        "service.yaml",
        &format!("tick_ms: 20\napi:\n  enabled: true\n  bind: '127.0.0.1:{api_port}'\n  token: '{TOKEN}'\n"),
    );
    let state = dir.path().join("state");
    let pid_file = state.join("engine.pid");

    let _guard = ChaosGuard {
        pid_file: pid_file.clone(),
        watchdog: Command::new(BIN)
            .args([
                "watchdog",
                "--max-restarts",
                "5",
                "--",
                "run",
                "--service",
                "--rules",
                pack_file.to_str().unwrap(),
                "--config",
                config.to_str().unwrap(),
                "--state-dir",
                state.to_str().unwrap(),
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    };

    wait_until("the engine to come up", Duration::from_secs(30), || {
        health(api_port).is_some()
    });
    let first_pid = read_pid(&pid_file).expect("engine pid file");

    // Chaos: kill the engine process outright, mid-flight.
    kill_pid(first_pid);

    wait_until(
        "the watchdog to bring the engine back",
        Duration::from_secs(30),
        || {
            matches!(read_pid(&pid_file), Some(pid) if pid != first_pid)
                && health(api_port).is_some()
        },
    );
    let second_pid = read_pid(&pid_file).unwrap();
    assert_ne!(first_pid, second_pid, "a new process, not the old one");
    let v = health(api_port).unwrap();
    assert_eq!(v["rules"], 1, "the restarted engine reloaded its rules");

    // And it keeps working afterwards.
    kill_pid(second_pid);
    wait_until("a second restart", Duration::from_secs(30), || {
        matches!(read_pid(&pid_file), Some(pid) if pid != second_pid) && health(api_port).is_some()
    });
}

#[test]
fn init_scaffolds_a_show_that_validates_and_simulates() {
    let dir = TempDir::new("init");
    let show = dir.path().join("show");
    let show_arg = show.to_str().unwrap();

    let made = cli(&["init", show_arg]);
    assert_eq!(code(&made), 0, "{}", stderr(&made));
    for file in ["pack.yaml", "devices.yaml", "service.yaml"] {
        assert!(show.join(file).exists(), "{file} should be created");
    }

    let rules = show.join("pack.yaml");
    let devices = show.join("devices.yaml");
    let config = show.join("service.yaml");
    let valid = cli(&[
        "validate",
        "--rules",
        rules.to_str().unwrap(),
        "--devices",
        devices.to_str().unwrap(),
        "--config",
        config.to_str().unwrap(),
    ]);
    assert_eq!(code(&valid), 0, "{}", stderr(&valid));

    let sim = cli(&[
        "simulate",
        "--rules",
        rules.to_str().unwrap(),
        "--devices",
        devices.to_str().unwrap(),
        "--event",
        "manual:start-show",
    ]);
    assert!(stdout(&sim).contains("SIMULATION"), "{}", stdout(&sim));

    // Existing files are never clobbered without --force.
    assert_eq!(code(&cli(&["init", show_arg])), 4);
    assert_eq!(code(&cli(&["init", show_arg, "--force"])), 0);
}

#[test]
fn init_lists_templates_and_rejects_unknown_ones() {
    let listed = cli(&["init", "--list"]);
    assert_eq!(code(&listed), 0);
    assert!(stdout(&listed).contains("classroom"));
    let dir = TempDir::new("init-bad");
    let bad = cli(&["init", dir.path().to_str().unwrap(), "--template", "nope"]);
    assert_eq!(code(&bad), 4, "{}", stderr(&bad));
}
