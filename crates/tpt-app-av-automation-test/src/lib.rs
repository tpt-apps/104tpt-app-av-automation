//! Replay harness for golden rule-pack and chaos tests (spec §18.2–§18.4, §18.6).
//!
//! A *fixture* is a directory:
//!
//! ```text
//! pack.yaml       the rule pack under test
//! devices.yaml    devices; every protocol must be `virtual`
//! script.yaml     the recorded sequence of events, time steps and injected faults
//! expected.json   the golden trace (execution records + what each device received)
//! ```
//!
//! [`Replay::run`] plays the script against the real engine with a fixed clock and virtual devices,
//! so the result is fully deterministic. [`assert_golden`] compares it with `expected.json`; set
//! `UPDATE_GOLDEN=1` to (re)write the expected trace — a diff in that file is a behaviour change and
//! must be deliberate (see `CONTRIBUTING.md`).

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Deserialize;
use serde_json::{json, Value};
use tpt_app_av_automation_actions::{Actions, MemorySink, NullSleeper};
use tpt_app_av_automation_core::{DeviceHealth, Error, Event, FixedClock, Result, Timestamp};
use tpt_app_av_automation_devices::{DeviceFile, DeviceRegistry, Faults, VirtualEndpoint};
use tpt_app_av_automation_engine::{Engine, EngineConfig};
use tpt_app_av_automation_model::{ExecutionRecord, LocalTime, RulePack, Weekday};

/// Monday 2024-01-01 00:00 UTC: the moment every replay starts.
pub const REPLAY_START_MS: u64 = 1_704_067_200_000;

/// A device fault to inject into a virtual endpoint.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FaultSpec {
    /// Device id.
    pub device: String,
    /// Take the device offline (or bring it back with `false`).
    #[serde(default)]
    pub offline: bool,
    /// Silently drop this many upcoming messages.
    #[serde(default)]
    pub drop_next: u32,
    /// Go offline after this many further successful sends ("drops out mid-chain").
    #[serde(default)]
    pub offline_after: Option<u32>,
    /// Every send times out.
    #[serde(default)]
    pub timeout: bool,
    /// Every send is delayed this long.
    #[serde(default)]
    pub delay_ms: u64,
}

/// A health override.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HealthSpec {
    /// Device id.
    pub device: String,
    /// `online`, `degraded`, `offline` or `unknown`.
    pub state: DeviceHealth,
}

/// One step of a recorded script.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Step {
    /// Jump the clock to a local time, `HH:MM` or `HH:MM@day`, on the replay's first week.
    SetTime(String),
    /// Advance the clock.
    AdvanceMs(u64),
    /// Override a device's health without emitting an event.
    Health(HealthSpec),
    /// Inject a device fault.
    Fault(FaultSpec),
    /// Feed an event to the engine.
    Event(Event),
    /// Let time-driven triggers (schedules, heartbeat deadlines) run.
    Tick,
    /// A device heartbeat.
    Heartbeat(String),
    /// Arm a rule.
    Arm(String),
    /// Disarm a rule.
    Disarm(String),
    /// Run a rule now.
    Run(String),
    /// Cancel everything currently running (only meaningful between steps; included for completeness).
    CancelAll,
}

/// A recorded script.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Script {
    /// The steps, in order. Written as one-key maps (`- arm: rule`) rather than YAML tags.
    #[serde(with = "serde_yaml::with::singleton_map_recursive")]
    pub steps: Vec<Step>,
}

/// The outcome of replaying a fixture.
#[derive(Debug, Clone)]
pub struct Replay {
    /// Every execution, in order.
    pub records: Vec<ExecutionRecord>,
    /// What each device received, described one line per command.
    pub sent: BTreeMap<String, Vec<String>>,
}

impl Replay {
    /// Plays the fixture in `dir`.
    pub fn run(dir: &Path) -> Result<Replay> {
        let pack = RulePack::from_path(dir.join("pack.yaml"))?;
        let devices = DeviceFile::from_path(dir.join("devices.yaml"))?;
        let script_text = std::fs::read_to_string(dir.join("script.yaml"))
            .map_err(|e| Error::Io(format!("{}: {e}", dir.join("script.yaml").display())))?;
        let script: Script =
            serde_yaml::from_str(&script_text).map_err(|e| Error::Parse(format!("script.yaml: {e}")))?;
        Self::play(pack, &devices, &script)
    }

    /// Plays a script against an in-memory pack and devices.
    pub fn play(pack: RulePack, devices: &DeviceFile, script: &Script) -> Result<Replay> {
        let clock = FixedClock::new(Timestamp::from_millis(REPLAY_START_MS));
        let sink = Arc::new(MemorySink::default());
        let mut actions = Actions::new(sink.clone(), sink, Arc::new(NullSleeper));
        let mut registry = DeviceRegistry::new();
        let mut endpoints: BTreeMap<String, VirtualEndpoint> = BTreeMap::new();
        for device in &devices.devices {
            if device.protocol != "virtual" {
                return Err(Error::InvalidOperation(format!(
                    "fixture device `{}` must use the `virtual` protocol, not `{}`",
                    device.id, device.protocol
                )));
            }
            registry.register(device.to_device());
            let endpoint = VirtualEndpoint::new();
            actions.bind_endpoint(device.id.clone(), Arc::new(endpoint.clone()));
            endpoints.insert(device.id.clone(), endpoint);
        }
        let mut engine = Engine::new(pack, registry, actions, Arc::new(clock.clone()), EngineConfig::default())?;

        let mut records = Vec::new();
        for step in &script.steps {
            match step {
                Step::SetTime(text) => {
                    let (time, day) = parse_local_time(text)?;
                    let ms = u64::from(day) * 86_400_000 + u64::from(time.minutes_since_midnight()) * 60_000;
                    clock.set(Timestamp::from_millis(REPLAY_START_MS + ms));
                }
                Step::AdvanceMs(ms) => clock.advance_millis(*ms),
                Step::Health(h) => {
                    engine.registry_mut().set_health(&h.device, h.state)?;
                    if h.state == DeviceHealth::Online {
                        // Mark it as recently seen so heartbeat deadlines count from now.
                        let now = engine.now();
                        engine.registry_mut().heartbeat(&h.device, now)?;
                    }
                }
                Step::Fault(f) => {
                    let endpoint = endpoints.get(&f.device).ok_or_else(|| Error::NotFound {
                        kind: "device",
                        id: f.device.clone(),
                    })?;
                    endpoint.set_faults(Faults {
                        offline: f.offline,
                        drop_next: f.drop_next,
                        offline_after: f.offline_after,
                        timeout: f.timeout,
                        delay_ms: f.delay_ms,
                    });
                }
                Step::Event(event) => records.extend(engine.handle_event(event.clone())),
                Step::Tick => records.extend(engine.tick()),
                Step::Heartbeat(device) => records.extend(engine.device_heartbeat(device)?),
                Step::Arm(rule) => engine.arm(rule)?,
                Step::Disarm(rule) => engine.disarm(rule)?,
                Step::Run(rule) => records.extend(engine.run_rule(rule)?),
                Step::CancelAll => {
                    engine.cancel_handle().cancel_all();
                }
            }
        }

        let sent = endpoints
            .iter()
            .map(|(id, ep)| (id.clone(), ep.sent().iter().map(|c| c.describe()).collect()))
            .collect();
        Ok(Replay { records, sent })
    }

    /// The canonical JSON form compared against `expected.json`.
    pub fn to_json(&self) -> Value {
        json!({ "records": self.records, "sent": self.sent })
    }
}

fn parse_local_time(text: &str) -> Result<(LocalTime, u8)> {
    let (time, day) = match text.split_once('@') {
        Some((t, d)) => (t, Some(d)),
        None => (text, None),
    };
    let time: LocalTime = time
        .parse()
        .map_err(|_| Error::Parse(format!("invalid set_time `{text}`: expected HH:MM or HH:MM@day")))?;
    let day = match day {
        Some(d) => Weekday::parse(d)
            .ok_or_else(|| Error::Parse(format!("invalid weekday in set_time `{text}`")))?
            .index() as u8,
        None => 0,
    };
    Ok((time, day))
}

/// The directory holding all fixtures.
pub fn fixtures_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures")
}

/// Every fixture directory (one containing `script.yaml`) under `tests/fixtures/<group>/`.
pub fn discover(group: &str) -> Vec<PathBuf> {
    let root = fixtures_root().join(group);
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(&root)
        .map(|entries| {
            entries
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.join("script.yaml").is_file())
                .collect()
        })
        .unwrap_or_default();
    dirs.sort();
    dirs
}

/// Replays a fixture and compares it with `expected.json`.
///
/// With `UPDATE_GOLDEN=1` the expected file is rewritten instead. A missing expected file is a
/// failure, so a fixture can never silently "pass" by having nothing to compare against.
pub fn assert_golden(dir: &Path) {
    let replay = Replay::run(dir).unwrap_or_else(|e| panic!("{}: replay failed: {e}", dir.display()));
    let actual = replay.to_json();
    let expected_path = dir.join("expected.json");
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        let text = serde_json::to_string_pretty(&actual).expect("trace serializes") + "\n";
        std::fs::write(&expected_path, text).expect("write expected.json");
        return;
    }
    let expected_text = std::fs::read_to_string(&expected_path).unwrap_or_else(|_| {
        panic!(
            "{} has no expected.json; review the replay output and run with UPDATE_GOLDEN=1 to create it",
            dir.display()
        )
    });
    let expected: Value = serde_json::from_str(&expected_text)
        .unwrap_or_else(|e| panic!("{}: expected.json is not valid JSON: {e}", dir.display()));
    if actual != expected {
        panic!(
            "golden trace mismatch for {}\n--- expected ---\n{}\n--- actual ---\n{}\n(run with UPDATE_GOLDEN=1 only if this behaviour change is intended)",
            dir.display(),
            serde_json::to_string_pretty(&expected).unwrap_or_default(),
            serde_json::to_string_pretty(&actual).unwrap_or_default()
        );
    }
}
