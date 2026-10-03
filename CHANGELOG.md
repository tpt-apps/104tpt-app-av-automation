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
- MIDI 2.0/UMP: raw Universal MIDI Packets parsed from a `ump` UDP listener and normalized to the
  same event shape as MIDI 1.0, carrying the 32-bit value and UMP group (inbound only; outbound
  actions still emit MIDI 1.0 bytes).
- DMX512-A serial output (`dmx512` devices) with driver-generated break framing, lazy reconnect and
  single-universe enforcement (not tested against a physical adapter).
- Integration and chaos scenario suites driving the shipped binary in a real process: rule-pack
  lifecycle, OSC/Art-Net/MIDI 2.0 over real sockets, forced-restart durability, and hostile
  inbound traffic (`tests/integration/`, `tests/chaos/`).
- Persistent incident log and `GET /incidents`.
- Write-ahead scheduler state: one-shot schedules are at-most-once across a hard kill.
- Golden rule-pack traces, chaos tests, mutation-based robustness tests and a throughput test.
- Coverage-guided fuzz targets (`fuzz/`) for the rule-pack parser, inbound OSC/MIDI/Art-Net/sACN and the
  local API, run by CI on Linux with AddressSanitizer. Windows cannot run them: Rust ships no ASan
  runtime for `x86_64-pc-windows-msvc`.
- GitHub Actions CI: Linux (fmt, clippy, systemd unit validation, tests), Windows (tests, packaged
  release smoke test) and nightly fuzzing.
- Windows packaging script (static CRT) and a hardened systemd unit (Linux unvalidated).

### Fixed
- Mistyped keys in a rule pack were silently ignored instead of rejected, so an operator could arm a
  show believing a cue was configured when the engine had dropped the instruction. Packs, rules,
  policies, conditions, actions and triggers are now strict; unit-variant triggers (`type: manual`)
  are audited against a key allowlist, which serde cannot police on its own.
- `device` inside an `action_library` fragment was dropped without complaint, leaving a fragment with
  no target. The target belongs on the calling `workflow.use_action` step and is now required there.
- `Event::Midi` serialized two `kind` keys, so any persisted MIDI execution could not be read back.
- `RulePack::from_path` no longer reports parse and validation failures as storage errors, which
  hid the located diagnostics and produced the wrong CLI exit code.

### Not yet implemented
Desktop application; `tpt-kinetix`/`tpt-cadence` media backends; Phase 2 and 3 features.
