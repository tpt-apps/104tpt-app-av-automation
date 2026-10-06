# Changelog

All notable changes to `tpt-app-av-automation-core` are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/) and the crate uses semantic versioning. Workspace-wide
changes are recorded in the [root changelog](../../CHANGELOG.md).

## [Unreleased]

Nothing yet.

## [0.1.0]

Initial development release.

### Added
- `Error`/`Result` taxonomy and located `Diagnostic`s with `Severity`.
- Typed identifiers (`RuleId`, `ConditionId`, `ActionId`, `DeviceId`, `ExecutionId`) and `slugify_rule_id`.
- `Timestamp` and the injectable `Clock` (`SystemClock`, `FixedClock`).
- Normalized `Event` with `Protocol`, `DeviceHealth`, `DmxLevel` and `MidiLevel`.
- `RateLimiter` and `BackoffPolicy` for untrusted inbound traffic.

