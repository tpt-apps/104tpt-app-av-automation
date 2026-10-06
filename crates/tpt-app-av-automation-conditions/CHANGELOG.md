# Changelog

All notable changes to `tpt-app-av-automation-conditions` are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/) and the crate uses semantic versioning. Workspace-wide
changes are recorded in the [root changelog](../../CHANGELOG.md).

## [Unreleased]

Nothing yet.

## [0.1.0]

Initial development release.

### Added
- `evaluate` and `gate` with three-valued `Met` / `NotMet` / `Unknown` results.
- `EvaluationContext` snapshot of time, device health, DMX levels, parameters and the armed set.
- Device health, time window (including across midnight), DMX channel and device parameter conditions.

