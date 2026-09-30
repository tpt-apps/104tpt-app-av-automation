# tpt-app-av-automation-core

Shared primitives for TPT AV Automation.

This crate deliberately contains **no** AV, media or protocol knowledge. It holds the pieces every
other crate needs:

- [`Error`] / [`Result`] — one error taxonomy for the whole workspace.
- Identifiers (`RuleId`, `ConditionId`, `ActionId`, `DeviceId`, `ExecutionId`).
- [`Timestamp`] — a monotonic-anchored millisecond timestamp used instead of `SystemTime` so that
  rule evaluation stays deterministic and testable (spec §3.2).
- [`Clock`] / [`FixedClock`] — injectable time source; the deterministic core never reads the wall
  clock directly.
- [`Event`] — the normalized event that every trigger matches against (spec §10).
- [`RateLimiter`] — inbound trigger rate limiting, required because control protocols are
  unauthenticated and untrusted input (spec §16).

## Licence

Dual MIT / Apache-2.0. See `LICENSE-MIT` and `LICENSE-APACHE` in the repository root.

```text
d:\Programming\1PRODUCTION\Open Source\104tpt-app-av-automation\crates\tpt-app-av-automation-core\README.md
```