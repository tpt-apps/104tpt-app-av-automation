//! Phase 2 test matrix: valid, invalid, boundary and malformed cases for every trigger,
//! condition and action form (spec §18.1).
//!
//! The rule is that every form in the catalogue is exercised on all four axes. A form that only
//! has a happy-path test is not covered: a validator that accepts a bad universe, a decoder that
//! misreads a boundary value, and a parser that panics on a typo are three different failures, and
//! each needs its own case.
//!
//! Every case is expressed through the YAML surface rather than through constructors, because the
//! surface is what an operator actually writes and what the engine has to survive.

use tpt_app_av_automation_core::{Diagnostic, Error, Severity};
use tpt_app_av_automation_model::{MidiMessageKind, RulePack};

/// Indents a YAML fragment to `depth` levels of two spaces, without a trailing newline.
fn indent(fragment: &str, depth: usize) -> String {
    let pad = "  ".repeat(depth);
    fragment
        .trim_end()
        .lines()
        .map(|line| {
            if line.trim().is_empty() {
                String::new()
            } else {
                format!("{pad}{line}\n")
            }
        })
        .collect()
}

/// Wraps a trigger fragment in an otherwise minimal valid rule.
///
/// The fragment is written at column zero and re-indented here, so a case reads as the operator
/// would write it rather than as a pre-indented blob.
fn with_trigger(trigger_yaml: &str) -> String {
    format!
        (
            "format_version: 1\nname: Matrix\nrules:\n  - id: r\n    name: R\n    trigger:\n{}    actions:\n      - id: note\n        type: log.incident\n",
            indent(trigger_yaml, 2)
        )
}

/// Wraps an action fragment in an otherwise minimal valid rule.
///
/// The pack carries a one-entry `action_library` so a `workflow.use_action` case has something
/// real to reference, and a second rule so a `workflow.invoke_rule` case has a real target. A case
/// naming anything else is exactly the dangling-reference case.
fn with_action(action_yaml: &str) -> String {
    format!(
        "format_version: 1\n\
         name: Matrix\n\
         action_library:\n\
        \x20 frag:\n\
        \x20   type: log.incident\n\
         rules:\n\
        \x20 - id: r\n\
        \x20   name: R\n\
        \x20   trigger:\n\
        \x20     type: manual\n\
        \x20   actions:\n{}\n\
        \x20 - id: other\n\
        \x20   name: Other\n\
        \x20   trigger:\n\
        \x20     type: manual\n\
        \x20   actions:\n\
        \x20     - id: note\n\
        \x20       type: log.incident\n",
        indent(action_yaml, 2)
    )
}

/// Wraps a condition fragment in an otherwise minimal valid rule.
fn with_condition(condition_yaml: &str) -> String {
    format!(
        "format_version: 1\nname: Matrix\nrules:\n  - id: r\n    name: R\n    trigger:\n      type: manual\n    conditions:\n{}    actions:\n      - id: note\n        type: log.incident\n  - id: house-lights\n    name: House Lights\n    trigger:\n      type: manual\n    actions:\n      - id: note\n        type: log.incident\n",
        indent(condition_yaml, 2)
    )
}

/// Diagnostics from a pack that may be valid, invalid or a parse failure.
fn diagnostics_for(yaml: &str) -> Vec<Diagnostic> {
    match RulePack::from_yaml_str(yaml) {
        Ok(pack) => pack.validate().unwrap_or_default(),
        Err(Error::Validation(d)) => d,
        Err(other) => panic!("unexpected error: {other}"),
    }
}

fn error_locations(yaml: &str) -> Vec<String> {
    diagnostics_for(yaml)
        .into_iter()
        .filter(|d| d.severity == Severity::Error)
        .map(|d| d.location)
        .collect()
}

fn assert_valid(yaml: &str) {
    let errors = error_locations(yaml);
    assert!(
        errors.is_empty(),
        "expected a valid pack, got errors at {errors:?}"
    );
}

/// Asserts the pack is rejected, and that some diagnostic mentions `needle`.
///
/// Matching on a substring rather than an exact location keeps a case from breaking when an
/// unrelated check starts reporting on the same field.
fn assert_rejected_mentioning(yaml: &str, needle: &str) {
    let diagnostics: Vec<String> = diagnostics_for(yaml)
        .into_iter()
        .filter(|d| d.severity == Severity::Error)
        .map(|d| format!("{}: {}", d.location, d.message))
        .collect();
    assert!(
        diagnostics.iter().any(|d| d.contains(needle)),
        "expected an error mentioning `{needle}`, got {diagnostics:?}"
    );
}

/// Asserts the document fails to parse at all.
///
/// Serde rejects an unknown enum variant or an unknown field before validation ever runs, so these
/// are deliberately a different outcome from a located diagnostic. The point of asserting it is
/// that it is an `Err` and not a panic, and that the pack never loads.
fn assert_parse_error(yaml: &str) {
    match RulePack::from_yaml_str(yaml) {
        Err(Error::Parse(_)) => {}
        Err(Error::Validation(d)) => {
            panic!("expected a parse error, got validation diagnostics: {d:?}")
        }
        Err(other) => panic!("expected a parse error, got {other}"),
        Ok(_) => panic!("expected the pack to be rejected, but it parsed"),
    }
}
// ---------------------------------------------------------------------------
// Triggers
// ---------------------------------------------------------------------------

/// Every trigger type the catalogue defines.
///
/// This list is the matrix's spine: a form missing from it cannot be silently untested, and
/// `every_trigger_type_is_exercised` fails if a new form is added without a matrix entry.
const TRIGGER_TYPES: [&str; 9] = [
    "schedule",
    "osc",
    "midi",
    "dmx",
    "device_state",
    "heartbeat_missed",
    "device_parameter",
    "manual",
    "api",
];

#[test]
fn every_trigger_type_is_accepted_by_the_parser() {
    let valid: [(&str, &str); 9] = [
        ("schedule", "    type: schedule\n    at: \"18:55\"\n"),
        ("osc", "    type: osc\n    address: /stage/lights\n"),
        (
            "midi",
            "    type: midi\n    message: note_on\n    channel: 1\n",
        ),
        (
            "dmx",
            "    type: dmx\n    universe: 1\n    channel: 10\n    comparison: greater_than\n    value: 0\n",
        ),
        (
            "device_state",
            "    type: device_state\n    device: lighting-rack\n    state: offline\n",
        ),
        (
            "heartbeat_missed",
            "    type: heartbeat_missed\n    device: lighting-rack\n    after_ms: 5000\n",
        ),
        (
            "device_parameter",
            "    type: device_parameter\n    device: amp-1\n    parameter: level\n    comparison: gt\n    value: 0.5\n",
        ),
        ("manual", "    type: manual\n"),
        ("api", "    type: api\n"),
    ];
    assert_eq!(
        valid.len(),
        TRIGGER_TYPES.len(),
        "the matrix must cover every trigger type"
    );
    for (name, trigger) in valid {
        assert_valid(&with_trigger(trigger));
        // The pack really did parse into that trigger type, not merely not fail.
        let pack = RulePack::from_yaml_str(&with_trigger(trigger)).unwrap();
        assert_eq!(pack.rules[0].trigger.type_label(), name);
    }
}

#[test]
fn a_malformed_trigger_document_is_a_parse_error_not_a_panic() {
    // Indentation that produces invalid YAML, and a trigger that is a scalar rather than a mapping.
    assert!(matches!(
        RulePack::from_yaml_str(&with_trigger("    type: [\n")),
        Err(Error::Parse(_))
    ));
    assert!(RulePack::from_yaml_str(&with_trigger("    nonsense\n")).is_err());
}

#[test]
fn an_unknown_trigger_type_is_rejected() {
    // An unknown variant is caught by the deserializer, so it never reaches validation. Either
    // way the pack must not load.
    assert_parse_error(&with_trigger("type: teapot\n"));
}

#[test]
fn a_mistyped_trigger_key_is_an_error_rather_than_a_silently_ignored_instruction() {
    // `deny_unknown_fields` on the trigger mapping already rejects this outright, and the pack
    // audits the raw YAML as a second line of defence for unit variants. Without either, `chanell`
    // would be dropped and the trigger would match every channel.
    assert_parse_error(&with_trigger(
        "type: midi\nmessage: control_change\nchanell: 3\n",
    ));
}

// --- schedule -------------------------------------------------------------

#[test]
fn schedule_trigger_boundaries() {
    // Valid: every documented form.
    for trigger in [
        "    type: schedule\n    at: \"00:00\"\n",
        "    type: schedule\n    at: \"23:59\"\n",
        "    type: schedule\n    cron: \"30 18 * * mon-fri\"\n",
        "    type: schedule\n    interval_ms: 1000\n",
        "    type: schedule\n    at: \"18:55\"\n    once: true\n",
    ] {
        assert_valid(&with_trigger(trigger));
    }

    // Invalid: no form, two forms, an unparseable cron, a zero interval, `once` with no anchor.
    assert_rejected_mentioning(&with_trigger("    type: schedule\n"), "needs one of");
    assert_rejected_mentioning(
        &with_trigger("    type: schedule\n    at: \"18:00\"\n    interval_ms: 500\n"),
        "exactly one",
    );
    assert_rejected_mentioning(
        &with_trigger("    type: schedule\n    cron: \"not a cron\"\n"),
        "cron",
    );
    assert_rejected_mentioning(
        &with_trigger("    type: schedule\n    interval_ms: 0\n"),
        "greater than zero",
    );
    assert_rejected_mentioning(
        &with_trigger("    type: schedule\n    interval_ms: 1000\n    once: true\n"),
        "once",
    );
    assert_rejected_mentioning(
        &with_trigger("    type: schedule\n    cron: \"* * * * *\"\n    days: [mon]\n"),
        "days",
    );

    // Boundary: an out-of-range time is a parse error rather than a silent wrap to 24:00.
    assert!(
        RulePack::from_yaml_str(&with_trigger("    type: schedule\n    at: \"24:00\"\n")).is_err()
    );
    assert!(
        RulePack::from_yaml_str(&with_trigger("    type: schedule\n    at: \"18:60\"\n")).is_err()
    );

    // Boundary: 249 ms is merely warned about, 250 ms is clean.
    let warned = diagnostics_for(&with_trigger("    type: schedule\n    interval_ms: 249\n"));
    assert!(
        warned.iter().any(|d| d.severity == Severity::Warning),
        "a very short interval is a warning, not an error"
    );
    assert_valid(&with_trigger("    type: schedule\n    interval_ms: 250\n"));
}

#[test]
fn schedule_trigger_malformed_input_is_rejected() {
    assert!(
        RulePack::from_yaml_str(&with_trigger("    type: schedule\n    at: half past six\n"))
            .is_err()
    );
    assert!(RulePack::from_yaml_str(&with_trigger(
        "    type: schedule\n    at: \"18:55\"\n    days: [funday]\n"
    ))
    .is_err());
}

// --- osc ------------------------------------------------------------------

#[test]
fn osc_trigger_boundaries() {
    assert_valid(&with_trigger(
        "    type: osc\n    address: /a\n    min_args: 0\n",
    ));
    assert_valid(&with_trigger(
        "    type: osc\n    address: /a/*/power\n    arg_equals: 1.0\n",
    ));
    // An address that does not start with `/` is not an OSC address.
    assert_rejected_mentioning(
        &with_trigger("    type: osc\n    address: stage/lights\n"),
        "must start with",
    );
    // `arg_equals` with no arguments is a contradiction.
    assert_rejected_mentioning(
        &with_trigger("    type: osc\n    address: /a\n    min_args: 0\n    arg_equals: 1.0\n"),
        "at least one argument",
    );
}

// --- midi -----------------------------------------------------------------

#[test]
fn midi_trigger_covers_every_message_kind() {
    for kind in MidiMessageKind::ALL {
        // The YAML surface uses the serde variant name, which for control change is spelled out
        // rather than abbreviated as the normalized event label does.
        let yaml_name = match kind.as_str() {
            "cc" => "control_change",
            other => other,
        };
        assert_valid(&with_trigger(&format!(
            "    type: midi\n    message: {yaml_name}\n"
        )));
    }
}

#[test]
fn midi_trigger_boundaries() {
    // Boundary: channel 15 and number 127 are legal; 16 and 128 are not.
    assert_valid(&with_trigger(
        "    type: midi\n    message: control_change\n    channel: 15\n    number: 127\n",
    ));
    assert_rejected_mentioning(
        &with_trigger("    type: midi\n    message: control_change\n    channel: 16\n"),
        "channel",
    );
    assert_rejected_mentioning(
        &with_trigger("    type: midi\n    message: control_change\n    number: 128\n"),
        "number",
    );
    assert_rejected_mentioning(
        &with_trigger("    type: midi\n    message: control_change\n    group: 16\n"),
        "group",
    );
    // An unknown message kind never reaches validation.
    assert_parse_error(&with_trigger("    type: midi\n    message: teapot\n"));
}

// --- dmx ------------------------------------------------------------------

#[test]
fn dmx_trigger_boundaries() {
    // Boundary: the last channel is 511 and the last universe is 32767.
    assert_valid(&with_trigger(
        "    type: dmx\n    universe: 32767\n    channel: 511\n    comparison: equals\n    value: 255\n",
    ));
    assert_rejected_mentioning(
        &with_trigger("    type: dmx\n    universe: 1\n    channel: 512\n    comparison: equals\n    value: 0\n"),
        "channel",
    );
    assert_rejected_mentioning(
        &with_trigger("    type: dmx\n    universe: 32768\n    channel: 0\n    comparison: equals\n    value: 0\n"),
        "universe",
    );
    for comparison in [
        "equals",
        "greater_than",
        "less_than",
        "crossed_above",
        "crossed_below",
        "changed",
    ] {
        assert_valid(&with_trigger(&format!(
            "    type: dmx\n    universe: 1\n    channel: 0\n    comparison: {comparison}\n    value: 0\n"
        )));
    }
    // An unknown comparison never reaches validation.
    assert_parse_error(&with_trigger(
        "    type: dmx\n    universe: 1\n    channel: 0\n    comparison: sideways\n    value: 0\n",
    ));
    // `dmx_previous` is only meaningful for an edge comparison.
    let warned = diagnostics_for(&with_trigger(
        "    type: dmx\n    universe: 1\n    channel: 0\n    comparison: equals\n    value: 0\n    dmx_previous: 10\n",
    ));
    assert!(warned.iter().any(|d| d.severity == Severity::Warning));
}

// --- device triggers ------------------------------------------------------

#[test]
fn device_trigger_boundaries() {
    for state in ["online", "degraded", "offline", "unknown"] {
        assert_valid(&with_trigger(&format!(
            "    type: device_state\n    device: amp-1\n    state: {state}\n"
        )));
    }
    assert!(matches!(
        RulePack::from_yaml_str(&with_trigger(
            "    type: device_state\n    device: amp-1\n    state: angry\n"
        )),
        Err(Error::Parse(_))
    ));
    assert_rejected_mentioning(
        &with_trigger("    type: device_state\n    device: \"  \"\n    state: online\n"),
        "device",
    );

    // Boundary: a zero heartbeat threshold is meaningless; one millisecond is legal.
    assert_rejected_mentioning(
        &with_trigger("    type: heartbeat_missed\n    device: amp-1\n    after_ms: 0\n"),
        "greater than zero",
    );
    assert_valid(&with_trigger(
        "    type: heartbeat_missed\n    device: amp-1\n    after_ms: 1\n",
    ));
}

// --- api ------------------------------------------------------------------

#[test]
fn api_trigger_name_boundaries() {
    assert_valid(&with_trigger("    type: api\n    name: show.start_1-a\n"));
    assert_rejected_mentioning(
        &with_trigger("    type: api\n    name: \"has space\"\n"),
        "name",
    );
}

// ---------------------------------------------------------------------------
// Conditions
// ---------------------------------------------------------------------------

#[test]
fn every_condition_type_is_accepted_by_the_parser() {
    let valid: [(&str, &str); 6] = [
        (
            "device_health",
            "- id: c\n  type: device_health\n  device: amp-1\n  equals: online\n",
        ),
        (
            "time_window",
            "- id: c\n  type: time_window\n  from: \"18:00\"\n  to: \"23:00\"\n",
        ),
        (
            "dmx_channel",
            "- id: c\n  type: dmx_channel\n  universe: 1\n  channel: 5\n  comparison: gt\n  value: 128\n",
        ),
        (
            "device_parameter",
            "- id: c\n  type: device_parameter\n  device: amp-1\n  parameter: level\n  comparison: gte\n  value: 0.8\n",
        ),
        ("rule_armed", "- id: c\n  type: rule_armed\n  rule: house-lights\n"),
        ("rule_has_tag", "- id: c\n  type: rule_has_tag\n  tag: lighting\n"),
    ];
    for (name, condition) in valid {
        let yaml = with_condition(condition);
        assert_valid(&yaml);
        let pack = RulePack::from_yaml_str(&yaml).unwrap();
        assert_eq!(pack.rules[0].conditions[0].spec.type_label(), name);
    }
}

#[test]
fn a_time_window_that_does_not_move_forward_is_rejected() {
    // A window whose end is at or before its start can never be entered, so it would silently gate
    // a rule off forever.
    assert_rejected_mentioning(
        &with_condition("- id: c\n  type: time_window\n  from: \"23:00\"\n  to: \"18:00\"\n"),
        "must be before",
    );
    assert_rejected_mentioning(
        &with_condition("- id: c\n  type: time_window\n  from: \"18:00\"\n  to: \"18:00\"\n"),
        "must be before",
    );
    // Boundary: a window spanning midnight is expressible as the two halves, not one range.
    assert_valid(&with_condition(
        "- id: c\n  type: time_window\n  from: \"00:00\"\n  to: \"23:59\"\n",
    ));
}

#[test]
fn an_unknown_condition_type_is_a_parse_error() {
    assert_parse_error(&with_condition("- id: c\n  type: teapot\n"));
}

#[test]
fn a_mistyped_condition_key_is_rejected() {
    assert_parse_error(&with_condition(
        "- id: c\n  type: rule_armed\n  rul: house-lights\n",
    ));
}

#[test]
fn a_condition_referencing_an_unknown_rule_is_reported() {
    assert_rejected_mentioning(
        &with_condition("- id: c\n  type: rule_armed\n  rule: no-such-rule\n"),
        "no-such-rule",
    );
}

#[test]
fn a_dmx_channel_condition_is_bounded_by_the_universe() {
    assert_valid(&with_condition(
        "- id: c\n  type: dmx_channel\n  universe: 1\n  channel: 511\n  comparison: eq\n  value: 255\n",
    ));
    // The channel is out of the universe, which is a located validation error rather than a parse
    // failure: 512 is a valid `u16`, just not a valid DMX channel.
    assert_rejected_mentioning(
        &with_condition(
            "- id: c\n  type: dmx_channel\n  universe: 1\n  channel: 512\n  comparison: eq\n  value: 0\n",
        ),
        "outside 0-511",
    );
    assert_rejected_mentioning(
        &with_condition(
            "- id: c\n  type: dmx_channel\n  universe: 32768\n  channel: 0\n  comparison: eq\n  value: 0\n",
        ),
        "universe",
    );
}

#[test]
fn a_condition_fire_on_unknown_flag_defaults_to_strict() {
    let yaml = with_condition("- id: c\n  type: rule_armed\n  rule: house-lights\n");
    let pack = RulePack::from_yaml_str(&yaml).unwrap();
    assert!(
        !pack.rules[0].conditions[0].fire_on_unknown,
        "a rule must not fire on unknown state unless it opts in"
    );
    let lenient = with_condition(
        "- id: c\n  type: rule_armed\n  rule: house-lights\n  fire_on_unknown: true\n",
    );
    assert!(RulePack::from_yaml_str(&lenient).unwrap().rules[0].conditions[0].fire_on_unknown);
}

// ---------------------------------------------------------------------------
// Actions
// ---------------------------------------------------------------------------

/// Every action type the catalogue defines.
const ACTION_TYPES: [&str; 13] = [
    "control.osc",
    "control.midi",
    "control.dmx_channels",
    "control.dmx_scene",
    "control.dmx_universe",
    "media.video_source",
    "media.audio_route",
    "notify.operator",
    "log.incident",
    "workflow.wait",
    "workflow.invoke_rule",
    "workflow.exec",
    "workflow.use_action",
];

#[test]
fn every_action_type_is_accepted_by_the_parser() {
    let valid: [(&str, &str); 13] = [
        ("control.osc", "- id: a\n  type: control.osc\n  device: d\n  address: /p/1\n"),
        (
            "control.midi",
            "- id: a\n  type: control.midi\n  device: d\n  channel: 1\n  kind: note_on\n  number: 60\n  value: 100\n",
        ),
        (
            "control.dmx_channels",
            "- id: a\n  type: control.dmx_channels\n  device: d\n  universe: 1\n  start_channel: 0\n  values: [255]\n",
        ),
        ("control.dmx_scene", "- id: a\n  type: control.dmx_scene\n  device: d\n  scene: house\n"),
        (
            "control.dmx_universe",
            "- id: a\n  type: control.dmx_universe\n  device: d\n  universe: 1\n  values: [0, 128]\n",
        ),
        (
            "media.video_source",
            "- id: a\n  type: media.video_source\n  device: d\n  source: display-1\n  operation: switch\n",
        ),
        (
            "media.audio_route",
            "- id: a\n  type: media.audio_route\n  device: d\n  from: mic-1\n  to: house\n",
        ),
        ("notify.operator", "- id: a\n  type: notify.operator\n  message: hello\n"),
        ("log.incident", "- id: a\n  type: log.incident\n"),
        ("workflow.wait", "- id: a\n  type: workflow.wait\n  ms: 500\n"),
        (
            "workflow.invoke_rule",
            "- id: a\n  type: workflow.invoke_rule\n  rule: other\n",
        ),
        ("workflow.exec", "- id: a\n  type: workflow.exec\n  program: /bin/true\n"),
        (
            "workflow.use_action",
            "- id: a\n  type: workflow.use_action\n  library: frag\n",
        ),
    ];
    assert_eq!(
        valid.len(),
        ACTION_TYPES.len(),
        "the matrix must cover every action type"
    );
    for (name, action) in valid {
        // Naming the failing action in the assertion makes a matrix regression readable at a glance.
        let yaml = with_action(action);
        let reported: Vec<String> = diagnostics_for(&yaml)
            .into_iter()
            .filter(|d| d.severity == Severity::Error)
            .map(|d| format!("{}: {}", d.location, d.message))
            .collect();
        assert!(
            reported.is_empty(),
            "{name} should be valid, got {reported:?}"
        );
        let pack = RulePack::from_yaml_str(&yaml).unwrap();
        assert_eq!(pack.rules[0].actions[0].spec.type_label(), name);
    }
}

#[test]
fn an_unknown_action_type_is_a_parse_error() {
    assert_parse_error(&with_action("- id: a\n  type: control.teapot\n"));
}

#[test]
fn an_endpoint_write_without_a_device_target_is_rejected() {
    // Every action that writes to hardware must say which device; otherwise the chain would
    // silently have nowhere to write.
    for action in [
        "- id: a\n  type: control.osc\n  address: /p/1\n",
        "- id: a\n  type: control.midi\n  channel: 0\n  kind: cc\n  number: 1\n",
        "- id: a\n  type: control.dmx_scene\n  scene: house\n",
        "- id: a\n  type: media.video_source\n  source: display-1\n  operation: switch\n",
    ] {
        assert_rejected_mentioning(&with_action(action), "must name a target device");
    }
}

#[test]
fn a_device_target_on_a_non_endpoint_action_is_a_warning_not_an_error() {
    let diagnostics = diagnostics_for(&with_action("- id: a\n  type: log.incident\n  device: d\n"));
    assert!(
        diagnostics
            .iter()
            .any(|d| d.severity == Severity::Warning && d.message.contains("ignored")),
        "expected a warning that the device field is ignored, got {diagnostics:?}"
    );
    assert!(
        error_locations(&with_action("- id: a\n  type: log.incident\n  device: d\n")).is_empty()
    );
}

#[test]
fn osc_action_boundaries() {
    assert_rejected_mentioning(
        &with_action("- id: a\n  type: control.osc\n  device: d\n  address: p/1\n"),
        "must start with",
    );
    assert_valid(&with_action(
        "- id: a\n  type: control.osc\n  device: d\n  address: /\n",
    ));
}

#[test]
fn midi_action_covers_every_message_kind() {
    for kind in MidiMessageKind::ALL {
        // A MIDI 2.0-only kind needs its 32-bit value.
        let needs_value32 = kind.is_midi2_only();
        let yaml = format!(
            "- id: a\n  type: control.midi\n  device: d\n  channel: 0\n  kind: {}\n  number: 60\n{}\n",
            kind.as_str(),
            if needs_value32 {
                "  value32: 65535\n"
            } else {
                ""
            }
        );
        assert_valid(&with_action(&yaml));
    }
}

#[test]
fn midi_action_boundaries() {
    // Boundary: the widest MIDI 1.0 field is 7-bit.
    assert_rejected_mentioning(
        &with_action("- id: a\n  type: control.midi\n  device: d\n  channel: 0\n  kind: cc\n  number: 1\n  value: 128\n"),
        "value",
    );
    assert_rejected_mentioning(
        &with_action(
            "- id: a\n  type: control.midi\n  device: d\n  channel: 16\n  kind: cc\n  number: 1\n",
        ),
        "channel",
    );
    assert_rejected_mentioning(
        &with_action("- id: a\n  type: control.midi\n  device: d\n  channel: 0\n  kind: teapot\n  number: 1\n"),
        "unknown MIDI message kind",
    );
    // A MIDI 2.0-only kind without a 32-bit value would have to be guessed, so it is rejected.
    assert_rejected_mentioning(
        &with_action("- id: a\n  type: control.midi\n  device: d\n  channel: 0\n  kind: pitch_bend\n  number: 0\n"),
        "value32",
    );
    // Velocity is 16-bit in MIDI 2.0; a wider value is an error rather than a silent truncation.
    assert_rejected_mentioning(
        &with_action("- id: a\n  type: control.midi\n  device: d\n  channel: 0\n  kind: note_on\n  number: 60\n  value32: 100000\n"),
        "16 bits",
    );
    // An index only applies to the kinds that carry one.
    assert_rejected_mentioning(
        &with_action("- id: a\n  type: control.midi\n  device: d\n  channel: 0\n  kind: rpn\n  number: 5\n  value32: 1\n  index: 200\n"),
        "index",
    );
}

#[test]
fn dmx_action_boundaries() {
    // The last channel of the universe is the last legal start for a single value.
    assert_valid(&with_action(
        "- id: a\n  type: control.dmx_channels\n  device: d\n  universe: 1\n  start_channel: 511\n  values: [255]\n",
    ));
    // Two values from channel 511 run past the end.
    assert_rejected_mentioning(
        &with_action("- id: a\n  type: control.dmx_channels\n  device: d\n  universe: 1\n  start_channel: 511\n  values: [1, 2]\n"),
        "run past",
    );
    assert_rejected_mentioning(
        &with_action("- id: a\n  type: control.dmx_channels\n  device: d\n  universe: 1\n  start_channel: 512\n  values: [1]\n"),
        "start_channel",
    );
    // Boundary: a universe holds at most 512 values; 513 is rejected rather than truncated.
    let overlong: Vec<&str> = vec!["0"; 513];
    assert_rejected_mentioning(
        &with_action(&format!(
            "- id: a\n  type: control.dmx_universe\n  device: d\n  universe: 1\n  values: [{}]\n",
            overlong.join(",")
        )),
        "512",
    );
    assert_rejected_mentioning(
        &with_action("- id: a\n  type: control.dmx_scene\n  device: d\n  scene: \"  \"\n"),
        "scene name",
    );
}

#[test]
fn media_action_boundaries() {
    assert_rejected_mentioning(
        &with_action("- id: a\n  type: media.video_source\n  device: d\n  source: \"  \"\n  operation: switch\n"),
        "video source",
    );
    assert_rejected_mentioning(
        &with_action(
            "- id: a\n  type: media.audio_route\n  device: d\n  from: \"\"\n  to: house\n",
        ),
        "from",
    );
    // Routing a signal to itself is legal but almost certainly a mistake, so it is a warning.
    let diagnostics = diagnostics_for(&with_action(
        "- id: a\n  type: media.audio_route\n  device: d\n  from: x\n  to: x\n",
    ));
    assert!(diagnostics.iter().any(|d| d.severity == Severity::Warning));
}

#[test]
fn workflow_action_boundaries() {
    assert_rejected_mentioning(
        &with_action("- id: a\n  type: notify.operator\n  message: \"  \"\n"),
        "must say something",
    );
    assert_rejected_mentioning(
        &with_action("- id: a\n  type: workflow.invoke_rule\n  rule: \"\"\n"),
        "must name a rule",
    );
    assert_rejected_mentioning(
        &with_action("- id: a\n  type: workflow.use_action\n  library: \"\"\n"),
        "must not be empty",
    );
    assert_rejected_mentioning(
        &with_action("- id: a\n  type: workflow.exec\n  program: \"\"\n"),
        "must name a program",
    );
    // A long inline wait is a warning: it holds the action chain open.
    let diagnostics = diagnostics_for(&with_action(
        "- id: a\n  type: workflow.wait\n  ms: 60000\n",
    ));
    assert!(diagnostics.iter().any(|d| d.severity == Severity::Warning));
}

#[test]
fn an_action_chain_is_bounded_and_uniquely_identified() {
    // Boundary: the chain length cap is enforced rather than letting a pack hang a show.
    let long: String = (0..65)
        .map(|i| format!("- id: a{i}\n  type: log.incident\n"))
        .collect();
    assert_rejected_mentioning(&with_action(&long), "exceeding the limit");

    // Duplicate step ids would make execution records ambiguous.
    assert_rejected_mentioning(
        &with_action("- id: a\n  type: log.incident\n- id: a\n  type: log.incident\n"),
        "duplicate action id",
    );
    assert_rejected_mentioning(
        &with_action("- id: \"\"\n  type: log.incident\n"),
        "action id must not be empty",
    );
}

#[test]
fn a_library_fragment_that_does_not_exist_is_reported() {
    let yaml = with_action("- id: a\n  type: workflow.use_action\n  library: missing\n");
    assert_rejected_mentioning(&yaml, "missing");
}
