# Changelog

All notable changes to `tpt-app-av-automation-test` are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/) and the crate uses semantic versioning. Workspace-wide
changes are recorded in the [root changelog](../../CHANGELOG.md).

## [Unreleased]

Nothing yet.

## [0.1.0]

Initial development release.

### Added
- `Replay` harness playing recorded scripts against the real engine with a fixed clock and virtual devices.
- `Script`, `Step`, `FaultSpec` and `HealthSpec` script format.
- `assert_golden` with `UPDATE_GOLDEN=1` regeneration, plus fixture discovery.
- `LatencySamples` / `Percentiles` for latency budgets.
- Golden, chaos, fuzz-style, latency and round-trip test suites.

