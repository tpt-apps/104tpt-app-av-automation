# Changelog

All notable changes to `tpt-app-av-automation-service` are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/) and the crate uses semantic versioning. Workspace-wide
changes are recorded in the [root changelog](../../CHANGELOG.md).

## [Unreleased]

### Fixed / Changed
- A mistyped key in the service settings (including a misspelled `api:` key) was silently ignored; it is now an error.

## [0.1.0]

Initial development release.

### Added
- `Service` runtime: engine thread, UDP/MIDI listeners, tick loop and heartbeat checks.
- SQLite `Store` for execution history, incidents, pack versions and restart-safe one-shot schedules.
- Local REST API and WebSocket event stream, disabled by default, loopback-only with a bearer token.
- `supervise` watchdog with `WatchdogPolicy`.
- `UiBridge` command surface for the desktop app, including the device manager (live apply, `devices.yaml.bak` backup, in-use delete protection).
- Built-in starter `templates`.

