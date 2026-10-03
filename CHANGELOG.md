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
- Inbound validation, per-source rate limiting and adaptive backoff.
- Phase 2 unit-test matrix (`tests/matrix.rs`): valid, invalid, boundary and malformed cases for
  every trigger, condition and action form, asserting the matrix stays complete.
- SQLite persistence; restart-safe one-shot schedules.
- Headless service, watchdog, local REST + WebSocket API (disabled by default, loopback + token).
- CLI: `validate`, `simulate`, `run`, `watchdog`, `history`.
- Live MIDI I/O by port name through `midir`, with lazy reconnection (not hardware-tested).
- MIDI 2.0/UMP: raw Universal MIDI Packets parsed from a `ump` UDP listener and normalized to the
  same event shape as MIDI 1.0, carrying the 32-bit value and UMP group.
- MIDI 2.0 outbound: `control.midi` accepts `group`, `value32` and `index`, and a `ump` device encodes
  them as Universal MIDI Packets, including all ten MIDI 2.0-only message kinds. A MIDI 1.0 port
  rejects those fields rather than silently truncating them.
- DMX512-A serial output (`dmx512` devices) with driver-generated break framing, lazy reconnect and
  single-universe enforcement (not tested against a physical adapter).
- DMX512-A serial input: a `dmx512` service listener recovers frame boundaries from inter-byte
  timing (`Dmx512Assembler`) and diffs each frame into the same channel-change events Art-Net and
  sACN produce (not tested against a physical adapter).
- Sunrise/sunset-relative schedules (`solar: {event, minutes}`) computed offline from a pack-level
  `site` using the NOAA solar-position algorithm; polar days report why they never fire rather than
  being given an invented time.
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
- The inbound rate limiter and the fixed clock recovered nothing from a poisoned lock, unlike every
  other shared-state lock in the workspace: 16 `.expect()` calls. Both sit on paths a panic anywhere
  in the process can poison, and `RateLimiter::try_acquire` runs for every inbound datagram, so a
  poison there would take a listener thread down while `/health` kept reporting nominal. They now
  recover, with tests proving accounting still works after a panic inside the critical section.
- A listener thread that panicked was lost silently: nothing joined or watched the workers, so the
  process stayed up and healthy-looking while one protocol stopped receiving. Workers are now
  counted, `/health` reports `listeners: {expected, live, lost}` and drops to `degraded` when one
  is lost, and a panicking worker is accounted exactly like one that returns.
- A `ump` device named in `devices.yaml` passed configuration validation but then failed to build,
  because `build_endpoint` had no `ump` arm — every MIDI 2.0 outbound action failed at load.
- A `dmx_channel` *condition* was not bounds-checked against the universe, so a rule could gate on
  channel 512 and silently never pass. `dmx_channel` triggers were already checked.
- A `rule_armed` condition naming a rule that does not exist was not reported. Such a condition can
  never be met, so the rule sat armed and never fired; it is now a load-time error, matching how an
  `invoke_rule` naming a missing rule is already handled.
- `device_health` and `device_parameter` conditions accepted an empty `device`, and
  `device_parameter` an empty `parameter`.
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
