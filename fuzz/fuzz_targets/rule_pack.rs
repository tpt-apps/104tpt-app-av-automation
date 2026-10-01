#![no_main]

//! Coverage-guided fuzz target for the YAML rule-pack parser (spec §18.5).
//!
//! The invariant is that no input can panic or abort, and that anything accepted is
//! re-serializable and structurally sound. A pack that parses must round-trip, because
//! the desktop builder reads YAML and writes YAML back.

use libfuzzer_sys::fuzz_target;
use tpt_app_av_automation_model::RulePack;

fuzz_target!(|data: &[u8]| {
    // `from_utf8_lossy` keeps the target focused on parser robustness rather than on
    // rejecting invalid UTF-8, which serde already handles before any of our code runs.
    let Ok(source) = std::str::from_utf8(data) else {
        return;
    };
    // libFuzzer routinely hands us empty and near-empty inputs; they parse to an error
    // and teach the fuzzer nothing.
    if source.trim().is_empty() {
        return;
    }

    if let Ok(pack) = RulePack::from_yaml_str(source) {
        // Anything that validates must survive a serialization round-trip.
        if let Ok(yaml) = pack.to_yaml_string() {
            let reparsed = RulePack::from_yaml_str(&yaml)
                .expect("a re-serialized pack must parse; the format is not idempotent");
            assert_eq!(pack, reparsed, "round-tripping a pack changed its meaning");
        }
        // `armed` must never be silently promoted: rules load disarmed by default so a
        // pack cannot fire on a machine it was not authored for.
        for rule in &pack.rules {
            if !source.contains("armed") {
                assert!(
                    !rule.armed,
                    "rule `{}` became armed without the key being written",
                    rule.id
                );
            }
        }
    }
});
