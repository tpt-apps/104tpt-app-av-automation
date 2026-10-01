//! Mutation-based robustness tests for the rule-pack parser (spec §18.5).
//!
//! These are *seeded, deterministic* mutation tests: they start from every valid pack in the
//! repository and corrupt it thousands of ways (bit flips, truncation, splicing, structural
//! tokens, line shuffles), asserting that parsing and validation never panic and that anything
//! that still parses is safe to run in the engine. They complement — they do not replace — a
//! coverage-guided fuzzer (`cargo fuzz`), which needs a nightly toolchain and is tracked in
//! `todo.md`.

use std::path::PathBuf;
use std::sync::Arc;

use tpt_app_av_automation_actions::Actions;
use tpt_app_av_automation_core::{Event, FixedClock};
use tpt_app_av_automation_devices::{Device, DeviceRegistry};
use tpt_app_av_automation_engine::{Engine, EngineConfig, Mode};
use tpt_app_av_automation_model::RulePack;
use tpt_app_av_automation_test::{discover, fixtures_root};

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

const TOKENS: &[&str] = &[
    "- ", "  ", "\n", ": ", "[", "]", "{", "}", "\"", "'", "&a ", "*a", "!!str ", "---\n", "? ", "null",
    "~", "true", "-1", "18446744073709551615", "1e999", "0x10", "\t", "\u{0}", "\u{feff}", "%", "|\n", ">\n",
    "<<: ", "type: workflow.invoke_rule", "type: control.dmx_channels", "start_channel: 65535",
    "values: [", "rule: self", "armed: true", "id: ", "timeout_ms: 0", "at: \"99:99\"",
];

fn mutate(rng: &mut Rng, seed: &[u8]) -> Vec<u8> {
    let mut data = seed.to_vec();
    for _ in 0..=rng.below(6) {
        if data.is_empty() {
            data.push(b'a');
        }
        match rng.below(8) {
            0 => {
                let i = rng.below(data.len());
                data[i] ^= 1 << rng.below(8);
            }
            1 => {
                let i = rng.below(data.len());
                data[i] = rng.next() as u8;
            }
            2 => data.truncate(rng.below(data.len() + 1)),
            3 => {
                let i = rng.below(data.len() + 1);
                data.splice(i..i, TOKENS[rng.below(TOKENS.len())].bytes());
            }
            4 => {
                let (a, b) = (rng.below(data.len()), rng.below(data.len()));
                let (lo, hi) = (a.min(b), a.max(b));
                let chunk = data[lo..hi].to_vec();
                let at = rng.below(data.len() + 1);
                data.splice(at..at, chunk);
            }
            5 => {
                let (a, b) = (rng.below(data.len()), rng.below(data.len()));
                data.drain(a.min(b)..a.max(b));
            }
            6 => {
                // Swap two lines.
                let text = String::from_utf8_lossy(&data).into_owned();
                let mut lines: Vec<&str> = text.lines().collect();
                if lines.len() > 1 {
                    let (a, b) = (rng.below(lines.len()), rng.below(lines.len()));
                    lines.swap(a, b);
                    data = lines.join("\n").into_bytes();
                }
            }
            _ => {
                let i = rng.below(data.len() + 1);
                let deep = "- ".repeat(rng.below(200));
                data.splice(i..i, deep.bytes());
            }
        }
    }
    data
}

fn seed_packs() -> Vec<String> {
    let mut packs = Vec::new();
    let mut paths: Vec<PathBuf> = discover("golden").into_iter().map(|d| d.join("pack.yaml")).collect();
    let rules = fixtures_root().join("../../rules");
    for sub in ["examples", "live-event", "failover"] {
        if let Ok(entries) = std::fs::read_dir(rules.join(sub)) {
            for e in entries.flatten() {
                let p = e.path();
                if p.extension().is_some_and(|x| x == "yaml") && p.file_name().is_some_and(|n| n != "devices.yaml") {
                    paths.push(p);
                }
            }
        }
    }
    for p in paths {
        if let Ok(text) = std::fs::read_to_string(&p) {
            packs.push(text);
        }
    }
    packs
}

fn run_in_engine(pack: RulePack) {
    let mut registry = DeviceRegistry::new();
    for rule in &pack.rules {
        for device in rule.targeted_devices() {
            registry.register(Device::new(device.as_str(), device.as_str()));
        }
    }
    let config = EngineConfig {
        mode: Mode::Simulation,
        ..EngineConfig::default()
    };
    let Ok(mut engine) = Engine::new(pack, registry, Actions::with_defaults(), Arc::new(FixedClock::default()), config)
    else {
        return;
    };
    for event in [
        Event::Manual { rule: None },
        Event::Osc {
            address: "/go".into(),
            args: vec![1.0],
        },
        Event::Schedule { spec: "18:55".into() },
    ] {
        let _ = engine.handle_event(event);
    }
    let _ = engine.tick();
}

#[test]
fn mutated_rule_packs_never_panic_the_parser_validator_or_engine() {
    let seeds = seed_packs();
    assert!(seeds.len() >= 10, "seed corpus is missing ({} packs)", seeds.len());
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let (mut parsed, mut rejected) = (0u32, 0u32);
    for round in 0..4000 {
        let seed = &seeds[round % seeds.len()];
        let bytes = mutate(&mut rng, seed.as_bytes());
        // Invalid UTF-8 must be handled by the caller; lossy conversion keeps the garbage coming.
        let text = String::from_utf8_lossy(&bytes);
        match RulePack::from_yaml_str(&text) {
            Ok(pack) => {
                parsed += 1;
                let _ = pack.to_yaml_string();
                run_in_engine(pack);
            }
            Err(_) => rejected += 1,
        }
    }
    eprintln!("fuzz-style run: {parsed} packs still parsed, {rejected} rejected");
    assert!(rejected > 0, "the mutator is not producing invalid input");
}

#[test]
fn pathological_documents_are_rejected_not_fatal() {
    let deep_seq = format!("{}x{}", "[".repeat(5000), "]".repeat(5000));
    let deep_map = "a: ".repeat(3000) + "1";
    let huge_scalar = format!("name: {}\n", "x".repeat(2_000_000));
    let many_rules = {
        let mut s = String::from("format_version: 1\nname: Big\nrevision: 1\nrules:\n");
        for i in 0..2000 {
            s.push_str(&format!(
                "  - {{id: r{i}, name: R, trigger: {{type: manual}}, actions: [{{id: a, type: notify.operator, message: m}}]}}\n"
            ));
        }
        s
    };
    for doc in [deep_seq.as_str(), deep_map.as_str(), huge_scalar.as_str(), "\u{0}\u{0}\u{0}", "&a [*a, *a]"] {
        let _ = RulePack::from_yaml_str(doc);
    }
    let big = RulePack::from_yaml_str(&many_rules).expect("2000 small rules are legitimate");
    assert_eq!(big.rules.len(), 2000);
}
