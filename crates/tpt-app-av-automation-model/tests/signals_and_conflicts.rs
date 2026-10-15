//! Matching semantics for producer-reported signal events and the `file_arrived` / `job` triggers
//! (spec §7.4, §7.5).

use tpt_app_av_automation_core::{Event, JobOutcome, SignalKind};
use tpt_app_av_automation_model::{Comparison, RulePack, Trigger};

fn trigger_from(yaml: &str) -> Trigger {
    let pack = RulePack::from_yaml_str(&format!(
        "format_version: 1\nname: T\nrevision: 1\nrules:\n  - id: r\n    name: R\n    trigger:\n{}\n    actions:\n      - {{ id: a, type: log.incident }}\n",
        yaml.lines()
            .map(|l| format!("      {l}"))
            .collect::<Vec<_>>()
            .join("\n")
    ))
    .expect("pack should parse");
    pack.rules[0].trigger.clone()
}

fn level(source: &str, value: f64) -> Event {
    Event::Signal {
        source: source.into(),
        channel: Some("main-L".into()),
        signal: SignalKind::Level,
        value: Some(value),
    }
}

#[test]
fn a_level_threshold_fires_only_when_the_comparison_holds() {
    let t =
        trigger_from("type: signal\nsource: audio-desk\nsignal: level\ncomparison: lt\nvalue: -20");
    assert!(t.matches(&level("audio-desk", -25.0)));
    assert!(!t.matches(&level("audio-desk", -10.0)));
    // A different producer never matches.
    assert!(!t.matches(&level("playout", -25.0)));
    // A yes/no observation is not a level.
    assert!(!t.matches(&Event::Signal {
        source: "audio-desk".into(),
        channel: None,
        signal: SignalKind::Silence,
        value: None,
    }));
}

#[test]
fn a_yes_no_signal_matches_on_kind_and_optional_source() {
    let t = trigger_from("type: signal\nsignal: black");
    let black = |source: &str| Event::Signal {
        source: source.into(),
        channel: None,
        signal: SignalKind::Black,
        value: None,
    };
    assert!(t.matches(&black("playout")));
    assert!(t.matches(&black("anything")));
    assert!(!t.matches(&Event::Signal {
        source: "playout".into(),
        channel: None,
        signal: SignalKind::Freeze,
        value: None,
    }));
}

#[test]
fn a_channel_filter_requires_that_channel() {
    let t = trigger_from("type: signal\nchannel: main-R\nsignal: clipping");
    let clip = |channel: Option<&str>| Event::Signal {
        source: "audio-desk".into(),
        channel: channel.map(str::to_owned),
        signal: SignalKind::Clipping,
        value: None,
    };
    assert!(t.matches(&clip(Some("main-R"))));
    assert!(!t.matches(&clip(Some("main-L"))));
    assert!(!t.matches(&clip(None)));
}

#[test]
fn file_arrived_matches_its_folder_and_extension_case_insensitively() {
    let t = trigger_from("type: file_arrived\nfolder: /in\nextensions: [mov]");
    let arrived = |folder: &str, path: &str| Event::FileArrived {
        folder: folder.into(),
        path: path.into(),
    };
    assert!(t.matches(&arrived("/in", "/in/CLIP.MOV")));
    assert!(!t.matches(&arrived("/in", "/in/notes.txt")));
    assert!(!t.matches(&arrived("/elsewhere", "/elsewhere/clip.mov")));
}

#[test]
fn job_matches_source_outcome_and_kind() {
    let t = trigger_from("type: job\nsource: qc\noutcome: failed");
    let job = |source: &str, outcome: JobOutcome| Event::Job {
        source: source.into(),
        job_id: "j".into(),
        outcome,
        job_kind: None,
    };
    assert!(t.matches(&job("qc", JobOutcome::Failed)));
    assert!(!t.matches(&job("qc", JobOutcome::Completed)));
    assert!(!t.matches(&job("forensics", JobOutcome::Failed)));
}

#[test]
fn comparison_is_the_model_type() {
    // Keeps the public re-export honest: thresholds use the same comparison as conditions.
    assert!(Comparison::Lt.evaluate_f64(1.0, 2.0));
}
