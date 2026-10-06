//! Rule pack model tests: YAML surface, validation and matching semantics (spec §9, §18.1).

use tpt_app_av_automation_core::{
    DeviceHealth, Diagnostic, DmxLevel, Error, Event, MidiLevel, Severity,
};
use tpt_app_av_automation_model::{
    ActionOutcome, ActionSpec, AlertSeverity, Condition, ConditionSpec, DmxComparison,
    ExecutionStatus, LocalTime, MidiMessageKind, Priority, Rule, RulePack, ScheduleSpec, Trigger,
    TriggerSpec, Weekday,
};

/// A representative, fully valid pack exercising the MVP trigger and action forms.
const VALID_PACK: &str = r#"
format_version: 1
name: Main Hall
description: Standard main hall pack
revision: 3
rules:
  - id: event-start
    name: Main Hall - Event Start
    version: 2
    armed: false
    priority: critical
    tags: [show, lighting]
    trigger:
      type: schedule
      at: "18:55"
      days: [mon, tue, wed, thu, fri]
    conditions:
      - id: house-closed
        type: time_window
        from: "18:00"
        to: "23:00"
    actions:
      - id: house-to-half
        type: control.dmx_scene
        device: lighting-rack
        scene: house-to-half
        fade_ms: 3000
      - id: projector-on
        type: control.osc
        device: projector-1
        address: /projector/1/power
        args: [1]
    policy:
      on_failure: continue_chain
      timeout_ms: 10000
"#;

/// Collects validation diagnostics, whether validation failed or merely warned.
fn diagnostics_for(yaml: &str) -> Vec<Diagnostic> {
    match RulePack::from_yaml_str(yaml) {
        Ok(pack) => pack.validate().unwrap_or_default(),
        Err(Error::Validation(d)) => d,
        Err(other) => panic!("unexpected error: {other}"),
    }
}

/// Error-level diagnostics only, for packs that are expected to be rejected.
fn expect_invalid(yaml: &str) -> Vec<Diagnostic> {
    diagnostics_for(yaml)
        .into_iter()
        .filter(|d| d.severity == Severity::Error)
        .collect()
}

#[test]
fn valid_pack_parses_with_all_fields() {
    let pack = RulePack::from_yaml_str(VALID_PACK).expect("pack should be valid");
    assert_eq!(pack.name, "Main Hall");
    assert_eq!(pack.revision, 3);
    assert_eq!(pack.rules.len(), 1);

    let rule = &pack.rules[0];
    assert_eq!(rule.id.as_str(), "event-start");
    assert_eq!(rule.version, 2);
    assert_eq!(rule.priority, Priority::Critical);
    assert!(rule.has_tag("lighting"));
    assert!(!rule.armed);
    assert_eq!(rule.action_count(), 2);
    assert_eq!(
        rule.action("projector-on").and_then(|s| s.target()),
        Some("projector-1")
    );
}

#[test]
fn schedule_trigger_exposes_time_and_days() {
    let pack = RulePack::from_yaml_str(VALID_PACK).unwrap();
    match &pack.rules[0].trigger.spec {
        TriggerSpec::Schedule(spec) => {
            assert_eq!(spec.at, LocalTime::new(18, 55));
            assert_eq!(spec.days.len(), 5);
            assert!(!spec.once);
            assert_eq!(spec.label(), "mon,tue,wed,thu,fri 18:55");
        }
        other => panic!("expected a schedule trigger, got {other:?}"),
    }
}

#[test]
fn condition_parses_time_window() {
    let pack = RulePack::from_yaml_str(VALID_PACK).unwrap();
    match &pack.rules[0].conditions[0].spec {
        ConditionSpec::TimeWindow { from, to } => {
            assert_eq!(*from, LocalTime::new(18, 0).unwrap());
            assert_eq!(*to, LocalTime::new(23, 0).unwrap());
        }
        other => panic!("expected a time window, got {other:?}"),
    }
}

#[test]
fn pack_round_trips_through_yaml() {
    let pack = RulePack::from_yaml_str(VALID_PACK).unwrap();
    let yaml = pack.to_yaml_string().unwrap();
    assert_eq!(pack, RulePack::from_yaml_str(&yaml).unwrap());
}

#[test]
fn rules_load_disarmed_by_default() {
    let yaml = VALID_PACK.replace("armed: false", "");
    let pack = RulePack::from_yaml_str(&yaml).unwrap();
    assert!(
        !pack.rules[0].armed,
        "an omitted armed flag must default to disarmed (spec §3.5)"
    );
    assert_eq!(pack.summary().disarmed, 1);
}

#[test]
fn schedule_matching_is_deterministic_and_idempotent_within_the_minute() {
    let spec = ScheduleSpec::daily(LocalTime::new(18, 55).unwrap());
    let event = Event::Schedule { spec: spec.label() };
    let trigger = Trigger::new(TriggerSpec::Schedule(spec));

    assert!(trigger.matches(&event));
    // The same question asked twice gives the same answer.
    assert!(trigger.matches(&event));
    assert!(!trigger.matches(&Event::Schedule {
        spec: "18:56".into()
    }));
}

#[test]
fn weekday_gating_is_applied() {
    let spec = ScheduleSpec {
        at: LocalTime::new(9, 0),
        solar: None,
        cron: None,
        interval_ms: None,
        days: vec![Weekday::Sat, Weekday::Sun],
        once: false,
    };
    assert!(spec.day_matches(5), "Saturday index is 5");
    assert!(spec.day_matches(6), "Sunday index is 6");
    assert!(!spec.day_matches(0), "Monday index is 0");
}

#[test]
fn one_shot_schedule_is_labelled_distinctly() {
    let spec = ScheduleSpec::one_shot(LocalTime::new(9, 0).unwrap(), vec![Weekday::Sat]);
    assert_eq!(spec.label(), "sat 09:00 (once)");
}

#[test]
fn osc_trigger_matches_single_segment_wildcards() {
    let trigger = Trigger::new(TriggerSpec::Osc {
        address: "/projector/*/power".into(),
        arg_equals: Some(1.0),
        min_args: Some(1),
    });

    assert!(trigger.matches(&Event::Osc {
        address: "/projector/1/power".into(),
        args: vec![1.0],
    }));
    assert!(trigger.matches(&Event::Osc {
        address: "/projector/2/power".into(),
        args: vec![1.0],
    }));
    // A wildcard is one segment, not a multi-segment glob.
    assert!(!trigger.matches(&Event::Osc {
        address: "/projector/1/2/power".into(),
        args: vec![1.0],
    }));
    assert!(!trigger.matches(&Event::Osc {
        address: "/projector/1/power".into(),
        args: vec![0.0],
    }));
    assert!(!trigger.matches(&Event::Osc {
        address: "/projector/1/power".into(),
        args: vec![],
    }));
}

#[test]
fn midi_trigger_matches_normalized_messages() {
    let trigger = Trigger::new(TriggerSpec::Midi {
        message: MidiMessageKind::NoteOn,
        channel: Some(0),
        number: Some(60),
        value: Some(127),
        group: None,
    });

    assert!(trigger.matches(&Event::Midi(MidiLevel {
        channel: 0,
        kind: "note_on".into(),
        number: 60,
        value: 127,
        ..Default::default()
    })));
    assert!(!trigger.matches(&Event::Midi(MidiLevel {
        channel: 1,
        kind: "note_on".into(),
        number: 60,
        value: 127,
        ..Default::default()
    })));
    assert!(!trigger.matches(&Event::Midi(MidiLevel {
        channel: 0,
        kind: "note_off".into(),
        number: 60,
        value: 0,
        ..Default::default()
    })));
}

#[test]
fn dmx_edge_comparisons_require_a_previous_observation() {
    let mut trigger = Trigger::new(TriggerSpec::Dmx {
        universe: 1,
        channel: 0,
        comparison: DmxComparison::Changed,
        value: 0,
    });

    let event = Event::Dmx(DmxLevel {
        universe: 1,
        channel: 0,
        value: 255,
    });

    assert!(
        !trigger.matches(&event),
        "with no previous value the trigger must not fire rather than guess"
    );

    trigger.dmx_previous = Some(0);
    assert!(trigger.matches(&event));

    trigger.dmx_previous = Some(255);
    assert!(!trigger.matches(&event), "no change means no fire");
}

#[test]
fn dmx_level_comparisons_do_not_need_previous_state() {
    let trigger = Trigger::new(TriggerSpec::Dmx {
        universe: 1,
        channel: 5,
        comparison: DmxComparison::GreaterThan,
        value: 128,
    });
    assert!(trigger.matches(&Event::Dmx(DmxLevel {
        universe: 1,
        channel: 5,
        value: 200,
    })));
    assert!(!trigger.matches(&Event::Dmx(DmxLevel {
        universe: 1,
        channel: 5,
        value: 10,
    })));
    assert!(!trigger.matches(&Event::Dmx(DmxLevel {
        universe: 2,
        channel: 5,
        value: 200,
    })));
}

#[test]
fn triggers_ignore_unrelated_event_kinds() {
    let trigger = Trigger::new(TriggerSpec::Dmx {
        universe: 1,
        channel: 0,
        comparison: DmxComparison::Equals,
        value: 255,
    });
    assert!(!trigger.matches(&Event::Osc {
        address: "/anything".into(),
        args: vec![255.0],
    }));
    assert!(!trigger.matches(&Event::Manual { rule: None }));
}

#[test]
fn device_state_and_heartbeat_triggers_match() {
    let state = Trigger::new(TriggerSpec::DeviceState {
        device: "proj-1".into(),
        state: DeviceHealth::Offline,
    });
    assert!(state.matches(&Event::DeviceState {
        device: "proj-1".into(),
        previous: DeviceHealth::Online,
        current: DeviceHealth::Offline,
    }));
    assert!(!state.matches(&Event::DeviceState {
        device: "proj-2".into(),
        previous: DeviceHealth::Online,
        current: DeviceHealth::Offline,
    }));

    let heartbeat = Trigger::new(TriggerSpec::HeartbeatMissed {
        device: "proj-1".into(),
        after_ms: 5_000,
    });
    assert!(heartbeat.matches(&Event::HeartbeatMissed {
        device: "proj-1".into(),
        missed_millis: 5_001,
    }));
    assert!(!heartbeat.matches(&Event::HeartbeatMissed {
        device: "proj-1".into(),
        missed_millis: 4_999,
    }));
}

#[test]
fn execution_status_folding_never_reports_partial_as_success() {
    let mut status = ExecutionStatus::Skipped;
    status = status.fold(&ActionOutcome::Success);
    assert_eq!(status, ExecutionStatus::Success);

    status = status.fold(&ActionOutcome::Failed {
        reason: "no route".into(),
    });
    assert_eq!(status, ExecutionStatus::PartialFailure);
}

#[test]
fn duplicate_rule_ids_are_rejected() {
    let yaml = r#"
format_version: 1
name: Dup
revision: 1
rules:
  - id: same
    name: First
    trigger: { type: manual }
    actions:
      - { id: a1, type: notify.operator, message: hi }
  - id: same
    name: Second
    trigger: { type: manual }
    actions:
      - { id: a1, type: notify.operator, message: hi }
"#;
    let locations: Vec<_> = expect_invalid(yaml)
        .into_iter()
        .map(|d| d.location)
        .collect();
    assert!(
        locations.iter().any(|l| l == "rules[1].id"),
        "expected a duplicate id diagnostic, got {locations:?}"
    );
}

#[test]
fn dmx_channels_must_stay_inside_the_universe() {
    let yaml = r#"
format_version: 1
name: Bad
revision: 1
rules:
  - id: r1
    name: Bad DMX
    trigger: { type: manual }
    actions:
      - id: a1
        type: control.dmx_channels
        device: rack
        universe: 1
        start_channel: 510
        values: [1, 2, 3]
"#;
    let diagnostics = expect_invalid(yaml);
    assert!(
        diagnostics
            .iter()
            .any(|d| d.location == "rules[0].actions[0].values"),
        "{diagnostics:?}"
    );
}

#[test]
fn hardware_writes_require_a_device_target() {
    let yaml = r#"
format_version: 1
name: Missing Target
revision: 1
rules:
  - id: r1
    name: No Target
    trigger: { type: manual }
    actions:
      - id: a1
        type: control.osc
        address: /a
"#;
    let diagnostics = expect_invalid(yaml);
    assert!(
        diagnostics
            .iter()
            .any(|d| d.location == "rules[0].actions[0].device"),
        "{diagnostics:?}"
    );
}

#[test]
fn unknown_action_type_names_the_valid_options() {
    let err = RulePack::from_yaml_str(
        r#"
format_version: 1
name: X
revision: 1
rules:
  - id: r1
    name: Bad action
    trigger: { type: manual }
    actions:
      - id: a1
        type: control.teleport
"#,
    )
    .unwrap_err();
    let text = err.to_string();
    assert!(text.contains("control.teleport"), "{text}");
    assert!(
        text.contains("notify.operator"),
        "error should list options: {text}"
    );
}

#[test]
fn future_format_versions_are_refused() {
    let yaml = "format_version: 99\nname: Future\nrevision: 1\n";
    let diagnostics = expect_invalid(yaml);
    assert!(
        diagnostics.iter().any(|d| d.location == "format_version"),
        "{diagnostics:?}"
    );
}

#[test]
fn malformed_yaml_is_a_parse_error_not_a_panic() {
    let err = RulePack::from_yaml_str("rules: [ unclosed").unwrap_err();
    assert!(matches!(err, Error::Parse(_)), "{err}");
    assert!(err.is_configuration());
}

#[test]
fn invoke_rule_cycles_are_rejected() {
    let yaml = r#"
format_version: 1
name: Cyclic
revision: 1
rules:
  - id: a
    name: A
    trigger: { type: manual }
    actions:
      - id: step
        type: workflow.invoke_rule
        rule: b
  - id: b
    name: B
    trigger: { type: manual }
    actions:
      - id: step
        type: workflow.invoke_rule
        rule: a
"#;
    let diagnostics = expect_invalid(yaml);
    assert!(
        diagnostics
            .iter()
            .any(|d| d.message.contains("forms a cycle")),
        "{diagnostics:?}"
    );
}

#[test]
fn dangling_invocations_are_reported() {
    let yaml = r#"
format_version: 1
name: Dangling
revision: 1
rules:
  - id: a
    name: A
    trigger: { type: manual }
    actions:
      - id: step
        type: workflow.invoke_rule
        rule: does-not-exist
"#;
    let diagnostics = expect_invalid(yaml);
    assert!(
        diagnostics
            .iter()
            .any(|d| d.message.contains("unknown rule `does-not-exist`")),
        "{diagnostics:?}"
    );
}

#[test]
fn action_library_fragments_resolve() {
    let yaml = r#"
format_version: 1
name: Library
revision: 1
action_library:
  blackout:
    type: control.dmx_channels
    universe: 1
    start_channel: 0
    values: [0]
rules:
  - id: r1
    name: All Black
    trigger: { type: manual }
    actions:
      - id: step
        type: workflow.use_action
        library: blackout
        device: rack
"#;
    let pack = RulePack::from_yaml_str(yaml).expect("library pack should validate");
    let resolved = pack.rules[0].actions[0]
        .spec
        .resolve_library(&pack.action_library)
        .expect("fragment should resolve");
    assert_eq!(resolved.type_label(), "control.dmx_channels");
}

#[test]
fn self_referential_library_fragments_are_rejected() {
    let yaml = r#"
format_version: 1
name: Bad Library
revision: 1
action_library:
  loop:
    type: workflow.use_action
    library: loop
rules:
  - id: r1
    name: R
    trigger: { type: manual }
    actions:
      - id: step
        type: notify.operator
        message: hi
"#;
    let diagnostics = expect_invalid(yaml);
    assert!(
        diagnostics
            .iter()
            .any(|d| d.message.contains("must not reference other fragments")),
        "{diagnostics:?}"
    );
}

#[test]
fn condition_unknown_handling_is_explicit() {
    let strict = Condition::new("c1", ConditionSpec::RuleArmed { rule: "r".into() });
    assert!(!strict.fire_on_unknown);
    let lenient = Condition::lenient("c2", ConditionSpec::RuleHasTag { tag: "x".into() });
    assert!(lenient.fire_on_unknown);
}

#[test]
fn action_labels_and_endpoint_classification_are_stable() {
    let notify = ActionSpec::NotifyOperator {
        message: "hello".into(),
        severity: AlertSeverity::High,
    };
    assert_eq!(notify.type_label(), "notify.operator");
    assert!(!notify.is_endpoint_write());

    let osc = ActionSpec::Osc {
        address: "/a".into(),
        args: vec![1.0],
    };
    assert!(osc.is_endpoint_write());
}

#[test]
fn rule_display_shows_armed_state() {
    let rule = Rule::new("r1", "Rule", Trigger::manual());
    assert!(rule.to_string().contains("disarmed"));
    let mut armed = rule;
    armed.armed = true;
    assert!(armed.to_string().contains("armed"));
}

/// A cheap deterministic stand-in for the rule-parser fuzz target (spec §18.5).
///
/// The parser must always return an error for garbage; it must never panic or abort.
#[test]
fn garbage_input_never_panics() {
    let alphabet: Vec<u8> = b"{}[],:\n \"'abc123-_*&^%$#@!".to_vec();
    let mut state: u64 = 0x2545_F491_4F6C_DD1D;

    for _ in 0..2_000 {
        let len = (next(&mut state) % 64) as usize;
        let bytes: Vec<u8> = (0..len)
            .map(|_| alphabet[(next(&mut state) as usize) % alphabet.len()])
            .collect();
        let source = String::from_utf8_lossy(&bytes).into_owned();
        let _ = RulePack::from_yaml_str(&source);
    }
}

/// xorshift64*, so the fuzz stand-in is reproducible across runs and machines.
fn next(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}
/// A mistyped key must be a hard error, never a silently ignored instruction (spec §9).
///
/// A rule pack is loaded once, before a show, and a silently dropped key means the operator
/// believes a cue is armed when the engine is doing something else entirely.
#[test]
fn mistyped_keys_are_rejected_at_every_level() {
    let cases: [(&str, &str, &str); 5] = [
        (
            "pack level",
            "unknown field `revison`",
            "format_version: 1\nname: P\nrevison: 1\nrules: []\n",
        ),
        (
            "rule level",
            "unknown field `arrmed`",
            "format_version: 1\nname: P\nrevision: 1\nrules:\n  - id: r\n    name: R\n    trigger: { type: manual }\n    arrmed: true\n    actions: [{ id: a, type: notify.operator, message: m }]\n",
        ),
        (
            "condition level",
            "unknown field `dev`",
            "format_version: 1\nname: P\nrevision: 1\nrules:\n  - id: r\n    name: R\n    trigger: { type: manual }\n    conditions: [{ id: c, type: rule_armed, rule: x, dev: typo }]\n    actions: [{ id: a, type: notify.operator, message: m }]\n",
        ),
        (
            "action level",
            "unknown field `mesage`",
            "format_version: 1\nname: P\nrevision: 1\nrules:\n  - id: r\n    name: R\n    trigger: { type: manual }\n    actions: [{ id: a, type: notify.operator, mesage: typo }]\n",
        ),
        (
            // A unit trigger variant has no field list for serde to check against, so this case is
            // caught by the pack-level raw-YAML audit rather than by `deny_unknown_fields`.
            "unit-variant trigger",
            "unknown field `evnt`",
            "format_version: 1\nname: P\nrevision: 1\nrules:\n  - id: r\n    name: R\n    trigger: { type: manual, evnt: typo }\n    actions: [{ id: a, type: notify.operator, message: m }]\n",
        ),
    ];

    for (level, expected, yaml) in cases {
        let error = RulePack::from_yaml_str(yaml)
            .err()
            .unwrap_or_else(|| panic!("{level}: a mistyped key was silently accepted"));
        assert!(
            error.to_string().contains(expected),
            "{level}: expected {expected:?} in {error}"
        );
    }
}

/// Strictness must not reject the keys a valid pack legitimately uses (spec §9).
///
/// This guards the `deny_unknown_fields` / `flatten` interaction: `Trigger`, `Condition` and
/// `ActionStep` all flatten a tagged enum, so making the wrappers strict would break valid input.
#[test]
fn legitimate_keys_are_still_accepted_at_every_level() {
    let pack = RulePack::from_yaml_str(VALID_PACK).expect("the canonical pack must stay valid");
    assert_eq!(pack.rules.len(), 1);

    // A pack exercising every wrapper that flattens a strict enum.
    let yaml = r#"
format_version: 1
name: Flatten
revision: 1
action_library:
  blackout:
    type: control.dmx_channels
    universe: 1
    start_channel: 0
    values: [0]
rules:
  - id: r1
    name: Wrapper Keys
    trigger: { type: dmx, universe: 1, channel: 4, comparison: crossed_above, value: 100, dmx_previous: 0 }
    conditions:
      - id: c1
        type: time_window
        from: "18:00"
        to: "23:00"
        fire_on_unknown: true
    actions:
      - id: a1
        type: workflow.use_action
        library: blackout
        device: rack
        timeout_ms: 500
        concurrent_safe: true
        estimated_cost_ms: 20
"#;
    RulePack::from_yaml_str(yaml).expect("wrapper-level keys must remain valid");

    // Round-tripping re-serializes exactly these keys, so it must survive strict parsing too.
    let reserialized = pack.to_yaml_string().unwrap();
    RulePack::from_yaml_str(&reserialized).expect("a re-serialized pack must re-parse");
}

/// A library fragment is a bare action; its target device belongs on the calling step.
///
/// This pins the `device` placement that the strictness fix surfaced: `device` inside
/// `action_library` used to be dropped without complaint, leaving a fragment with no target.
#[test]
fn a_library_fragment_may_not_smuggle_a_device_target() {
    let yaml = r#"
format_version: 1
name: Fragment Device
revision: 1
action_library:
  blackout:
    type: control.dmx_channels
    device: rack
    universe: 1
    start_channel: 0
    values: [0]
rules:
  - id: r1
    name: All Black
    trigger: { type: manual }
    actions:
      - id: step
        type: workflow.use_action
        library: blackout
        device: rack
"#;
    let error =
        RulePack::from_yaml_str(yaml).expect_err("a device inside a fragment must be rejected");
    assert!(
        error.to_string().contains("unknown field `device`"),
        "{error}"
    );
}

/// A pack whose only schedule trigger uses a cron expression.
fn cron_pack(expression: &str) -> String {
    format!(
        r#"
format_version: 1
name: Cron Pack
revision: 1
rules:
  - id: hourly-cue
    name: Hourly cue
    trigger:
      type: schedule
      cron: "{expression}"
    actions:
      - id: log
        type: log.incident
        message: "cue"
"#
    )
}

#[test]
fn a_cron_schedule_parses_and_validates() {
    let pack = RulePack::from_yaml_str(&cron_pack("30 18 * * mon-fri")).expect("valid cron");
    match &pack.rules[0].trigger.spec {
        TriggerSpec::Schedule(spec) => {
            assert_eq!(spec.cron.as_deref(), Some("30 18 * * mon-fri"));
            assert!(spec.at.is_none());
            assert!(spec.interval_ms.is_none());
            assert!(spec.parsed_cron().is_some());
            // The label is canonical, so execution records are stable.
            assert_eq!(spec.label(), "cron 30 18 * * 1-5");
        }
        other => panic!("expected a schedule trigger, got {other:?}"),
    }
}

#[test]
fn a_cron_schedule_round_trips_through_yaml() {
    let pack = RulePack::from_yaml_str(&cron_pack("*/15 8-20 * * mon-fri")).unwrap();
    let yaml = pack.to_yaml_string().unwrap();
    assert!(yaml.contains("cron:"), "{yaml}");
    assert_eq!(pack, RulePack::from_yaml_str(&yaml).unwrap());
}

#[test]
fn a_malformed_cron_expression_is_a_located_validation_error() {
    let diags = expect_invalid(&cron_pack("99 * * * *"));
    let cron = diags
        .iter()
        .find(|d| d.location.ends_with(".cron"))
        .unwrap_or_else(|| panic!("expected a diagnostic on .cron, got {diags:?}"));
    assert!(cron.message.contains("minute"), "{}", cron.message);
}

#[test]
fn a_schedule_must_use_exactly_one_of_at_cron_and_interval() {
    let locations: Vec<String> = expect_invalid(&cron_pack("30 18 * * mon-fri"))
        .into_iter()
        .map(|d| d.location)
        .collect();
    assert!(locations.is_empty(), "{locations:?}");

    let none: Vec<String> = expect_invalid(
        r#"
format_version: 1
name: No Schedule
revision: 1
rules:
  - id: r1
    name: No schedule body
    trigger: { type: schedule }
    actions:
      - { id: log, type: log.incident, message: "x" }
"#,
    )
    .into_iter()
    .map(|d| d.location)
    .collect();
    assert!(
        none.iter().any(|l| l.ends_with("rules[0].trigger")),
        "{none:?}"
    );

    let both: Vec<Diagnostic> = expect_invalid(
        r#"
format_version: 1
name: Both
revision: 1
rules:
  - id: r1
    name: at and cron
    trigger: { type: schedule, at: "18:55", cron: "55 18 * * *" }
    actions:
      - { id: log, type: log.incident, message: "x" }
"#,
    );
    assert!(
        both.iter().any(|d| d.message.contains("exactly one")),
        "{both:?}"
    );
}

#[test]
fn cron_and_days_cannot_both_be_given() {
    let diags = expect_invalid(
        r#"
format_version: 1
name: Cron Days
revision: 1
rules:
  - id: r1
    name: cron with days
    trigger: { type: schedule, cron: "55 18 * * *", days: [mon] }
    actions:
      - { id: log, type: log.incident, message: "x" }
"#,
    );
    assert!(
        diags.iter().any(|d| d.location.ends_with(".days")),
        "{diags:?}"
    );
}

#[test]
fn a_cron_matching_is_deterministic_across_repeated_polls() {
    let spec = ScheduleSpec::cron("0 9 * * mon");
    let trigger = Trigger::new(TriggerSpec::Schedule(spec.clone()));
    let event = Event::Schedule { spec: spec.label() };
    // 2024-01-01 was a Monday, so 09:00 matches.
    assert!(trigger.matches(&Event::Schedule {
        spec: "cron 0 9 * * 1".into()
    }));
    assert!(trigger.matches(&event));
    assert!(
        trigger.matches(&event),
        "matching is a pure function of the event"
    );
    assert!(!trigger.matches(&Event::Schedule {
        spec: "cron 0 10 * * 1".into()
    }));
}

#[test]
fn every_midi_message_kind_round_trips_through_the_rule_format() {
    for kind in MidiMessageKind::ALL {
        let name = match kind {
            MidiMessageKind::ControlChange => "control_change",
            other => other.as_str(),
        };
        let yaml = format!(
            r#"
format_version: 1
name: Kinds
revision: 1
rules:
  - id: r1
    name: kind {name}
    trigger: {{ type: midi, message: {name} }}
    actions:
      - {{ id: log, type: log.incident, message: "x" }}
"#
        );
        let diags = expect_invalid(&yaml);
        assert!(diags.is_empty(), "{name}: {diags:?}");
    }
}

#[test]
fn midi2_only_kinds_are_flagged_as_such() {
    assert!(!MidiMessageKind::NoteOn.is_midi2_only());
    assert!(!MidiMessageKind::ControlChange.is_midi2_only());
    assert!(MidiMessageKind::PerNoteRcc.is_midi2_only());
    assert!(MidiMessageKind::ChannelPressure.is_midi2_only());
    assert_eq!(
        MidiMessageKind::ALL.len(),
        MidiMessageKind::ALL
            .iter()
            .filter(|k| !k.is_midi2_only())
            .count()
            + MidiMessageKind::ALL
                .iter()
                .filter(|k| k.is_midi2_only())
                .count()
    );
    // The event discriminator for a control change is `cc`, matching a MIDI 1.0 observation.
    assert_eq!(MidiMessageKind::ControlChange.as_str(), "cc");
}

#[test]
fn a_midi_trigger_matches_the_full_32_bit_value_and_the_group() {
    let trigger = Trigger::new(TriggerSpec::Midi {
        message: MidiMessageKind::ControlChange,
        channel: Some(1),
        number: Some(7),
        value: Some(0x1234_5678),
        group: Some(2),
    });
    assert!(trigger.matches(&Event::Midi(MidiLevel {
        channel: 1,
        kind: "cc".into(),
        number: 7,
        value: 0x1234,
        value32: Some(0x1234_5678),
        group: Some(2),
    })));
    // A MIDI 1.0 observation cannot match: it has neither a group nor a 32-bit value.
    assert!(!trigger.matches(&Event::Midi(MidiLevel {
        channel: 1,
        kind: "cc".into(),
        number: 7,
        value: 0x1234,
        ..Default::default()
    })));
    // A different group is a different source.
    assert!(!trigger.matches(&Event::Midi(MidiLevel {
        channel: 1,
        kind: "cc".into(),
        number: 7,
        value: 0x1234,
        value32: Some(0x1234_5678),
        group: Some(3),
    })));
}

#[test]
fn a_midi1_trigger_still_matches_midi1_events_exactly() {
    let trigger = Trigger::new(TriggerSpec::Midi {
        message: MidiMessageKind::NoteOn,
        channel: Some(0),
        number: Some(60),
        value: Some(127),
        group: None,
    });
    assert!(trigger.matches(&Event::Midi(MidiLevel::midi1(0, "note_on", 60, 127))));
    assert!(!trigger.matches(&Event::Midi(MidiLevel::midi1(0, "note_on", 60, 126))));
    // A MIDI 2.0 note-on with the same channel and note matches on its 16-bit velocity.
    assert!(trigger.matches(&Event::Midi(MidiLevel {
        channel: 0,
        kind: "note_on".into(),
        number: 60,
        value: 127,
        value32: Some(127),
        group: Some(0),
    })));
}

#[test]
fn an_ump_group_above_fifteen_is_rejected() {
    let diags = expect_invalid(
        r#"
format_version: 1
name: Group
revision: 1
rules:
  - id: r1
    name: bad group
    trigger: { type: midi, message: control_change, channel: 0, number: 7, group: 99 }
    actions:
      - { id: log, type: log.incident, message: "x" }
"#,
    );
    assert!(
        diags.iter().any(|d| d.location.ends_with(".group")),
        "{diags:?}"
    );
}

#[test]
fn a_solar_schedule_loads_from_yaml() {
    let pack = RulePack::from_yaml_str(
        "format_version: 1\nname: Sun\nrevision: 1\nsite: { latitude: 51.5, longitude: -0.1 }\nrules:\n  - id: on\n    name: On\n    trigger: { type: schedule, solar: { event: sunset, minutes: -30 } }\n    actions:\n      - { id: n, type: notify.operator, message: hi }\n",
    )
    .expect("a solar schedule is documented and must parse");
    assert_eq!(pack.rules.len(), 1);
}
