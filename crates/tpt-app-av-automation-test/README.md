# tpt-app-av-automation-test

Replay harness for golden, fault-injection and latency tests of
[TPT AV Automation](../../README.md). It exists so that rule behaviour can be tested on a laptop
with no AV equipment, and so that any change in behaviour shows up as a reviewable diff.

## Fixtures

A *fixture* is a directory:

```text
pack.yaml       the rule pack under test
devices.yaml    devices; every protocol must be `virtual`
script.yaml     the recorded sequence of events, time steps and injected faults
expected.json   the golden trace (execution records + what each device received)
```

`Replay::run(dir)` plays the script against the **real engine** with a fixed clock and virtual
devices, so the result is fully deterministic. `assert_golden(dir)` compares it with
`expected.json`. Set `UPDATE_GOLDEN=1` to (re)write the expected trace; a diff in that file is a
behaviour change and must be deliberate (see [CONTRIBUTING.md](../../CONTRIBUTING.md)).

## API

- `Replay::run`, `Replay::play`, `Replay::to_json`: execute a fixture or an in-memory
  `RulePack` + `DeviceFile` + `Script`.
- `Script`, `Step`, `FaultSpec`, `HealthSpec`: the script format (events, clock steps, device
  health changes, dropped messages, devices going offline mid-chain).
- `fixtures_root`, `discover(group)`, `assert_golden`: locate and check fixtures under
  `tests/fixtures/`.
- `LatencySamples`, `Percentiles`: percentile summaries for latency budgets.
- `REPLAY_START_MS`: the fixed start time used by every replay.

## Test suites in this crate

`tests/golden.rs`, `tests/chaos.rs`, `tests/fuzz.rs`, `tests/latency.rs` and `tests/roundtrip.rs`
cover the golden fixtures, chaos scripts, malformed-input robustness, latency percentiles and
pack/record serialization round trips.

## Example

```rust,ignore
use tpt_app_av_automation_test::{assert_golden, discover};

#[test]
fn golden_fixtures_match() {
    for dir in discover("golden") {
        assert_golden(&dir);
    }
}
```

## Licence

Dual-licensed under [MIT](../../LICENSE-MIT) or [Apache-2.0](../../LICENSE-APACHE), at your option.
