//! Integration: the operator's whole lifecycle against the shipped binary (spec §13, §19, §24).
//!
//! A rule pack has to survive the whole path an operator walks: validate it, dry-run it, read back
//! what happened, and get a clear, stable exit code when something is wrong. Everything here runs
//! the real binary in a real process against real files.

use tpt_app_av_automation_scenarios::{
    cli, code, stderr, stdout, wait_until, TempDir, EXIT_CONFIG, EXIT_OK, EXIT_SKIPPED,
    PATIENCE,
};

const PACK: &str = r#"
format_version: 1
name: Lifecycle
revision: 1
rules:
  - id: go
    name: Go
    armed: true
    trigger: { type: osc, address: /go }
    actions:
      - { id: power, type: control.osc, device: proj, address: /power, args: [1] }
      - { id: note, type: notify.operator, message: show started }
"#;

const DEVICES: &str = "devices:\n  - { id: proj, protocol: virtual }\n";

#[test]
fn a_valid_pack_validates_and_simulates() {
    let dir = TempDir::new("lifecycle-happy");
    let rules = dir.write("pack.yaml", PACK);
    let devices = dir.write("devices.yaml", DEVICES);

    let out = cli(&[
        "validate",
        "--rules",
        rules.to_str().unwrap(),
        "--devices",
        devices.to_str().unwrap(),
    ]);
    assert_eq!(code(&out), EXIT_OK, "validate failed: {}", stderr(&out));
    assert!(
        stdout(&out).contains("Lifecycle"),
        "validate should report the pack name: {}",
        stdout(&out)
    );

    let out = cli(&[
        "simulate",
        "--rules",
        rules.to_str().unwrap(),
        "--devices",
        devices.to_str().unwrap(),
        "--event",
        "osc:/go",
    ]);
    assert_eq!(code(&out), EXIT_OK, "simulate failed: {}", stderr(&out));
    let text = stdout(&out);
    assert!(
        text.contains("power"),
        "simulate should describe the action: {text}"
    );
    assert!(
        text.to_lowercase().contains("simulat"),
        "simulation mode must be unmistakable in the output: {text}"
    );
}

#[test]
fn a_broken_pack_is_rejected_with_a_configuration_exit_code() {
    let dir = TempDir::new("lifecycle-broken");
    // The trigger key is misspelled, which the strict key allowlist must reject.
    let rules = dir.write(
        "pack.yaml",
        r#"
format_version: 1
name: Broken
revision: 1
rules:
  - id: go
    name: Go
    armed: true
    trigger: { type: osc, adress: /go }
    actions:
      - { id: power, type: control.osc, device: ghost, address: /power, args: [1] }
"#,
    );

    let out = cli(&["validate", "--rules", rules.to_str().unwrap()]);
    assert_eq!(
        code(&out),
        EXIT_CONFIG,
        "a broken pack must exit 4, not 0: {}",
        stderr(&out)
    );
    let text = format!("{}{}", stdout(&out), stderr(&out));
    assert!(
        !text.trim().is_empty(),
        "the operator must be told what is wrong, not just shown a code"
    );
}

#[test]
fn validate_catches_an_unknown_device_protocol_and_a_missing_file() {
    let dir = TempDir::new("lifecycle-devices");
    let rules = dir.write("pack.yaml", PACK);
    let devices = dir.write(
        "devices.yaml",
        "devices:\n  - { id: proj, protocol: quantum-flux }\n",
    );

    let out = cli(&[
        "validate",
        "--rules",
        rules.to_str().unwrap(),
        "--devices",
        devices.to_str().unwrap(),
    ]);
    assert_eq!(code(&out), EXIT_CONFIG, "{}", stderr(&out));
    assert!(
        format!("{}{}", stdout(&out), stderr(&out)).contains("quantum-flux"),
        "the offending protocol should be named"
    );

    let missing = dir.path().join("nope.yaml");
    let out = cli(&[
        "validate",
        "--rules",
        rules.to_str().unwrap(),
        "--devices",
        missing.to_str().unwrap(),
    ]);
    assert_ne!(
        code(&out),
        EXIT_OK,
        "a missing device file is not a successful validation"
    );
}

#[test]
fn every_shipped_example_pack_still_validates() {
    // Guards against the documentation drifting away from the parser: if a change breaks one of
    // the packs under rules/, this fails rather than the first operator who tries one.
    let workspace = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root")
        .join("rules");
    let mut checked = 0;
    let mut stack = vec![workspace];
    while let Some(dir) = stack.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "yaml")
                && path.file_name().is_some_and(|n| n != "devices.yaml")
            {
                let out = cli(&["validate", "--rules", path.to_str().unwrap()]);
                assert_eq!(
                    code(&out),
                    EXIT_OK,
                    "{} no longer validates: {}{}",
                    path.display(),
                    stdout(&out),
                    stderr(&out)
                );
                checked += 1;
            }
        }
    }
    assert!(checked > 0, "no example rule packs were found to check");
}

#[test]
fn history_is_graceful_when_the_database_is_empty() {
    let dir = TempDir::new("lifecycle-history-empty");
    let state = dir.path().join("state");
    std::fs::create_dir_all(&state).unwrap();

    // No database has ever been written, so this is a configuration problem, reported as one (4)
    // with an explanation rather than a panic.
    let out = cli(&["history", "--state-dir", state.to_str().unwrap()]);
    assert_eq!(code(&out), EXIT_CONFIG, "{}{}", stdout(&out), stderr(&out));
    assert!(
        stderr(&out).contains("no state database"),
        "the operator should be told what is missing: {}",
        stderr(&out)
    );
}

#[test]
fn history_of_an_existing_but_empty_database_is_not_an_error() {
    let dir = TempDir::new("lifecycle-history-real");
    let rules = dir.write("pack.yaml", PACK);
    let devices = dir.write("devices.yaml", DEVICES);
    let state = dir.path().join("state");

    // Run the engine briefly so the database is created, then ask it what it recorded.
    let mut engine = tpt_app_av_automation_scenarios::Engine::start(&[
        "run",
        "--service",
        "--rules",
        rules.to_str().unwrap(),
        "--devices",
        devices.to_str().unwrap(),
        "--state-dir",
        state.to_str().unwrap(),
    ]);
    wait_until("the state database to appear", PATIENCE, || {
        state.join("automation.db").exists()
    });
    engine.kill();

    let out = cli(&["history", "--state-dir", state.to_str().unwrap()]);
    // "no executions recorded" is a skipped report (3), not a failure — the database opened fine.
    assert_eq!(
        code(&out),
        EXIT_SKIPPED,
        "an empty but real database should report cleanly: {}{}",
        stdout(&out),
        stderr(&out)
    );
    assert!(
        format!("{}{}", stdout(&out), stderr(&out)).contains("no executions"),
        "the operator should be told the history is empty: {}{}",
        stdout(&out),
        stderr(&out)
    );
}

#[test]
fn the_help_text_documents_every_command() {
    let out = cli(&["--help"]);
    assert_eq!(code(&out), EXIT_OK);
    let text = stdout(&out);
    for command in ["validate", "simulate", "run", "watchdog", "history"] {
        assert!(
            text.contains(command),
            "`{command}` should appear in --help: {text}"
        );
    }
}

#[test]
fn bad_invocations_are_refused_rather_than_silently_succeeding() {
    assert_ne!(code(&cli(&["definitely-not-a-command"])), EXIT_OK);
    assert_ne!(code(&cli(&["run"])), EXIT_OK, "run requires --rules");
    assert_ne!(code(&cli(&["validate"])), EXIT_OK, "validate requires --rules");
}

#[test]
fn simulation_of_a_disarmed_rule_still_explains_itself() {
    let dir = TempDir::new("lifecycle-disarmed");
    let rules = dir.write("pack.yaml", &PACK.replace("armed: true", "armed: false"));
    let devices = dir.write("devices.yaml", DEVICES);
    let out = cli(&[
        "simulate",
        "--rules",
        rules.to_str().unwrap(),
        "--devices",
        devices.to_str().unwrap(),
        "--event",
        "osc:/go",
    ]);
    assert_eq!(code(&out), EXIT_OK, "{}", stderr(&out));
    assert!(!stdout(&out).trim().is_empty());
}

#[test]
fn validation_is_fast_enough_to_run_in_a_pre_show_check() {
    // Not a benchmark: a guard that validation does not accidentally open sockets or sleep.
    let dir = TempDir::new("lifecycle-speed");
    let rules = dir.write("pack.yaml", PACK);
    let start = std::time::Instant::now();
    let out = cli(&["validate", "--rules", rules.to_str().unwrap()]);
    assert_eq!(code(&out), EXIT_OK);
    assert!(
        start.elapsed() < std::time::Duration::from_secs(10),
        "validation took {:?}, far too long for a pre-show check",
        start.elapsed()
    );
}

