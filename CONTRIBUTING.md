# Contributing

## Ground rules

* **Simulation must stay incapable of side effects.** Anything reachable from `Actions::simulate`,
  `Mode::Simulation` or a disarmed rule must not touch an endpoint, the notifier, the incident log,
  a subprocess or the network.
* **Inbound traffic is hostile.** Anything parsing bytes from OSC/MIDI/Art-Net/sACN/HTTP returns an
  error for bad input and never panics. Add a malformed-input test with every new parser.
* **Never hide a failure.** A chain with any failed step is `partial_failure` or `failed`.
* **The exit-code contract (0–6) and `format_version: 1` are stable.** Additive changes only.
* The deterministic core (`model`, `conditions`, `engine`) does no I/O and reads no clock; time,
  devices and sleeping are injected.
* Keep product-specific rule logic here; generic protocol fixes belong in `tpt-av-control`.

## Testing

```sh
cargo test --workspace
```

The suite needs the sibling `tpt-av-control` checkout (see the README) and binds local UDP/TCP
ports on `127.0.0.1`.

Every trigger, condition and action needs tests for valid input, invalid input, a boundary case and
malformed input (spec §18.1).

## Regression-fixture policy (spec §18.6)

**Every production bug gets a permanent regression fixture before it is fixed.**

1. Reproduce it with the smallest rule pack and event sequence that shows the bug.
2. Add the pack and the recorded events/expected trace under `tests/fixtures/<short-bug-name>/`,
   plus a test that replays it against the engine (see `crates/tpt-app-av-automation-test`).
3. Confirm the test fails without your fix and passes with it.
4. Fixtures are never deleted or loosened to make a change pass. If behaviour is *meant* to change,
   regenerate the expected trace in the same commit and explain why in the commit message.

The golden traces under `tests/fixtures/golden/` work the same way: a diff in them is a behaviour
change and must be deliberate.

## Style

`cargo fmt` and `cargo clippy --workspace --all-targets` clean. Comment *why*, not what; match the
density of the surrounding code.
