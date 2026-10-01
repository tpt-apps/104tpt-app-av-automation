//! End-to-end engine behaviour against virtual devices (spec §10, §18.2–§18.4).

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use tpt_app_av_automation_actions::{Actions, CancelToken, MemorySink, NullSleeper, Sleeper};
use tpt_app_av_automation_core::{
    DeviceHealth, DmxLevel, Event, FixedClock, MidiLevel, Timestamp,
};
use tpt_app_av_automation_devices::{Command, Device, DeviceRegistry, Faults, VirtualEndpoint};
use tpt_app_av_automation_engine::{CancelHandle, Engine, EngineConfig, Mode};
use tpt_app_av_automation_model::{
    ActionOutcome, ConditionResult, ExecutionRecord, ExecutionStatus, RulePack,
};

/// 2024-01-01 00:00 UTC, a Monday.
const T0: u64 = 1_704_067_200_000;

struct Harness {
    engine: Engine,
    endpoints: BTreeMap<String, VirtualEndpoint>,
    sink: Arc<MemorySink>,
    clock: FixedClock,
}

struct CancellingSleeper(Mutex<Option<CancelHandle>>);

impl Sleeper for CancellingSleeper {
    fn sleep(&self, _ms: u64, _cancel: &CancelToken) {
        if let Some(handle) = self.0.lock().unwrap().as_ref() {
            handle.cancel_all();
        }
    }
}

fn harness_with(yaml: &str, devices: &[&str], sleeper: Arc<dyn Sleeper>, config: EngineConfig) -> Harness {
    let pack = RulePack::from_yaml_str(yaml).expect("test pack must be valid");
    let clock = FixedClock::new(Timestamp::from_millis(T0));
    let sink = Arc::new(MemorySink::default());
    let mut actions = Actions::new(sink.clone(), sink.clone(), sleeper);
    let mut registry = DeviceRegistry::new();
    let mut endpoints = BTreeMap::new();
    for id in devices {
        let mut device = Device::new(*id, *id);
        device.heartbeat_ms = None;
        registry.register(device);
        registry.heartbeat(id, Timestamp::from_millis(T0)).unwrap();
        let ep = VirtualEndpoint::new();
        actions.bind_endpoint(*id, Arc::new(ep.clone()));
        endpoints.insert((*id).to_string(), ep);
    }
    let engine = Engine::new(pack, registry, actions, Arc::new(clock.clone()), config).unwrap();
    Harness {
        engine,
        endpoints,
        sink,
        clock,
    }
}

fn harness(yaml: &str, devices: &[&str]) -> Harness {
    harness_with(yaml, devices, Arc::new(NullSleeper), EngineConfig::default())
}

impl Harness {
    fn sent(&self, device: &str) -> Vec<Command> {
        self.endpoints[device].sent()
    }
}

fn osc(address: &str, args: &[f64]) -> Event {
    Event::Osc {
        address: address.into(),
        args: args.to_vec(),
    }
}

fn outcomes(record: &ExecutionRecord) -> Vec<&ActionOutcome> {
    record.action_results.iter().map(|r| &r.outcome).collect()
}

const SIMPLE_ARMED: &str = r#"
format_version: 1
name: Simple
revision: 1
rules:
  - id: go
    name: Go
    armed: true
    trigger: { type: osc, address: /go }
    actions:
      - { id: power, type: control.osc, device: proj, address: /power, args: [1] }
      - { id: note, type: notify.operator, message: hello }
"#;

#[test]
fn armed_rule_sends_real_actions_and_explains_itself() {
    let mut h = harness(SIMPLE_ARMED, &["proj"]);
    let records = h.engine.handle_event(osc("/go", &[]));
    assert_eq!(records.len(), 1);
    let r = &records[0];
    assert_eq!(r.overall_status, ExecutionStatus::Success);
    assert!(!r.simulated);
    assert_eq!(r.rule_id.as_str(), "go");
    assert_eq!(r.rule_version, 1);
    assert_eq!(r.trigger_type, "osc");
    assert_eq!(r.trigger_detail, osc("/go", &[]));
    assert_eq!(r.action_results.len(), 2);
    assert_eq!(r.action_results[0].detail.as_deref(), Some("osc /power = 1 -> proj"));
    assert_eq!(h.sent("proj").len(), 1);
    assert_eq!(h.sink.notices().len(), 1);
    assert_eq!(h.engine.history().count(), 1);
}

#[test]
fn unrelated_events_do_nothing() {
    let mut h = harness(SIMPLE_ARMED, &["proj"]);
    assert!(h.engine.handle_event(osc("/other", &[])).is_empty());
    assert!(h
        .engine
        .handle_event(Event::Dmx(DmxLevel {
            universe: 1,
            channel: 1,
            value: 1
        }))
        .is_empty());
    assert!(h.sent("proj").is_empty());
}

#[test]
fn disarmed_rules_only_simulate() {
    let yaml = SIMPLE_ARMED.replace("armed: true", "armed: false");
    let mut h = harness(&yaml, &["proj"]);
    let r = &h.engine.handle_event(osc("/go", &[]))[0];
    assert!(r.simulated);
    assert_eq!(r.overall_status, ExecutionStatus::Success);
    assert!(r.action_results.iter().all(|a| a.simulated));
    assert_eq!(h.endpoints["proj"].attempts(), 0, "nothing may be sent");
    assert!(h.sink.notices().is_empty(), "nothing may be raised either");
}

#[test]
fn simulation_mode_overrides_arming() {
    let mut h = harness(SIMPLE_ARMED, &["proj"]);
    h.engine.set_mode(Mode::Simulation);
    let r = &h.engine.handle_event(osc("/go", &[]))[0];
    assert!(r.simulated);
    assert_eq!(h.endpoints["proj"].attempts(), 0);
    h.engine.set_mode(Mode::Live);
    let r = &h.engine.handle_event(osc("/go", &[]))[0];
    assert!(!r.simulated);
    assert_eq!(h.endpoints["proj"].attempts(), 1);
}

#[test]
fn arming_and_disarming_take_effect_immediately() {
    let yaml = SIMPLE_ARMED.replace("armed: true", "armed: false");
    let mut h = harness(&yaml, &["proj"]);
    assert!(!h.engine.is_armed("go"));
    h.engine.arm("go").unwrap();
    assert!(h.engine.is_armed("go"));
    h.engine.handle_event(osc("/go", &[]));
    assert_eq!(h.endpoints["proj"].attempts(), 1);
    h.engine.disarm("go").unwrap();
    h.engine.handle_event(osc("/go", &[]));
    assert_eq!(h.endpoints["proj"].attempts(), 1);
    assert!(h.engine.arm("nope").is_err());
}

const GATED: &str = r#"
format_version: 1
name: Gated
revision: 1
rules:
  - id: gated
    name: Gated
    armed: true
    trigger: { type: manual }
    conditions:
      - { id: proj-online, type: device_health, device: proj, equals: online }
    actions:
      - { id: a, type: control.osc, device: proj, address: /x, args: [1] }
"#;

#[test]
fn unmet_condition_skips_and_explains() {
    let mut h = harness(GATED, &["proj"]);
    h.engine
        .registry_mut()
        .set_health("proj", DeviceHealth::Offline)
        .unwrap();
    let r = &h.engine.handle_event(Event::Manual { rule: None })[0];
    assert_eq!(r.overall_status, ExecutionStatus::Skipped);
    assert!(r.action_results.is_empty());
    assert_eq!(r.condition_results[0].result, ConditionResult::NotMet);
    assert!(!r.condition_results[0].permits);
    assert!(r.condition_results[0].observed.as_ref().unwrap().contains("offline"));
    assert!(h.sent("proj").is_empty());
}

#[test]
fn unknown_condition_blocks_unless_the_rule_opts_in() {
    let mut h = harness(GATED, &["proj"]);
    h.engine
        .registry_mut()
        .set_health("proj", DeviceHealth::Unknown)
        .unwrap();
    let r = &h.engine.handle_event(Event::Manual { rule: None })[0];
    assert_eq!(r.overall_status, ExecutionStatus::Skipped);
    assert!(r.blocked_by_unknown_condition());

    let lenient = GATED.replace("equals: online }", "equals: online, fire_on_unknown: true }");
    let mut h = harness(&lenient, &["proj"]);
    h.engine
        .registry_mut()
        .set_health("proj", DeviceHealth::Unknown)
        .unwrap();
    let r = &h.engine.handle_event(Event::Manual { rule: None })[0];
    assert_eq!(r.overall_status, ExecutionStatus::Success);
}

#[test]
fn time_window_uses_the_injected_clock() {
    let yaml = r#"
format_version: 1
name: Hours
revision: 1
rules:
  - id: evening
    name: Evening
    armed: true
    trigger: { type: manual }
    conditions:
      - { id: hours, type: time_window, from: "18:00", to: "23:00" }
    actions:
      - { id: n, type: notify.operator, message: hi }
"#;
    let mut h = harness(yaml, &[]);
    assert_eq!(
        h.engine.handle_event(Event::Manual { rule: None })[0].overall_status,
        ExecutionStatus::Skipped
    );
    h.clock.advance_millis(19 * 3_600_000);
    assert_eq!(
        h.engine.handle_event(Event::Manual { rule: None })[0].overall_status,
        ExecutionStatus::Success
    );
}

const TWO_DEVICES: &str = r#"
format_version: 1
name: Chain
revision: 1
rules:
  - id: chain
    name: Chain
    armed: true
    trigger: { type: manual }
    actions:
      - { id: one, type: control.osc, device: dev-a, address: /x, args: [1] }
      - { id: two, type: control.osc, device: dev-b, address: /x, args: [1] }
      - { id: three, type: control.osc, device: dev-a, address: /y, args: [1] }
    policy:
      on_failure: continue_chain
"#;

#[test]
fn partial_failure_is_never_reported_as_success() {
    let mut h = harness(TWO_DEVICES, &["dev-a", "dev-b"]);
    h.endpoints["dev-b"].set_offline(true);
    let r = &h.engine.handle_event(Event::Manual { rule: None })[0];
    assert_eq!(r.overall_status, ExecutionStatus::PartialFailure);
    assert_eq!(r.actions_succeeded(), 2);
    assert_eq!(r.actions_failed(), 1);
    assert!(matches!(outcomes(r)[1], ActionOutcome::Failed { .. }));
}

#[test]
fn stop_chain_skips_the_remaining_steps() {
    let yaml = TWO_DEVICES.replace("continue_chain", "stop_chain");
    let mut h = harness(&yaml, &["dev-a", "dev-b"]);
    h.endpoints["dev-b"].set_offline(true);
    let r = &h.engine.handle_event(Event::Manual { rule: None })[0];
    assert_eq!(r.overall_status, ExecutionStatus::PartialFailure);
    assert!(matches!(outcomes(r)[2], ActionOutcome::Skipped { .. }));
    assert_eq!(h.sent("dev-a").len(), 1, "the third step never ran");
}

#[test]
fn a_chain_where_everything_fails_is_failed() {
    let mut h = harness(TWO_DEVICES, &["dev-a", "dev-b"]);
    h.endpoints["dev-a"].set_offline(true);
    h.endpoints["dev-b"].set_offline(true);
    let r = &h.engine.handle_event(Event::Manual { rule: None })[0];
    assert_eq!(r.overall_status, ExecutionStatus::Failed);
}

#[test]
fn run_fallback_runs_the_fallback_chain_and_still_reports_the_failure() {
    let yaml = r#"
format_version: 1
name: Failover
revision: 1
rules:
  - id: failover
    name: Failover
    armed: true
    trigger: { type: manual }
    actions:
      - { id: switch, type: control.osc, device: switcher, address: /input, args: [2] }
      - { id: after, type: control.osc, device: switcher, address: /after, args: [1] }
    policy:
      on_failure: run_fallback
      fallback:
        - { id: manual, type: notify.operator, message: "Manual intervention required", severity: critical }
"#;
    let mut h = harness(yaml, &["switcher"]);
    h.endpoints["switcher"].set_offline(true);
    let r = &h.engine.handle_event(Event::Manual { rule: None })[0];
    let ids: Vec<_> = r.action_results.iter().map(|a| a.action_id.as_str()).collect();
    assert_eq!(ids, ["switch", "after", "manual"]);
    assert!(matches!(outcomes(r)[1], ActionOutcome::Skipped { .. }));
    assert!(outcomes(r)[2].is_success());
    assert_eq!(r.overall_status, ExecutionStatus::PartialFailure, "fallback success does not hide the failure");
    assert_eq!(h.sink.notices().len(), 1);
}

#[test]
fn fallback_does_not_run_when_nothing_failed() {
    let yaml = r#"
format_version: 1
name: F
revision: 1
rules:
  - id: f
    name: F
    armed: true
    trigger: { type: manual }
    actions:
      - { id: a, type: notify.operator, message: fine }
    policy:
      on_failure: run_fallback
      fallback:
        - { id: fb, type: notify.operator, message: bad }
"#;
    let mut h = harness(yaml, &[]);
    let r = &h.engine.handle_event(Event::Manual { rule: None })[0];
    assert_eq!(r.action_results.len(), 1);
    assert_eq!(r.overall_status, ExecutionStatus::Success);
}

const CONFLICT: &str = r#"
format_version: 1
name: Conflict
revision: 1
rules:
  - id: zz-low
    name: Low
    armed: true
    priority: low
    trigger: { type: osc, address: /go }
    actions:
      - { id: set, type: control.osc, device: proj, address: /input, args: [1] }
  - id: aa-critical
    name: Critical
    armed: true
    priority: critical
    trigger: { type: osc, address: /go }
    actions:
      - { id: set, type: control.osc, device: proj, address: /input, args: [2] }
  - id: bb-normal
    name: Normal
    armed: true
    trigger: { type: osc, address: /go }
    actions:
      - { id: set, type: control.osc, device: proj, address: /input, args: [2] }
      - { id: other, type: control.osc, device: proj, address: /other, args: [5] }
"#;

#[test]
fn conflicting_rules_resolve_by_priority_and_are_recorded() {
    let mut h = harness(CONFLICT, &["proj"]);
    let records = h.engine.handle_event(osc("/go", &[]));
    let order: Vec<_> = records.iter().map(|r| r.rule_id.as_str()).collect();
    assert_eq!(order, ["aa-critical", "bb-normal", "zz-low"], "critical first, then by id");

    let by = |id: &str| records.iter().find(|r| r.rule_id.as_str() == id).unwrap();
    assert!(outcomes(by("aa-critical"))[0].is_success());
    // Identical value to the same slot is not a conflict; an unrelated slot is fine too.
    assert!(outcomes(by("bb-normal")).iter().all(|o| o.is_success()));
    // A different value to the claimed slot is rejected and says why.
    match outcomes(by("zz-low"))[0] {
        ActionOutcome::Skipped { reason } => assert!(reason.contains("aa-critical"), "{reason}"),
        other => panic!("expected conflict skip, got {other:?}"),
    }
    let sent = h.sent("proj");
    assert!(!sent.iter().any(|c| matches!(c, Command::Osc { args, .. } if args == &vec![1.0])));
}

#[test]
fn equal_priority_conflicts_are_resolved_deterministically_by_rule_id() {
    let yaml = CONFLICT.replace("priority: low", "priority: normal").replace("priority: critical", "priority: normal");
    let mut a = harness(&yaml, &["proj"]);
    let mut b = harness(&yaml, &["proj"]);
    let first = a.engine.handle_event(osc("/go", &[]));
    let second = b.engine.handle_event(osc("/go", &[]));
    assert_eq!(first, second);
    assert_eq!(first[0].rule_id.as_str(), "aa-critical");
}

#[test]
fn disarmed_rule_does_not_block_an_armed_rule() {
    let yaml = r#"
format_version: 1
name: Mixed
revision: 1
rules:
  - id: aa-sim
    name: Sim
    armed: false
    priority: critical
    trigger: { type: osc, address: /go }
    actions:
      - { id: set, type: control.osc, device: proj, address: /input, args: [2] }
  - id: zz-live
    name: Live
    armed: true
    priority: low
    trigger: { type: osc, address: /go }
    actions:
      - { id: set, type: control.osc, device: proj, address: /input, args: [1] }
"#;
    let mut h = harness(yaml, &["proj"]);
    let records = h.engine.handle_event(osc("/go", &[]));
    let live = records.iter().find(|r| r.rule_id.as_str() == "zz-live").unwrap();
    assert!(outcomes(live)[0].is_success(), "live write not blocked by a simulated one");
    assert_eq!(h.sent("proj").len(), 1);
}

#[test]
fn operator_cancellation_stops_the_chain_and_records_what_ran() {
    let yaml = r#"
format_version: 1
name: Cancel
revision: 1
rules:
  - id: show
    name: Show
    armed: true
    trigger: { type: manual }
    actions:
      - { id: first, type: control.osc, device: proj, address: /a, args: [1] }
      - { id: pause, type: workflow.wait, ms: 60000 }
      - { id: second, type: control.osc, device: proj, address: /b, args: [1] }
      - { id: third, type: notify.operator, message: never }
"#;
    let sleeper = Arc::new(CancellingSleeper(Mutex::new(None)));
    let mut h = harness_with(yaml, &["proj"], sleeper.clone(), EngineConfig::default());
    *sleeper.0.lock().unwrap() = Some(h.engine.cancel_handle());

    let r = &h.engine.handle_event(Event::Manual { rule: None })[0];
    let summary: Vec<_> = r
        .action_results
        .iter()
        .map(|a| (a.action_id.as_str(), a.outcome.is_success(), a.outcome.is_skipped()))
        .collect();
    assert_eq!(
        summary,
        [("first", true, false), ("pause", true, false), ("second", false, true), ("third", false, true)]
    );
    assert_eq!(h.sent("proj").len(), 1, "second step must not run");
    assert!(h.sink.notices().is_empty());
    assert!(h.engine.cancel_handle().active().is_empty(), "finished chains are deregistered");
}

#[test]
fn device_dropping_out_mid_chain_degrades_it_and_can_trigger_a_rule() {
    let yaml = r#"
format_version: 1
name: Dropout
revision: 1
rules:
  - id: chain
    name: Chain
    armed: true
    trigger: { type: manual }
    actions:
      - { id: one, type: control.osc, device: proj, address: /a, args: [1] }
      - { id: two, type: control.osc, device: proj, address: /b, args: [1] }
    policy:
      on_failure: continue_chain
  - id: on-degraded
    name: On degraded
    armed: true
    trigger: { type: device_state, device: proj, state: degraded }
    actions:
      - { id: alert, type: notify.operator, message: "projector degraded", severity: high }
"#;
    let mut h = harness(yaml, &["proj"]);
    h.endpoints["proj"].set_faults(Faults {
        offline_after: Some(1),
        ..Faults::default()
    });
    let records = h.engine.handle_event(Event::Manual { rule: None });
    assert_eq!(records.len(), 2, "the degradation cascaded into a second rule");
    assert_eq!(records[0].overall_status, ExecutionStatus::PartialFailure);
    assert_eq!(records[1].rule_id.as_str(), "on-degraded");
    assert_eq!(h.engine.registry().health("proj"), Some(DeviceHealth::Degraded));
    assert_eq!(h.sink.notices()[0].message, "projector degraded");
}

#[test]
fn missed_heartbeat_takes_a_device_offline_and_fires_rules() {
    let yaml = r#"
format_version: 1
name: Heartbeat
revision: 1
rules:
  - id: lost
    name: Lost
    armed: true
    trigger: { type: heartbeat_missed, device: proj, after_ms: 5000 }
    actions:
      - { id: alert, type: log.incident, severity: high }
  - id: offline
    name: Offline
    armed: true
    trigger: { type: device_state, device: proj, state: offline }
    actions:
      - { id: alert, type: notify.operator, message: offline }
"#;
    let mut h = harness(yaml, &["proj"]);
    h.engine.registry_mut().register({
        let mut d = Device::new("proj", "proj");
        d.heartbeat_ms = Some(5_000);
        d
    });
    h.engine.device_heartbeat("proj").unwrap();
    h.clock.advance_millis(4_000);
    assert!(h.engine.tick().is_empty());
    h.clock.advance_millis(2_000);
    let records = h.engine.tick();
    let rules: Vec<_> = records.iter().map(|r| r.rule_id.as_str()).collect();
    assert_eq!(rules, ["lost", "offline"]);
    assert_eq!(h.engine.registry().health("proj"), Some(DeviceHealth::Offline));
    // Nothing more until it recovers.
    h.clock.advance_millis(60_000);
    assert!(h.engine.tick().is_empty());
    assert!(h.engine.device_heartbeat("proj").unwrap().is_empty());
    assert_eq!(h.engine.registry().health("proj"), Some(DeviceHealth::Online));
}

const SCHEDULED: &str = r#"
format_version: 1
name: Sched
revision: 1
rules:
  - id: start
    name: Start
    armed: true
    trigger: { type: schedule, at: "18:55", once: true }
    actions:
      - { id: a, type: notify.operator, message: start }
  - id: nightly
    name: Nightly
    armed: true
    trigger: { type: schedule, at: "18:55" }
    actions:
      - { id: a, type: notify.operator, message: nightly }
"#;

#[test]
fn schedules_fire_on_tick_once_per_minute() {
    let mut h = harness(SCHEDULED, &[]);
    h.clock.advance_millis((18 * 60 + 54) * 60_000);
    assert!(h.engine.tick().is_empty());
    h.clock.advance_millis(60_000);
    let fired: Vec<_> = h.engine.tick().iter().map(|r| r.rule_id.to_string()).collect();
    assert_eq!(fired.len(), 2);
    h.clock.advance_millis(20_000);
    assert!(h.engine.tick().is_empty(), "same minute");
}

#[test]
fn one_shot_schedules_do_not_refire_after_a_restart() {
    let mut first = harness(SCHEDULED, &[]);
    first.clock.advance_millis((18 * 60 + 55) * 60_000);
    assert_eq!(first.engine.tick().len(), 2);
    let saved = serde_json::to_string(&first.engine.snapshot()).unwrap();

    // The process is killed and restarted within the same minute...
    let mut second = harness(SCHEDULED, &[]);
    second.clock.advance_millis((18 * 60 + 55) * 60_000 + 30_000);
    second.engine.restore(serde_json::from_str(&saved).unwrap());
    assert!(second.engine.tick().is_empty(), "no duplicate firing in the same minute");

    // ...and the next day only the recurring rule fires.
    second.clock.advance_millis(86_400_000);
    let next: Vec<_> = second.engine.tick().iter().map(|r| r.rule_id.to_string()).collect();
    assert_eq!(next, ["nightly"]);
}

#[test]
fn execution_ids_keep_counting_across_a_restart() {
    let mut first = harness(SIMPLE_ARMED, &["proj"]);
    first.engine.handle_event(osc("/go", &[]));
    let state = first.engine.snapshot();
    let mut second = harness(SIMPLE_ARMED, &["proj"]);
    second.engine.restore(state);
    let r = &second.engine.handle_event(osc("/go", &[]))[0];
    assert_eq!(r.execution_id, "exec-000002");
}

#[test]
fn dmx_edge_triggers_use_the_previous_observation() {
    let yaml = r#"
format_version: 1
name: Dmx
revision: 1
rules:
  - id: up
    name: Up
    armed: true
    trigger: { type: dmx, universe: 1, channel: 4, comparison: crossed_above, value: 100 }
    actions:
      - { id: a, type: notify.operator, message: up }
"#;
    let mut h = harness(yaml, &[]);
    let dmx = |value| {
        Event::Dmx(DmxLevel {
            universe: 1,
            channel: 4,
            value,
        })
    };
    assert!(h.engine.handle_event(dmx(50)).is_empty());
    assert_eq!(h.engine.handle_event(dmx(150)).len(), 1, "crossed");
    assert!(h.engine.handle_event(dmx(200)).is_empty(), "already above");
    assert!(h.engine.handle_event(dmx(10)).is_empty());
    assert_eq!(h.engine.handle_event(dmx(101)).len(), 1, "crossed again");
}

#[test]
fn midi_and_osc_triggers_match_end_to_end() {
    let yaml = r#"
format_version: 1
name: Io
revision: 1
rules:
  - id: cue
    name: Cue
    armed: true
    trigger: { type: midi, message: note_on, channel: 0, number: 60 }
    actions:
      - { id: m, type: control.midi, device: synth, channel: 1, kind: note_on, number: 64, value: 100 }
      - { id: o, type: control.osc, device: proj, address: /cue, args: [1] }
"#;
    let mut h = harness(yaml, &["synth", "proj"]);
    let records = h.engine.handle_event(Event::Midi(MidiLevel {
        channel: 0,
        kind: "note_on".into(),
        number: 60,
        value: 127,
    }));
    assert_eq!(records[0].overall_status, ExecutionStatus::Success);
    assert_eq!(h.sent("synth").len(), 1);
    assert_eq!(h.sent("proj").len(), 1);
    assert!(h
        .engine
        .handle_event(Event::Midi(MidiLevel {
            channel: 1,
            kind: "note_on".into(),
            number: 60,
            value: 127
        }))
        .is_empty());
}

#[test]
fn run_rule_bypasses_the_trigger_and_unknown_rules_error() {
    let mut h = harness(SIMPLE_ARMED, &["proj"]);
    let records = h.engine.run_rule("go").unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].trigger_detail, Event::Manual { rule: Some("go".into()) });
    assert!(h.engine.run_rule("nope").is_err());
}

#[test]
fn invoked_rules_run_as_nested_executions() {
    let yaml = r#"
format_version: 1
name: Invoke
revision: 1
rules:
  - id: parent
    name: Parent
    armed: true
    trigger: { type: manual }
    actions:
      - { id: call, type: workflow.invoke_rule, rule: child }
  - id: child
    name: Child
    armed: true
    trigger: { type: api, name: never-fired }
    actions:
      - { id: a, type: control.osc, device: proj, address: /child, args: [1] }
"#;
    let mut h = harness(yaml, &["proj"]);
    let records = h.engine.handle_event(Event::Manual { rule: None });
    let ids: Vec<_> = records.iter().map(|r| r.rule_id.as_str()).collect();
    assert_eq!(ids, ["child", "parent"], "child completes first");
    assert!(records.iter().all(|r| r.overall_status == ExecutionStatus::Success));
    assert_eq!(h.sent("proj").len(), 1);
    assert!(records[1].action_results[0].detail.as_ref().unwrap().contains("child"));
}

#[test]
fn simulated_parents_simulate_their_children() {
    let yaml = r#"
format_version: 1
name: Invoke
revision: 1
rules:
  - id: parent
    name: Parent
    armed: false
    trigger: { type: manual }
    actions:
      - { id: call, type: workflow.invoke_rule, rule: child }
  - id: child
    name: Child
    armed: true
    trigger: { type: api }
    actions:
      - { id: a, type: control.osc, device: proj, address: /child, args: [1] }
"#;
    let mut h = harness(yaml, &["proj"]);
    let records = h.engine.handle_event(Event::Manual { rule: None });
    assert!(records.iter().all(|r| r.simulated));
    assert_eq!(h.endpoints["proj"].attempts(), 0);
}

#[test]
fn a_failing_child_fails_the_invoking_step() {
    let yaml = r#"
format_version: 1
name: Invoke
revision: 1
rules:
  - id: parent
    name: Parent
    armed: true
    trigger: { type: manual }
    actions:
      - { id: call, type: workflow.invoke_rule, rule: child }
  - id: child
    name: Child
    armed: true
    trigger: { type: api }
    actions:
      - { id: a, type: control.osc, device: proj, address: /child, args: [1] }
"#;
    let mut h = harness(yaml, &["proj"]);
    h.endpoints["proj"].set_offline(true);
    let records = h.engine.handle_event(Event::Manual { rule: None });
    assert_eq!(records[0].overall_status, ExecutionStatus::Failed);
    assert_eq!(records[1].overall_status, ExecutionStatus::Failed);
}

#[test]
fn slow_devices_time_out_and_the_chain_reports_it() {
    let yaml = r#"
format_version: 1
name: Slow
revision: 1
rules:
  - id: slow
    name: Slow
    armed: true
    trigger: { type: manual }
    actions:
      - { id: a, type: control.osc, device: proj, address: /a, args: [1], timeout_ms: 30 }
"#;
    let mut h = harness(yaml, &["proj"]);
    h.endpoints["proj"].set_faults(Faults {
        delay_ms: 500,
        ..Faults::default()
    });
    let r = &h.engine.handle_event(Event::Manual { rule: None })[0];
    assert!(matches!(outcomes(r)[0], ActionOutcome::TimedOut { after_ms: 30 }));
    assert_eq!(r.overall_status, ExecutionStatus::Failed);
}

#[test]
fn library_fragments_are_resolved() {
    let yaml = r#"
format_version: 1
name: Library
revision: 1
action_library:
  projector-on:
    type: control.osc
    address: /projector/power
    args: [1]
rules:
  - id: lib
    name: Lib
    armed: true
    trigger: { type: manual }
    actions:
      - { id: on, type: workflow.use_action, library: projector-on, device: proj }
"#;
    let mut h = harness(yaml, &["proj"]);
    let r = &h.engine.handle_event(Event::Manual { rule: None })[0];
    assert_eq!(r.overall_status, ExecutionStatus::Success);
    assert_eq!(
        h.sent("proj"),
        vec![Command::Osc {
            address: "/projector/power".into(),
            args: vec![1.0]
        }]
    );
}

#[test]
fn hot_reload_disarms_edited_and_new_rules_but_keeps_unchanged_ones() {
    let mut h = harness(SIMPLE_ARMED, &["proj"]);
    // Unchanged version: stays armed.
    h.engine.load_pack(RulePack::from_yaml_str(SIMPLE_ARMED).unwrap()).unwrap();
    assert!(h.engine.is_armed("go"));
    // Edited (version bump): comes up disarmed.
    let edited = SIMPLE_ARMED.replace("name: Go\n", "name: Go\n    version: 2\n");
    h.engine.load_pack(RulePack::from_yaml_str(&edited).unwrap()).unwrap();
    assert!(!h.engine.is_armed("go"));
    // A new rule declared armed in YAML is still disarmed on reload.
    let added = format!(
        "{edited}  - id: extra\n    name: Extra\n    armed: true\n    trigger: {{ type: manual }}\n    actions:\n      - {{ id: n, type: notify.operator, message: x }}\n"
    );
    h.engine.load_pack(RulePack::from_yaml_str(&added).unwrap()).unwrap();
    assert!(!h.engine.is_armed("extra"));
}

#[test]
fn invalid_packs_are_refused_by_the_engine() {
    let mut pack = RulePack::from_yaml_str(SIMPLE_ARMED).unwrap();
    pack.rules[0].actions[0].device = None; // a hardware write with no target
    let sink = Arc::new(MemorySink::default());
    let actions = Actions::new(sink.clone(), sink, Arc::new(NullSleeper));
    let result = Engine::new(
        pack,
        DeviceRegistry::new(),
        actions,
        Arc::new(FixedClock::default()),
        EngineConfig::default(),
    );
    assert!(result.is_err());
}

#[test]
fn unresolved_devices_are_reported() {
    let h = harness(SIMPLE_ARMED, &[]);
    let diagnostics = h.engine.check_devices();
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].location, "rules[0].actions[0].device");
    assert!(harness(SIMPLE_ARMED, &["proj"]).engine.check_devices().is_empty());
}

#[test]
fn identical_inputs_produce_identical_records() {
    let run = || {
        let mut h = harness(CONFLICT, &["proj"]);
        let mut all = h.engine.handle_event(osc("/go", &[]));
        all.extend(h.engine.handle_event(osc("/go", &[])));
        serde_json::to_string(&all).unwrap()
    };
    assert_eq!(run(), run());
}

#[test]
fn cascade_limit_bounds_event_loops() {
    // A rule that degrades... is not needed: simply flood dispatch through device-state rules that
    // re-trigger each other via invoke cannot loop; verify the configured cap is honoured anyway.
    let config = EngineConfig {
        max_cascade: 1,
        ..EngineConfig::default()
    };
    let yaml = r#"
format_version: 1
name: Dropout
revision: 1
rules:
  - id: chain
    name: Chain
    armed: true
    trigger: { type: manual }
    actions:
      - { id: one, type: control.osc, device: proj, address: /a, args: [1] }
  - id: on-degraded
    name: On degraded
    armed: true
    trigger: { type: device_state, device: proj, state: degraded }
    actions:
      - { id: alert, type: notify.operator, message: degraded }
"#;
    let mut h = harness_with(yaml, &["proj"], Arc::new(NullSleeper), config);
    h.endpoints["proj"].set_offline(true);
    let records = h.engine.handle_event(Event::Manual { rule: None });
    assert_eq!(records.len(), 1, "the follow-up event was dropped at the cap");
}

#[test]
fn scheduler_state_is_handed_to_the_hook_before_the_chain_runs() {
    let mut h = harness(SCHEDULED, &[]);
    let observed: Arc<Mutex<Vec<(usize, usize)>>> = Arc::new(Mutex::new(Vec::new()));
    let (seen, sink) = (observed.clone(), h.sink.clone());
    h.engine.set_state_hook(move |state| {
        // (one-shots recorded as fired, operator alerts raised so far)
        seen.lock().unwrap().push((state.scheduler.fired_once.len(), sink.notices().len()));
    });
    h.clock.advance_millis((18 * 60 + 55) * 60_000);
    assert_eq!(h.engine.tick().len(), 2);
    assert_eq!(
        *observed.lock().unwrap(),
        vec![(1, 0)],
        "the one-shot was already recorded as fired while no action had run yet"
    );
    // Nothing fired, so the hook is not called again.
    h.clock.advance_millis(1_000);
    assert!(h.engine.tick().is_empty());
    assert_eq!(observed.lock().unwrap().len(), 1);
}
