//! Chaos: the engine killed mid-flight, and what survives (spec §11, §18.4, §24).
//!
//! A venue does not get to choose when the power fails. These scenarios kill the engine without
//! warning — no graceful shutdown, no flush — and then check what must still hold: the state
//! database is not corrupted, a one-shot schedule does not fire twice, and the engine comes back
//! and works.
//!
//! The watchdog restart itself is already covered by the CLI suite
//! (`the_watchdog_restarts_a_forcibly_killed_engine`); what is added here is the *durability* of
//! state across that restart, which the watchdog test cannot see.

use std::time::Duration;

use std::sync::{Mutex, MutexGuard, OnceLock};

use tpt_app_av_automation_scenarios::{
    cli, code, http_get, http_post, stderr, stdout, wait_until, Engine, TempDir, EXIT_OK, PATIENCE, TOKEN,
};

/// Only one engine runs at a time in this suite.
///
/// Every scenario needs a fixed loopback port for the API (the port has to be known in advance, so
/// it cannot be `0`), and a port chosen this way can still be taken between reservation and bind.
/// Serializing removes that race entirely, at the cost of running these scenarios sequentially —
/// which is fine, since each one is a few hundred milliseconds.
fn exclusive() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    match LOCK.get_or_init(|| Mutex::new(())).lock() {
        Ok(guard) => guard,
        // A previous scenario panicked while holding the lock; the data is still valid.
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// A pack with one armed manual rule, so a scenario can drive it through the API.
const MANUAL_PACK: &str = r#"
format_version: 1
name: Chaos
revision: 1
rules:
  - id: r
    name: R
    armed: true
    trigger: { type: manual }
    actions:
      - { id: note, type: log.incident, severity: high, message: fired by chaos }
"#;

/// A pack whose single one-shot schedule fires on the next minute boundary.
///
/// `cron: "* * * * *"` matches every minute, so the rule becomes due within a minute of the engine
/// starting whatever the wall clock says — unlike a fixed `at` time, which could be hours away.
const ONE_SHOT_PACK: &str = r#"
format_version: 1
name: One Shot
revision: 1
rules:
  - id: once
    name: Once
    armed: true
    trigger: { type: schedule, cron: "* * * * *", once: true }
    actions:
      - { id: note, type: log.incident, severity: high, message: one shot fired }
"#;

/// Builds a scenario directory and returns the paths a test needs.
struct Harness {
    _dir: TempDir,
    rules: std::path::PathBuf,
    devices: std::path::PathBuf,
    config: std::path::PathBuf,
    state: std::path::PathBuf,
    api: String,
}

impl Harness {
    /// Creates a scenario listening on `api_port`.
    /// Creates a scenario whose API listens on a freshly reserved port.
    ///
    /// The reservation is released immediately: it only picks a number unlikely to be in use, and
    /// scenarios are serialized by [`exclusive`] so two of them cannot pick the same one.
    fn new(name: &str, pack: &str) -> Self {
        let api_port = tpt_app_av_automation_scenarios::free_udp_port();
        let dir = TempDir::new(name);
        let rules = dir.write("pack.yaml", pack);
        let devices = dir.write("devices.yaml", "devices:\n  - { id: rig, protocol: virtual }\n");
        let config = dir.write(
            "service.yaml",
            &format!(
                "tick_ms: 25\nheartbeat_interval_ms: 200\n\
                 api:\n  enabled: true\n  bind: \"127.0.0.1:{api_port}\"\n  token: {TOKEN:?}\n"
            ),
        );
        let state = dir.path().join("state");
        std::fs::create_dir_all(&state).unwrap();
        let harness = Self {
            _dir: dir,
            rules,
            devices,
            config,
            state,
            api: format!("127.0.0.1:{api_port}"),
        };
        harness
    }

    /// The arguments that start `run --service` for this scenario.
    fn run_args(&self) -> Vec<String> {
        [
            "run",
            "--service",
            "--rules",
            self.rules.to_str().unwrap(),
            "--devices",
            self.devices.to_str().unwrap(),
            "--config",
            self.config.to_str().unwrap(),
            "--state-dir",
            self.state.to_str().unwrap(),
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    }

    /// Starts the engine and waits for its API to answer.
    fn start(&self) -> Engine {
        let args = self.run_args();
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let engine = Engine::start(&refs);
        wait_until("the API to answer", PATIENCE, || {
            http_get(&self.api, "/health", TOKEN).0 == 200
        });
        engine
    }
}


/// Killing the engine must never corrupt the state database: `history` can still read it afterwards,
/// and it still reports the executions that had been recorded.
#[test]
fn the_state_database_survives_a_forced_kill_and_still_reports_its_executions() {
    let _serialized = exclusive();
    let h = Harness::new("chaos-db", MANUAL_PACK);

    let mut engine = h.start();
    // Record some executions through the API before the kill.
    for _ in 0..3 {
        let _ = http_post(&h.api, "/rules/r/run", TOKEN);
    }
    wait_until("the executions to be recorded", PATIENCE, || {
        let (status, body) = http_get(&h.api, "/executions", TOKEN);
        status == 200 && body.contains("fired by chaos")
    });
    let recorded = http_get(&h.api, "/executions", TOKEN)
        .1
        .matches("fired by chaos")
        .count();
    assert!(recorded > 0, "the incidents should have been recorded");

    // Kill without warning: no graceful shutdown, no final flush.
    engine.kill();

    // A separate process opens the database independently; it must still be readable, and it must
    // still show the three executions recorded before the kill.
    let out = cli(&["history", "--state-dir", h.state.to_str().unwrap()]);
    assert_eq!(
        code(&out),
        EXIT_OK,
        "history after a forced kill should succeed: {}{}",
        stdout(&out),
        stderr(&out)
    );
    let text = format!("{}{}", stdout(&out), stderr(&out));
    for execution in ["exec-000001", "exec-000002", "exec-000003"] {
        assert!(
            text.contains(execution),
            "the executions recorded before the kill should survive: {text}"
        );
    }
    // All three are reported as successful, with their per-action counts intact.
    assert!(
        text.matches("1 ok / 0 failed / 0 skipped").count() >= 3,
        "each recorded execution should still report its action outcomes: {text}"
    );
}

/// A one-shot schedule is written ahead of its actions, so a hard kill must not cause it to fire a
/// second time on restart (spec §11, §24).
#[test]
fn a_one_shot_schedule_fires_at_most_once_across_a_forced_restart() {
    let _serialized = exclusive();
    let h = Harness::new("chaos-one-shot", ONE_SHOT_PACK);

    // First run: let the one-shot fire once, then kill it.
    let mut first = h.start();
    wait_until("the one-shot to fire", PATIENCE, || {
        let (status, body) = http_get(&h.api, "/incidents", TOKEN);
        status == 200 && body.contains("one shot fired")
    });
    first.kill();

    // Restart against the same state directory.
    let mut second = h.start();
    // Give it several tick intervals: long enough that a re-fired one-shot would have happened.
    std::thread::sleep(Duration::from_millis(750));
    let (_, body) = http_get(&h.api, "/incidents", TOKEN);
    let firings = body.matches("one shot fired").count();
    assert_eq!(
        firings, 1,
        "the one-shot fired {firings} times across a restart; it must fire at most once: {body}"
    );
    second.kill();
}

/// After a restart the engine must be fully functional, not merely listening.
#[test]
fn an_engine_restarted_after_a_kill_still_executes_rules() {
    let _serialized = exclusive();
    let h = Harness::new("chaos-restart-works", MANUAL_PACK);

    let mut first = h.start();
    let _ = http_post(&h.api, "/rules/r/run", TOKEN);
    wait_until("the first run to be recorded", PATIENCE, || {
        http_get(&h.api, "/executions", TOKEN).1.contains("fired by chaos")
    });
    first.kill();

    let mut second = h.start();
    // The API and engine are live after the restart, not merely the socket: the rule can still be
    // disarmed and re-armed.
    assert_eq!(
        http_post(&h.api, "/rules/r/disarm", TOKEN).0,
        200,
        "the restarted engine should accept arm/disarm"
    );
    assert_eq!(
        http_post(&h.api, "/rules/r/arm", TOKEN).0,
        200,
        "the restarted engine should accept arm/disarm"
    );
    let (_, body) = http_get(&h.api, "/executions", TOKEN);
    assert!(
        body.contains("fired by chaos"),
        "history survived the restart: {body}"
    );

    // A fresh manual run after the restart must still produce a record.
    let before = body.matches("fired by chaos").count();
    let _ = http_post(&h.api, "/rules/r/run", TOKEN);
    wait_until("a new execution after the restart", PATIENCE, || {
        http_get(&h.api, "/executions", TOKEN)
            .1
            .matches("fired by chaos")
            .count()
            > before
    });
    second.kill();
}

/// Repeated kills must not degrade the state directory into something unusable.
#[test]
fn the_engine_can_be_killed_repeatedly_and_still_start() {
    let _serialized = exclusive();
    let h = Harness::new("chaos-repeat-kill", MANUAL_PACK);

    for round in 0..3 {
        let mut engine = h.start();
        assert!(
            http_get(&h.api, "/health", TOKEN).0 == 200,
            "round {round}: the engine should be healthy"
        );
        engine.kill();
    }

    // The fourth start must still work, which is the property that actually matters operationally.
    let mut last = h.start();
    assert_eq!(
        http_get(&h.api, "/health", TOKEN).0,
        200,
        "the engine should still start after three forced kills"
    );
    last.kill();
}

