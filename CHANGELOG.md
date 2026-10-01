# Changelog

All notable changes to this project are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/) and the project uses semantic versioning once
released. The CLI exit-code contract (0–6) and the `format_version: 1` rule format are stable.

## [Unreleased]

### Added
- Rule pack model, YAML format and validation (`model`).
- Deterministic engine: priority-ordered matching, three-valued conditions, conflict resolution,
  failure policies with fallback, per-step and chain timeouts, operator cancellation, nested rule
  invocation, simulation mode and arm/disarm (`engine`).
- Triggers: schedule (cron-like, interval, one-shot), OSC, MIDI, DMX/Art-Net/sACN, device health,
  heartbeat-missed, device parameter, manual, API.
- Actions: OSC, MIDI, DMX channels/universe/scene, notify operator, log incident, wait, invoke rule,
  library fragments, sandboxed external program (disabled by default).
- Device registry with heartbeat monitoring; virtual endpoints with fault injection; UDP (OSC,
  Art-Net, sACN) and MIDI endpoints on the `tpt-av-control` codecs.
- Inbound validation and per-source rate limiting.
- SQLite persistence; restart-safe one-shot schedules.
- Headless service, watchdog, local REST + WebSocket API (disabled by default, loopback + token).
- CLI: `validate`, `simulate`, `run`, `watchdog`, `history`.
- Live MIDI I/O by port name through `midir`, with lazy reconnection (not hardware-tested).
- Persistent incident log and `GET /incidents`.
- Write-ahead scheduler state: one-shot schedules are at-most-once across a hard kill.
- Golden rule-pack traces, chaos tests, mutation-based robustness tests and a throughput test.
- Windows packaging script (static CRT) and a hardened systemd unit (Linux unvalidated).

### Fixed
- `Event::Midi` serialized two `kind` keys, so any persisted MIDI execution could not be read back.
- `RulePack::from_path` no longer reports parse and validation failures as storage errors, which
  hid the located diagnostics and produced the wrong CLI exit code.

### Not yet implemented
Desktop application; `tpt-kinetix`/`tpt-cadence` media backends; Phase 2 and 3 features.
