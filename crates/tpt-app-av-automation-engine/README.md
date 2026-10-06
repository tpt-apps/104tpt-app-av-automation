# tpt-app-av-automation-engine

The deterministic rule evaluation engine of [TPT AV Automation](../../README.md).

```text
event ──▶ ingest ──▶ match (priority order) ──▶ conditions ──▶ conflict check ──▶ action chain
                                                    │                                 │
                                                    └── not met / unknown: Skipped    ├─ live: Actions::execute
                                                                                      └─ disarmed / simulation: Actions::simulate
```

Given the same pack, registry state, clock and event, `Engine::handle_event` produces the **same
decision and the same ordered `ExecutionRecord`s**. Everything non-deterministic (time, device I/O,
sleeping) is injected: the `Clock`, the `Actions` dispatcher and its endpoints.

## Behaviour

- **Priority-ordered matching.** Rules are evaluated highest priority first.
- **Three-valued conditions.** `NotMet` and `Unknown` both skip the rule, and the record says which.
- **Conflict resolution.** Rules contending for the same target are resolved by priority and policy.
- **Failure policies.** Per-step `FailurePolicy` with fallback steps; per-step and whole-chain
  timeouts.
- **Cancellation.** `CancelHandle` cancels one execution, every execution of a rule, or all of them.
- **Nested invocation.** `invoke.rule` is depth-limited so rules cannot recurse forever.
- **Arm / disarm.** Rules load **disarmed**. A disarmed rule is simulated, never executed.
- **Modes.** `Mode::Live` (armed rules act) or `Mode::Simulation` (nothing is ever sent).
- **Restart safety.** `snapshot()` / `restore()` and `set_state_hook` expose `EngineState`
  (scheduler state, armed set) for persistence by the service layer.

## Example

```rust,ignore
use std::sync::Arc;
use tpt_app_av_automation_engine::{Engine, EngineConfig, Mode};
use tpt_app_av_automation_core::FixedClock;

let mut engine = Engine::new(pack, registry, actions, Arc::new(clock), EngineConfig::default())?;
engine.arm("event-start")?;
let records = engine.handle_event(event);   // deterministic for identical inputs
let due = engine.tick();                    // advance schedules
```

`Engine::new` validates the pack and refuses one with errors. `check_devices` reports rules that
name devices which do not exist, and `rules_using_device` answers "what breaks if I delete this?".

## Where it fits

Depends on `core`, `model`, `triggers`, `conditions`, `actions` and `devices`; wrapped by the
`service` crate for headless operation. Design notes: [docs/architecture.md](../../docs/architecture.md)
and [docs/reliability.md](../../docs/reliability.md).

## Licence

Dual-licensed under [MIT](../../LICENSE-MIT) or [Apache-2.0](../../LICENSE-APACHE), at your option.
