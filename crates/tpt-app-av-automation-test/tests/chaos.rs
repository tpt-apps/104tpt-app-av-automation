//! Chaos and failover tests (spec §18.4).
//!
//! | Scenario | Where |
//! |----------|-------|
//! | device goes offline mid-action-chain | here |
//! | conflicting rules firing on the same event | here + golden `conflict-priority` |
//! | network interruption during action execution | here |
//! | malformed inbound control message | `triggers` (parser), `service` (live socket) tests |
//! | engine killed mid-execution, restarted by the watchdog | `cli` test `the_watchdog_restarts_a_forcibly_killed_engine` |

use tpt_app_av_automation_core::{DeviceHealth, Event};
use tpt_app_av_automation_devices::DeviceFile;
use tpt_app_av_automation_model::{ActionOutcome, ExecutionStatus, RulePack};
use tpt_app_av_automation_test::{Replay, Script};

fn play(pack: &str, devices: &str, script: &str) -> Replay {
    Replay::play(
        RulePack::from_yaml_str(pack).unwrap(),
        &DeviceFile::from_yaml_str(devices).unwrap(),
        &serde_yaml::from_str::<Script>(script).unwrap(),
    )
    .unwrap()
}

const DEVICES: &str = "devices:\n  - { id: dev, protocol: virtual }\n  - { id: other, protocol: virtual }\n";

#[test]
fn a_device_dropping_out_mid_chain_yields_a_partial_failure_not_a_success() {
    let pack = r#"
format_version: 1
name: Chaos
revision: 1
rules:
  - id: chain
    name: Chain
    armed: true
    trigger: { type: manual }
    actions:
      - { id: one, type: control.osc, device: dev, address: /1, args: [1] }
      - { id: two, type: control.osc, device: dev, address: /2, args: [1] }
      - { id: three, type: control.osc, device: dev, address: /3, args: [1] }
      - { id: four, type: control.osc, device: other, address: /4, args: [1] }
    policy: { on_failure: continue_chain }
"#;
    let r = play(
        pack,
        DEVICES,
        r#"
steps:
  - health: { device: dev, state: online }
  - fault: { device: dev, offline_after: 1 }
  - event: { kind: manual, rule: null }
"#,
    );
    let record = &r.records[0];
    assert_eq!(record.overall_status, ExecutionStatus::PartialFailure);
    let outcomes: Vec<_> = record.action_results.iter().map(|a| a.outcome.is_success()).collect();
    assert_eq!(outcomes, [true, false, false, true], "the device died after one message; the other device was unaffected");
    assert_eq!(r.sent["dev"], ["osc /1 = 1"]);
    assert_eq!(r.sent["other"], ["osc /4 = 1"]);
}

#[test]
fn stop_chain_after_a_mid_chain_dropout_records_exactly_what_ran() {
    let pack = r#"
format_version: 1
name: Chaos
revision: 1
rules:
  - id: chain
    name: Chain
    armed: true
    trigger: { type: manual }
    actions:
      - { id: one, type: control.osc, device: dev, address: /1, args: [1] }
      - { id: two, type: control.osc, device: dev, address: /2, args: [1] }
      - { id: three, type: control.osc, device: other, address: /3, args: [1] }
"#;
    let r = play(
        pack,
        DEVICES,
        "steps:\n  - fault: { device: dev, offline_after: 1 }\n  - event: { kind: manual, rule: null }\n",
    );
    let record = &r.records[0];
    assert_eq!(record.overall_status, ExecutionStatus::PartialFailure);
    assert!(record.action_results[0].outcome.is_success());
    assert!(record.action_results[1].outcome.is_failure());
    assert!(matches!(&record.action_results[2].outcome, ActionOutcome::Skipped { reason } if reason.contains("two")));
    assert!(r.sent["other"].is_empty());
}

#[test]
fn network_interruption_during_execution_times_out_and_is_reported() {
    let pack = r#"
format_version: 1
name: Chaos
revision: 1
rules:
  - id: chain
    name: Chain
    armed: true
    trigger: { type: manual }
    actions:
      - { id: slow, type: control.osc, device: dev, address: /slow, args: [1], timeout_ms: 40 }
      - { id: after, type: notify.operator, message: should not run }
"#;
    let r = play(
        pack,
        DEVICES,
        "steps:\n  - fault: { device: dev, delay_ms: 600 }\n  - event: { kind: manual, rule: null }\n",
    );
    let record = &r.records[0];
    assert!(matches!(record.action_results[0].outcome, ActionOutcome::TimedOut { after_ms: 40 }));
    assert!(record.action_results[1].outcome.is_skipped());
    assert_eq!(record.overall_status, ExecutionStatus::Failed);
    // (The abandoned worker may still complete later, exactly as a late packet would on a real
    // network; the record says the step timed out, which is the truth the operator needs.)
}

#[test]
fn a_timeout_fault_on_the_device_is_a_timed_out_step() {
    let pack = r#"
format_version: 1
name: Chaos
revision: 1
rules:
  - id: chain
    name: Chain
    armed: true
    trigger: { type: manual }
    actions:
      - { id: a, type: control.osc, device: dev, address: /a, args: [1] }
"#;
    let r = play(
        pack,
        DEVICES,
        "steps:\n  - fault: { device: dev, timeout: true }\n  - event: { kind: manual, rule: null }\n",
    );
    assert!(matches!(r.records[0].action_results[0].outcome, ActionOutcome::TimedOut { .. }));
}

#[test]
fn a_failed_send_degrades_the_device_and_a_failover_rule_reacts() {
    let pack = r#"
format_version: 1
name: Chaos
revision: 1
rules:
  - id: main
    name: Main
    armed: true
    trigger: { type: manual }
    actions:
      - { id: a, type: control.osc, device: dev, address: /a, args: [1] }
  - id: failover
    name: Failover
    armed: true
    trigger: { type: device_state, device: dev, state: degraded }
    actions:
      - { id: b, type: control.osc, device: other, address: /backup, args: [1] }
"#;
    let r = play(
        pack,
        DEVICES,
        "steps:\n  - health: { device: dev, state: online }\n  - fault: { device: dev, offline: true }\n  - event: { kind: manual, rule: null }\n",
    );
    assert_eq!(r.records.len(), 2);
    assert_eq!(r.records[0].overall_status, ExecutionStatus::Failed);
    assert_eq!(r.records[1].rule_id.as_str(), "failover");
    assert_eq!(r.records[1].overall_status, ExecutionStatus::Success);
    assert!(matches!(
        r.records[1].trigger_detail,
        Event::DeviceState { current: DeviceHealth::Degraded, .. }
    ));
    assert_eq!(r.sent["other"], ["osc /backup = 1"]);
}

#[test]
fn flapping_devices_cannot_trigger_an_unbounded_cascade() {
    // Two rules that each (indirectly) cause the other's trigger: A fails -> degraded -> B fails
    // its own device -> degraded -> A ... The cascade cap stops it.
    let pack = r#"
format_version: 1
name: Flap
revision: 1
rules:
  - id: a
    name: A
    armed: true
    trigger: { type: device_state, device: other, state: degraded }
    actions:
      - { id: x, type: control.osc, device: dev, address: /x, args: [1] }
  - id: b
    name: B
    armed: true
    trigger: { type: device_state, device: dev, state: degraded }
    actions:
      - { id: y, type: control.osc, device: other, address: /y, args: [1] }
  - id: start
    name: Start
    armed: true
    trigger: { type: manual }
    actions:
      - { id: z, type: control.osc, device: dev, address: /z, args: [1] }
"#;
    let r = play(
        pack,
        DEVICES,
        r#"
steps:
  - health: { device: dev, state: online }
  - health: { device: other, state: online }
  - fault: { device: dev, offline: true }
  - fault: { device: other, offline: true }
  - event: { kind: manual, rule: null }
"#,
    );
    // Each failure only degrades a device that was Online, so the loop ends on its own after one
    // round; the point is that it terminates with a bounded, fully recorded trace.
    assert!(r.records.len() <= 64, "unbounded cascade: {} records", r.records.len());
    assert!(r.records.iter().all(|x| x.overall_status != ExecutionStatus::Success));
}

#[test]
fn sustained_event_load_is_processed_and_history_stays_bounded() {
    use std::sync::Arc;
    use std::time::Instant;
    use tpt_app_av_automation_actions::{Actions, MemorySink, NullSleeper};
    use tpt_app_av_automation_core::FixedClock;
    use tpt_app_av_automation_devices::{Device, DeviceRegistry, VirtualEndpoint};
    use tpt_app_av_automation_engine::{Engine, EngineConfig};

    let mut yaml = String::from("format_version: 1\nname: Load\nrevision: 1\nrules:\n");
    for i in 0..20 {
        yaml.push_str(&format!(
            "  - id: r{i}\n    name: R{i}\n    armed: true\n    trigger: {{ type: osc, address: /load/{i} }}\n    actions:\n      - {{ id: a, type: control.osc, device: dev, address: /out/{i}, args: [1] }}\n      - {{ id: n, type: notify.operator, message: x }}\n"
        ));
    }
    let sink = Arc::new(MemorySink::default());
    let mut actions = Actions::new(sink.clone(), sink, Arc::new(NullSleeper));
    let endpoint = VirtualEndpoint::new();
    actions.bind_endpoint("dev", Arc::new(endpoint.clone()));
    let mut registry = DeviceRegistry::new();
    registry.register(Device::new("dev", "dev"));
    let config = EngineConfig {
        history_limit: 1000,
        ..EngineConfig::default()
    };
    let mut engine = Engine::new(
        RulePack::from_yaml_str(&yaml).unwrap(),
        registry,
        actions,
        Arc::new(FixedClock::default()),
        config,
    )
    .unwrap();

    let events = 20_000usize;
    let started = Instant::now();
    let mut fired = 0usize;
    for i in 0..events {
        fired += engine
            .handle_event(Event::Osc {
                address: format!("/load/{}", i % 40),
                args: vec![],
            })
            .len();
    }
    let elapsed = started.elapsed();
    eprintln!(
        "{events} events -> {fired} executions in {elapsed:?} ({:.0} events/s, {} build)",
        events as f64 / elapsed.as_secs_f64(),
        if cfg!(debug_assertions) { "debug" } else { "release" }
    );
    assert_eq!(fired, events / 2, "half the events address one of the 20 rules");
    assert_eq!(engine.history().count(), 1000, "history is capped");
    assert_eq!(endpoint.sent().len(), fired);
    assert!(elapsed.as_secs() < 60, "engine is unreasonably slow: {elapsed:?}");
}
