//! Round-trip and arm-safety properties that every shipped rule pack must hold (spec §9, §18.5).
//!
//! These are the same invariants the `fuzz/fuzz_targets/rule-pack.rs` coverage-guided target
//! asserts, checked deterministically against the real fixtures so a regression is caught by
//! `cargo test` on stable rather than only by a nightly fuzzing campaign.

use tpt_app_av_automation_model::RulePack;
use tpt_app_av_automation_test::{discover, fixtures_root};

/// Every shipped pack must survive a YAML round-trip unchanged.
///
/// The desktop rule builder reads YAML and writes it back, so a pack that does not survive
/// serialization would be silently rewritten into something different from what was authored.
#[test]
fn every_shipped_pack_round_trips_through_yaml() {
    let fixtures = discover("golden");
    assert!(
        !fixtures.is_empty(),
        "no golden fixtures were discovered under {}",
        fixtures_root().display()
    );

    for dir in fixtures {
        let path = dir.join("pack.yaml");
        let pack = RulePack::from_path(&path)
            .unwrap_or_else(|e| panic!("{} failed to parse: {e}", path.display()));

        let yaml = pack.to_yaml_string().expect("pack should serialize");
        let reparsed = RulePack::from_yaml_str(&yaml)
            .unwrap_or_else(|e| panic!("{} did not re-parse: {e}", path.display()));

        assert_eq!(
            pack,
            reparsed,
            "{} changed meaning on round-trip",
            path.display()
        );
    }
}

/// A pack must never arm a rule that its source did not declare as armed.
///
/// Rules load disarmed by default so a pack cannot fire on a machine it was not authored for.
/// Serializing and re-parsing must not promote a rule either.
#[test]
fn no_shipped_pack_arms_a_rule_it_did_not_declare() {
    for dir in discover("golden") {
        let path = dir.join("pack.yaml");
        let source = std::fs::read_to_string(&path).expect("pack should read");
        let pack = RulePack::from_yaml_str(&source)
            .unwrap_or_else(|e| panic!("{} failed to parse: {e}", path.display()));

        for rule in &pack.rules {
            if !source.contains("armed") {
                assert!(
                    !rule.armed,
                    "{} has rule `{}` armed without declaring it",
                    path.display(),
                    rule.id
                );
            }
        }
    }
}

/// The example packs shipped in `rules/` must obey the same rules as the test fixtures.
#[test]
fn every_example_pack_round_trips_through_yaml() {
    let root = fixtures_root();
    let rules_dir = root
        .parent()
        .and_then(|p| p.parent())
        .map(|p| p.join("rules"))
        .expect("rules directory should be resolvable from the fixtures root");

    let mut checked = 0;
    for group in ["examples", "live-event", "media-pipeline", "failover"] {
        let dir = rules_dir.join(group);
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            // A rule directory may also hold its `devices.yaml`; that is a device
            // registry, not a rule pack, and is validated by a separate suite.
            if path.extension().and_then(|e| e.to_str()) != Some("yaml")
                || path.file_name().and_then(|n| n.to_str()) == Some("devices.yaml")
            {
                continue;
            }
            let pack = RulePack::from_path(&path)
                .unwrap_or_else(|e| panic!("{} failed to parse: {e}", path.display()));
            let yaml = pack.to_yaml_string().expect("pack should serialize");
            let reparsed = RulePack::from_yaml_str(&yaml)
                .unwrap_or_else(|e| panic!("{} did not re-parse: {e}", path.display()));
            assert_eq!(pack, reparsed, "{} changed meaning", path.display());
            checked += 1;
        }
    }

    assert!(
        checked > 0,
        "no example packs were found under {}",
        rules_dir.display()
    );
}
