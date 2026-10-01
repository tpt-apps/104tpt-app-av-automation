//! Golden rule-pack regression tests (spec §18.3, §18.6).
//!
//! Each fixture replays a recorded event sequence against virtual devices and compares the complete
//! execution trace with `expected.json`. The semantic tests below pin the *meaning* of the important
//! traces, so a golden file cannot drift into encoding a bug unnoticed.

use std::collections::BTreeSet;
use std::path::PathBuf;

use tpt_app_av_automation_model::{ActionOutcome, ExecutionStatus};
use tpt_app_av_automation_test::{assert_golden, discover, fixtures_root, Replay};

fn fixture(name: &str) -> PathBuf {
    fixtures_root().join("golden").join(name)
}

fn replay(name: &str) -> Replay {
    Replay::run(&fixture(name)).unwrap_or_else(|e| panic!("{name}: {e}"))
}

#[test]
fn every_golden_fixture_matches_its_recorded_trace() {
    let dirs = discover("golden");
    assert!(dirs.len() >= 10, "golden fixtures are missing: found {}", dirs.len());
    for dir in dirs {
        assert_golden(&dir);
    }
}

#[test]
fn every_regression_fixture_still_passes() {
    // Permanent fixtures added for production bugs (CONTRIBUTING.md). Empty until the first one.
    for dir in discover("regression") {
        assert_golden(&dir);
    }
}

#[test]
fn golden_fixtures_exercise_every_mvp_trigger_and_action() {
    let mut triggers = BTreeSet::new();
    let mut actions = BTreeSet::new();
    for dir in discover("golden") {
        let r = Replay::run(&dir).unwrap();
        for record in &r.records {
            triggers.insert(record.trigger_type.clone());
            for step in &record.action_results {
                if step.outcome.is_success() && !step.simulated {
                    actions.insert(step.action_type.clone());
                }
            }
        }
    }
    for trigger in [
        "schedule",
        "osc",
        "midi",
        "dmx",
        "device_state",
        "heartbeat_missed",
        "device_parameter",
        "manual",
        "api",
    ] {
        assert!(triggers.contains(trigger), "no golden fixture fires a `{trigger}` trigger");
    }
    for action in [
        "control.osc",
        "control.midi",
        "control.dmx_channels",
        "control.dmx_universe",
        "control.dmx_scene",
        "media.video_source",
        "media.audio_route",
        "notify.operator",
        "log.incident",
        "workflow.wait",
        "workflow.invoke_rule",
    ] {
        assert!(actions.contains(action), "no golden fixture successfully executes `{action}` live");
    }
}

// --- the meaning of the traces ------------------------------------------------------------------

#[test]
fn live_event_start_fires_on_the_right_weekdays_once_and_honours_its_condition() {
    let r = replay("live-event-start");
    let statuses: Vec<_> = r.records.iter().map(|x| x.overall_status).collect();
    assert_eq!(
        statuses,
        [ExecutionStatus::Success, ExecutionStatus::Skipped],
        "Monday fires once, Saturday never, Tuesday is blocked by the offline projector"
    );
    assert_eq!(r.sent["lighting-rack"], ["dmx scene `house-to-half` fade 3000ms"]);
    assert_eq!(r.sent["projector-1"], ["osc /projector/1/power = 1"]);
    assert_eq!(r.sent["audio-matrix"], ["audio switch `playback-1` -> `main-pa`"]);
    assert!(r.records[1].action_results.is_empty(), "a blocked rule runs nothing");
}

#[test]
fn osc_cues_respect_arguments_and_single_segment_wildcards() {
    let r = replay("osc-cue");
    let fired: Vec<_> = r.records.iter().map(|x| x.rule_id.as_str()).collect();
    assert_eq!(fired, ["cue-1", "any-mute"], "only /cue/1=1 and /channel/7/mute with an argument");
}

#[test]
fn midi_triggers_match_channel_number_and_kind() {
    let r = replay("midi-note");
    let fired: Vec<_> = r.records.iter().map(|x| x.rule_id.as_str()).collect();
    assert_eq!(fired, ["go-button", "program"]);
    assert_eq!(r.sent["synth"].len(), 3);
    assert_eq!(r.sent["mixer"], ["osc /go = 1"]);
}

#[test]
fn dmx_edge_trigger_fires_only_on_crossings_and_only_when_the_condition_holds() {
    let r = replay("dmx-crossing");
    // 50 -> 150 crosses, but the master (ch 0) has not been seen yet: Unknown blocks it.
    // After the master is at 255: 20 -> 101 crosses again and fires.
    let statuses: Vec<_> = r.records.iter().map(|x| x.overall_status).collect();
    assert_eq!(statuses, [ExecutionStatus::Skipped, ExecutionStatus::Success]);
    assert!(r.records[0].blocked_by_unknown_condition());
    assert_eq!(r.sent["node"].len(), 2);
}

#[test]
fn a_failed_failover_step_is_a_partial_failure_and_escalates_then_recovers() {
    let r = replay("device-failover");
    assert_eq!(r.records.len(), 2);
    let first = &r.records[0];
    assert_eq!(first.overall_status, ExecutionStatus::PartialFailure);
    let ids: Vec<_> = first.action_results.iter().map(|a| a.action_id.as_str()).collect();
    assert_eq!(ids, ["switch-input", "tell-operator", "incident", "escalate"]);
    assert!(first.action_results[0].outcome.is_failure());
    assert!(first.action_results[1].outcome.is_skipped());
    assert!(first.action_results[3].outcome.is_success(), "the fallback ran");
    assert_eq!(r.records[1].overall_status, ExecutionStatus::Success, "healthy switcher: clean run");
    assert_eq!(r.sent["switcher"], ["osc /switcher/main/input = 2"]);
}

#[test]
fn heartbeat_loss_then_recovery_drives_the_rules_in_order() {
    let r = replay("heartbeat-missed");
    let fired: Vec<_> = r.records.iter().map(|x| x.rule_id.as_str()).collect();
    // The first heartbeat brings the device online (unknown -> online), which is itself a transition.
    assert_eq!(fired, ["back", "lost", "offline", "back"]);
}

#[test]
fn disarmed_rules_never_reach_a_device_but_armed_ones_do() {
    let r = replay("disarmed-simulation");
    let simulated: Vec<_> = r.records.iter().map(|x| x.simulated).collect();
    assert_eq!(simulated, [true, true, false, true]);
    assert_eq!(r.sent["projector-1"], ["osc /power = 1"], "only the one armed execution was delivered");
    assert!(r.records[1].overall_status == ExecutionStatus::Success, "an offline device is irrelevant to a simulation");
}

#[test]
fn manual_and_api_triggers_match_by_name() {
    let r = replay("manual-and-api");
    let fired: Vec<_> = r.records.iter().map(|x| x.rule_id.as_str()).collect();
    assert_eq!(fired, ["panic", "remote-start", "remote-start", "panic"]);
}

#[test]
fn conflicting_rules_resolve_by_priority_with_the_loser_recorded() {
    let r = replay("conflict-priority");
    let order: Vec<_> = r.records.iter().map(|x| x.rule_id.as_str()).collect();
    assert_eq!(order, ["aa-critical", "bb-normal", "zz-low"]);
    let low = &r.records[2];
    assert!(matches!(&low.action_results[0].outcome, ActionOutcome::Skipped { reason } if reason.contains("aa-critical")));
    assert_eq!(r.sent["proj"], ["osc /input = 2", "osc /input = 2", "osc /other = 5"]);
}

#[test]
fn workflow_steps_run_in_order_and_nest() {
    let r = replay("wait-invoke-library");
    let rules: Vec<_> = r.records.iter().map(|x| x.rule_id.as_str()).collect();
    assert_eq!(rules, ["announce", "sequence"], "the invoked rule completes first");
    assert!(r.records.iter().all(|x| x.overall_status == ExecutionStatus::Success));
    assert_eq!(r.sent["projector-1"], ["osc /projector/power = 1", "video switch `hdmi-2`"]);
}

#[test]
fn unknown_state_blocks_unless_opted_in_and_gates_compose() {
    let r = replay("condition-gating");
    let summary: Vec<_> = r
        .records
        .iter()
        .map(|x| (x.rule_id.as_str().to_owned(), x.overall_status))
        .collect();
    use ExecutionStatus::{Skipped, Success};
    // Rules matched by one event run in (priority, id) order: evening, lenient, strict.
    assert_eq!(
        summary,
        [
            // 10:00, projector health Unknown: evening is outside its window, lenient opted in to
            // unknown state and fires, strict is blocked by the unknown state.
            ("evening".into(), Skipped),
            ("lenient".into(), Success),
            ("strict".into(), Skipped),
            // temperature 80 while lamp_hours is still unknown: blocked; once lamp_hours is 1500 it
            // fires; 60 is below the threshold so nothing matches.
            ("hot".into(), Skipped),
            ("hot".into(), Success),
            // 19:30 and online: all three gates open.
            ("evening".into(), Success),
            ("lenient".into(), Success),
            ("strict".into(), Success),
            // Degraded: a *known* wrong state blocks even the lenient rule (opt-in covers Unknown only).
            ("evening".into(), Success),
            ("lenient".into(), Skipped),
            ("strict".into(), Skipped),
        ]
    );
}
