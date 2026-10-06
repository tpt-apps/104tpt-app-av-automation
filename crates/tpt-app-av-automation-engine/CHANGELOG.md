# Changelog

All notable changes to `tpt-app-av-automation-engine` are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/) and the crate uses semantic versioning. Workspace-wide
changes are recorded in the [root changelog](../../CHANGELOG.md).

## [Unreleased]

Nothing yet.

## [0.1.0]

Initial development release.

### Added
- Deterministic, priority-ordered rule matching with injected clock, actions and endpoints.
- Conflict resolution, failure policies with fallback, per-step and chain timeouts.
- `CancelHandle` for one execution, one rule or all executions.
- Depth-limited nested rule invocation.
- Live and simulation `Mode`, arm/disarm, and `snapshot` / `restore` of `EngineState`.
- Device cross-checks: `check_devices` and `rules_using_device`.

