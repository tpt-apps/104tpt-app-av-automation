# Changelog

All notable changes to this project are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/) and the project uses semantic versioning once
released. The CLI exit-code contract (0–6) and the `format_version: 1` rule format are stable.

## [Unreleased]

### Fixed
- `solar` schedules were documented and modelled but rejected when loading YAML ("unknown field
  `solar`"); they now load.
- A mistyped key in `devices.yaml` or the service settings (`heartbeat_msec`, `exec_alow`, a
  misspelled `api:` key) was silently ignored; it is now an error.

### Added
- Desktop device manager: add, edit and delete devices from the Devices tab. Changes apply live
  without a restart and are written to the devices file at once (the original is kept once as
  `devices.yaml.bak`); a device a rule still uses cannot be deleted. With no devices file
  configured, `devices.yaml` is created beside the state database on the first edit.
- Stricter device validation: `host:port` addresses, id characters, heartbeat range.
- `tpt-av-automation init`: scaffold a show folder from a built-in template (`--list` shows them);
  starter templates under `rules/templates/` (classroom, meeting room, worship, theatre, museum,
  exterior lights, failover).
- `docs/devices.md`: the device file format, per-protocol addresses and health behaviour.
- `run` warns when started without `--state-dir`.
- `media` device protocol: `media.video_source` / `media.audio_route` delivered as OSC at
  per-device address templates, with address-segment validation.
- `scripts/install-windows-service.ps1`: registers the watchdog as a boot-time scheduled task.
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
- Latency suite (`tests/latency.rs`) reporting percentiles rather than averages: engine dispatch against
  a 20 ms cue budget, the dispatch curve from 5 to 500 rules, the cost of an unmatched event, and an
  OSC datagram in → datagram out across two loopback sockets through the production inbound gate.
  Measured figures and how to reproduce them are in `docs/reliability.md`.
- Coverage-guided fuzz targets (`fuzz/`) for the rule-pack parser, inbound OSC/MIDI/Art-Net/sACN and the
  local API, run by CI on Linux with AddressSanitizer. Windows cannot run them: Rust ships no ASan
  runtime for `x86_64-pc-windows-msvc`.
- GitHub Actions CI: Linux (fmt, clippy, systemd unit validation, tests), Windows (tests, packaged
  release smoke test) and nightly fuzzing.
- Windows packaging script (static CRT) and a hardened systemd unit (Linux unvalidated).
- Desktop application (Tauri, `crates/tpt-app-av-automation-tauri`, excluded from the default
  workspace build): dashboard, rule builder, device manager and execution timeline over an
  embedded service — the same engine, pack operations and status document as the CLI and the local
  API (spec §12). The builder canvas and the Pack YAML view are two renderings of one
  representation: edits go through the engine's own parse/validate/hot-load path, armed state is
  always visible, the LIVE/SIMULATION banner is always on screen, and a pack is applied or
  rejected with the engine's located diagnostics. Device manager supports manual pings; the
  timeline filters by rule, status and mode and jumps back to the rule that produced an execution.
  The webview holds no business logic: the `service::ui` bridge is covered by Rust integration
  tests, and the renderer's pure helpers by `node --test`.

### Fixed
- The `misbehaving_peers` chaos scenarios (`tests/chaos/misbehaving_peers.rs`) could fail on a loaded
  machine, including under `cargo test --workspace` where every scenario runs at once. They reserve
  loopback ports, release them, then let the engine bind — and on a collision the engine exits, which
  is correct behaviour but not the failure under test. The fixture waited out the full 20 s
  `PATIENCE` and then failed the scenario, so a transient port race became a red build. It now
  retries with fresh ports, as the integration suite already did.
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
`tpt-kinetix`/`tpt-cadence` media backends; desktop packaging/installer and hardware-validated
desktop use; Phase 2 and 3 features.
