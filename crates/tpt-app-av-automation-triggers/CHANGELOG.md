# Changelog

All notable changes to `tpt-app-av-automation-triggers` are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/) and the crate uses semantic versioning. Workspace-wide
changes are recorded in the [root changelog](../../CHANGELOG.md).

## [Unreleased]

Nothing yet.

## [0.1.0]

Initial development release.

### Added
- `Scheduler` with edge-triggered, minute-idempotent firing and persistable `SchedulerState`.
- `Inbound` validation of OSC, MIDI, MIDI 2.0 UMP, Art-Net and sACN datagrams.
- Per-source rate limiting with adaptive backoff and rejection counters.
- `DmxTracker` for crossed-above / crossed-below edge triggers.
- `LocalClock` / `LocalMoment` local-time conversion from an injected clock.

