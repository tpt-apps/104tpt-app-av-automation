# Changelog

All notable changes to `tpt-app-av-automation-actions` are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/) and the crate uses semantic versioning. Workspace-wide
changes are recorded in the [root changelog](../../CHANGELOG.md).

## [Unreleased]

Nothing yet.

## [0.1.0]

Initial development release.

### Added
- `Actions::execute` and `Actions::simulate` with identical signatures; simulation has no endpoint access.
- OSC, MIDI, DMX, media, notify, incident, wait, invoke-rule and library-fragment steps.
- `CancelToken` operator cancellation.
- Sandboxed external program execution behind an `ExecPolicy` allow-list, disabled by default.
- `Notifier`, `IncidentSink` and `Sleeper` traits with tracing, in-memory and null implementations.

