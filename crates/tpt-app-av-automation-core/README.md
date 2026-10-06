# tpt-app-av-automation-core

Shared primitives for [TPT AV Automation](../../README.md): identifiers, clocks, errors, the
normalized event and rate limiting.

This crate deliberately contains **no** AV, media or protocol knowledge. It sits at the bottom of the
dependency graph: every other crate in the workspace depends on it, and it depends only on `serde`,
`serde_json`, `thiserror` and `tracing`. It is `#![forbid(unsafe_code)]`.

## What is in it

| Item | Purpose |
|------|---------|
| `Error`, `Result` | One error taxonomy for the whole workspace. |
| `Diagnostic`, `Severity` | A located validation message (error or warning), used instead of a boolean "valid". |
| `RuleId`, `ConditionId`, `ActionId`, `DeviceId`, `ExecutionId` | Typed identifiers, so a rule id cannot be passed where a device id is expected. |
| `slugify_rule_id` | Derive a stable, URL-safe rule id from a display name. |
| `Timestamp` | Millisecond timestamp used instead of `SystemTime`, so evaluation stays deterministic. |
| `Clock`, `SystemClock`, `FixedClock` | Injectable time source. The deterministic core never reads the wall clock directly. |
| `Event`, `Protocol` | The normalized event every trigger matches against, whichever protocol it arrived on. |
| `DeviceHealth`, `DmxLevel`, `MidiLevel` | Event payload types for device health and control-level changes. |
| `RateLimiter`, `BackoffPolicy` | Per-source inbound rate limiting with adaptive backoff. Control protocols are unauthenticated, so inbound traffic is treated as untrusted. |

## Example

```rust
use tpt_app_av_automation_core::{Clock, FixedClock, Timestamp};

// Tests and simulation drive time explicitly; the engine never calls the OS clock itself.
let clock = FixedClock::new(Timestamp::now());
let before = clock.now();
clock.advance_millis(60_000);
assert_eq!(clock.now().millis_since(before), 60_000);
```

## Design notes

- **Determinism first.** Given the same pack, registry state, clock and event, the engine produces
  the same decision. Injecting `Clock` is what makes simulation, golden tests and chaos tests
  reproducible.
- **Stable by contract.** Identifier types and the `Event` shape are serialized into the state
  database and the local API; changes are recorded in [CHANGELOG.md](CHANGELOG.md).

## Where it fits

`core` → `model` → `triggers` / `conditions` / `devices` → `actions` → `engine` → `service` → `cli`.
See [docs/architecture.md](../../docs/architecture.md).

## Licence

Dual-licensed under [MIT](../../LICENSE-MIT) or [Apache-2.0](../../LICENSE-APACHE), at your option.
