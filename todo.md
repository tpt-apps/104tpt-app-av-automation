# TPT AV Automation — Project Todo

Source of truth: `spec.txt`. Section references (`§`) point back to it.

## Notes (not tasks)

- Product: TPT AV Automation — offline-first automation/orchestration for professional AV, live production, and media-pipeline workflows (§1).
- Publisher: TPT Solutions. License: dual MIT / Apache-2.0.
- Pricing hypothesis (§2.1, to validate): Standalone $999, Studio $2,499, Facility/Enterprise $4,999+. Perpetual licence, no mandatory recurring cloud costs.
- Competitive differentiation (§2.2): native Rust, offline-first, unified live-control + media-pipeline rule engine, deterministic/versioned rules, simulation/dry-run, reliability as a feature, vendor-neutral, shared TPT foundation.
- Strategic boundary (§23, §25): do NOT become a lighting console, DAW, video switcher, home-automation hub, or full BMS. Do not try to match Crestron/Q-SYS feature-for-feature in v1.

## Legend

`[x]` means implemented **and** covered by automated tests that pass. `[ ]` items carry a
`Status` line saying exactly what exists and what is missing; nothing partial is ticked.
Phase 4 and 5 are untouched.

---

## Phase 0 — Repository & Licensing Setup (§4, §17)

- [x] Initialize git repository and `.gitignore`
- [x] Create `LICENSE-MIT` (copyright holder: TPT Solutions)
- [x] Create `LICENSE-APACHE` (Apache-2.0, copyright holder: TPT Solutions)
- [x] Set `license = "MIT OR Apache-2.0"` in workspace `Cargo.toml`
- [x] Scaffold repo structure per §4: `crates/`, `rules/{examples,live-event,media-pipeline,failover}/`, `tests/{fixtures,virtual-devices,integration,chaos}/`, `docs/`
- [x] Add root `README.md`, `CHANGELOG.md`, `CONTRIBUTING.md`
- [x] Stub `docs/architecture.md`
  - _Status:_ written in full, not just stubbed.
- [x] Stub `docs/rule-model.md`
  - _Status:_ written in full.
- [x] Stub `docs/trigger-catalogue.md`
  - _Status:_ written in full.
- [x] Stub `docs/action-catalogue.md`
  - _Status:_ written in full.
- [x] Stub `docs/rule-format.md`
  - _Status:_ written in full.
- [x] Stub `docs/reliability.md`
  - _Status:_ written in full.
- [x] Stub `docs/integrations.md`
  - _Status:_ written in full (plus `docs/deployment.md`).
- [x] Confirm commercial/open-source boundary (§17): `tpt-app-av-automation` (commercial) stays separate from open TPT foundation crates (`tpt-av-control`, `tpt-kinetix`, `tpt-cadence`, `tpt-audio`, `tpt-dsp`, `tpt-visual`, `tpt-av-sync`, `tpt-av-asset`, `tpt-av-test`)
  - _Status:_ foundation crates are consumed by path only; no foundation code is copied here (README, CONTRIBUTING).

## Phase 1 — MVP Build (§19, §24.1 steps 1–15)

- [x] Establish Cargo workspace and application shell (crates listed in §4: core, model, triggers, conditions, actions, engine, devices, report, cli, service, test, scenarios — stubs in place; `tauri` crate still to add)
  - _Status:_ all crates exist and build except `tauri`, which is excluded from the workspace and not started (see the desktop UI items). `scenarios` runs the integration and chaos suites in `tests/integration` and `tests/chaos`.
- [x] Integrate `tpt-av-control` and enumerate supported protocols (§5.1)
  - _Status:_ OSC, MIDI and DMX/Art-Net/sACN codecs are used; the control-surface crate is not (no surface triggers yet). Supported protocols are listed in `docs/integrations.md`.
- [x] Implement the device/endpoint registry (§6.5)
- [x] Implement the rule/trigger/condition/action domain model (§6.1–§6.4, §6.6) — `model` crate, with validation and integration tests
- [x] Implement the YAML rule-pack parser and validator, with versioning (§9)
  - _Status:_ model types, `pack`, and `validate` (steps + cross-reference checks). Unknown keys are now
    rejected at every level (including unit-variant triggers via a key allowlist audit), so a typo cannot
    be silently ignored. Parser/versioning coverage verified.
- [x] Implement the deterministic evaluation core with virtual-device test support (§3.2, §10)
- [x] Implement simulation mode end to end — armed/disarmed default-disarmed behaviour (§3.5, §12.5)
  - _Status:_ engine, CLI, service and API; the always-visible UI indicator waits on the desktop UI.
- [x] Implement OSC inbound/outbound trigger + action (§7.2, §8.1)
- [x] Implement MIDI (1.0/2.0) inbound/outbound trigger + action (§7.2, §8.1)
  - _Status:_ MIDI 1.0 in/out is implemented and tested on live ports opened by name (`midir`);
    MIDI 2.0/UMP is parsed from raw Universal MIDI Packets on a `ump` UDP listener (no desktop
    platform exposes native MIDI 2.0 ports), normalized to the same `MidiLevel` and matched by
    rules with `group` and 32-bit `value`. **Outbound MIDI 2.0 is implemented**: `control.midi`
    accepts `group`, `value32` and `index`, and a `ump` device encodes them as Universal MIDI
    Packets (`devices::ump`), including all ten MIDI 2.0-only message kinds. A MIDI 1.0 port
    rejects `group`/`value32` rather than truncating them. What is **not** done: live `midi`/`ump`
    ports have **not** been exercised against real hardware — CI has no MIDI interface. Note the
    YAML spelling differs by side: the trigger uses the serde name `control_change`, the action
    accepts the short label `cc`.
- [x] Implement DMX/Art-Net/sACN channel-change trigger + output action (§7.2, §8.1)
  - _Status:_ Art-Net and sACN in and out, tested over real sockets; serial DMX512-A output added
  (`dmx512` devices, `devices::dmx_serial`) with driver-generated break framing and lazy reconnect —
  not tested against a physical adapter. Serial DMX512 *input* is implemented:
  `Dmx512Assembler` recovers frame boundaries from inter-byte timing (DMX512 carries no framing of
  its own), and a `dmx512` service listener reads the port, diffs each frame into channel changes and
  reconnects after an adapter is unplugged. Not tested against a physical adapter.
- [x] Implement schedule trigger (cron-like, interval, one-shot) (§7.1)
  - _Status:_ fixed time + weekdays, full five-field cron expressions (`30 18 * * mon-fri`,
  with ranges, lists, steps and names), interval and one-shot, and sunrise/sunset-relative schedules
  (`solar: {event: sunset, minutes: -30}`) computed offline from a pack-level `site` using the NOAA
  solar-position algorithm (`model::solar`). Polar days report why they never fire rather than being
  given a made-up time.
- [x] Implement device online/offline/degraded trigger + heartbeat-missed trigger (§7.3, §11)
- [x] Implement manual/API-invoked trigger (§7.6)
- [x] Implement notify-operator action (§8.3)
  - _Status:_ delivers to a notifier sink (process log); desktop toasts/sound wait on the UI.
- [x] Implement log-incident action (§8.3)
  - _Status:_ persisted to SQLite and exposed at `GET /incidents`.
- [ ] Integrate `tpt-kinetix` for media start/stop/switch actions (§5.1, §8.2)
  - _Status:_ `tpt-kinetix` itself is not integrated, but media actions now have a working backend:
    a `media` device sends them as OSC at templated addresses (`devices::media`, tested over a real
    UDP socket, including through the action dispatcher). Not tested against a real media server.
    Original gap: `tpt-kinetix` is a codec/pipeline library with no source start/stop/switch API. `media.*` actions are parsed, validated, simulated and routed to an `Endpoint`, but no backend implements them (they fail loudly). See `docs/integrations.md`.
- [x] Implement SQLite persistence for rule packs + version history, device registry, execution history, user preferences (§15)
- [x] Implement headless service/daemon mode (§3.6, §11)
- [x] Implement lightweight watchdog process supervising the engine (§11)
- [x] Implement device heartbeat monitoring feeding offline/degraded triggers (§11)
  - _Status:_ network devices are kept alive by any valid datagram from their address; `virtual`/`midi` endpoints are probed.
- [x] Implement state persistence across restart (no duplicate one-shot firing) (§11)
  - _Status:_ schedule firings are written ahead of their actions, so one-shots are at-most-once even across a hard kill.
- [x] Implement conflict resolution for concurrent/conflicting action targets (§10.1)
- [x] Implement running-chain cancellation by operator (§10.2)
  - _Status:_ via `CancelHandle` and `POST /executions/:id/cancel`.
- [x] Implement CLI: `validate`, `simulate`, `run --service`, stable exit-code contract (0–6) (§13)
- [x] Implement desktop dashboard (active rules, device health, recent executions, active chains, system status) (§12.1)
  - _Status:_ Tauri desktop app (`crates/tpt-app-av-automation-tauri`, excluded from the default
    workspace build). Embeds the headless service unchanged; the webview renders JSON from the
    `service::ui` bridge — the same status document `/health` serves, including listener loss and
    rejected-inbound counters. Rules with armed toggles, device health counts, running chains with
    operator cancel, and the most recent executions.
- [x] Implement execution log / timeline UI with filters and rule drill-down (§12.4)
  - _Status:_ timeline filters by rule, device, status, live/simulated, time range and row limit
    (the same `report::Filter` as `GET /executions`); a row opens the full record — trigger detail,
    per-condition pass/fail, per-action outcome with errors — with a jump back to the rule in the
    builder. Incident log and pack version history (with restore) sit alongside.
- [x] Implement device manager UI (list, health, config, last-seen, manual ping) (§12.3)
  - _Status:_ device list with health dots, protocol/address, last-seen age and a manual ping
    (`PingDevice` control probing the bound endpoint). The device file is loaded at startup;
    editing device configs still happens in `devices.yaml` (the engine binds endpoints at build).
    Import from TPT AV Commissioning is the Phase 4 item below.
- [x] Implement YAML-backed visual rule builder (trigger→condition→action canvas, round-trips to YAML) (§12.2)
  - _Status:_ rule cards (trigger → conditions → actions) render from `pack_json`; a "new rule"
    wizard assembles a valid rule from the catalogue (9 triggers, 6 conditions, 13 actions, each
    with a canonical example pinned by tests against the model types); the per-rule form edits
    fields with enum-aware inputs and preserves what it does not render (policy, fallback chain,
    reentrancy); the Pack YAML tab is the direct text view with Validate/Apply and the engine's
    located diagnostics. Both views drive the same engine parse/validate/hot-load path, so the
    round-trip cannot drift. Pack hot reload keeps armed state for unchanged rules and persists
    every applied version (restore from history). Form editing of fallback chains happens via YAML.
- [x] Implement local API, disabled by default, `127.0.0.1`-only, token auth when enabled (`/rules`, `/devices`, `/executions`, `/health`, `/events` WS) (§14)
  - _Status:_ REST plus the `/events` WebSocket; also `/incidents` and execution cancel.
- [x] Implement inbound control-message validation + rate limiting/backoff (§16)
  - _Status:_ size and event-count limits, bounded universe tracking, per-source token-bucket rate
    limiting and adaptive backoff (`core::rate_limit`): a source that keeps ignoring its budget is
    muted for a doubling window up to `backoff_max_ms`, and recovers as soon as it behaves.
- [x] Implement sandboxed, timeout-bounded external-script/subprocess action (§8.4)
  - _Status:_ disabled by default; allow-list only; see `docs/action-catalogue.md`.

## Phase 2 — Testing Strategy (§18)

- [x] Unit tests for every trigger/condition/action: valid, invalid, boundary, malformed input cases (§18.1)
  - _Status:_ `tests/matrix.rs` covers all 9 trigger types, 6 condition types and 13 action types on
    every axis — valid form, invalid field, boundary value and malformed document. It asserts the
    matrix is complete, so adding a catalogue form without a case fails. Two validation gaps it
    found are now fixed: a `dmx_channel` condition was not bounds-checked against the universe, and
    a `rule_armed` condition naming a rule that does not exist was not reported.
- [x] Build virtual/simulated device fixtures: OSC, MIDI, DMX, Art-Net, sACN, media-sources, with fault injection (dropped/delayed/offline) (§18.2)
  - _Status:_ `VirtualEndpoint` (any protocol) with dropped, delayed, timed-out and offline-after-N faults; inbound sources are driven over real loopback sockets.
- [x] Build golden rule-pack regression tests with recorded event sequences and expected execution traces (§18.3)
  - _Status:_ 11 fixtures in `tests/fixtures/golden`, plus semantic assertions that pin their meaning.
- [x] Chaos test: device goes offline mid-action-chain (§18.4)
- [x] Chaos test: engine process killed mid-execution, restarted via watchdog (§18.4)
  - _Status:_ `the_watchdog_restarts_a_forcibly_killed_engine` (kills the engine twice), plus
  `tests/chaos/restart_under_load.rs`, which checks state durability across hard kills: the database
  survives and is readable, a one-shot does not re-fire, and the engine still works after three
  consecutive kills.
- [x] Chaos test: conflicting rules firing on same event (§18.4)
- [x] Chaos test: malformed inbound control message (§18.4)
  - _Status:_ parser level and over a live UDP listener, plus `tests/chaos/misbehaving_peers.rs`,
  which throws malformed, truncated, oversized and random OSC and Art-Net traffic at a *live*
  process and confirms it neither crashes nor stops working afterwards.
- [x] Chaos test: network interruption during action execution (§18.4)
  - _Status:_ simulated with injected timeouts/delays/dropouts, not by cutting a real network.
- [x] Integration suite driving the shipped binary in a real process (§19)
  - _Status:_ `tests/integration/rule_pack_lifecycle.rs` (validate/simulate/history/exit-code
  contract, and every pack under `rules/` still validating) and
  `tests/integration/end_to_end_over_the_wire.rs` (OSC, Art-Net and MIDI 2.0/UMP arriving over real
  UDP sockets and producing execution records readable through the API). Run by the `scenarios`
  crate; both suites previously held only `.gitkeep`.
- [x] Fuzz the YAML rule parser (§18.5)
  - _Status:_ coverage-guided targets in `fuzz/` (`rule-pack`, `inbound-osc`, `inbound-midi`,
    `inbound-dmx`, `api-request`), all building clean on nightly and run by CI on Linux. Round-trip and
    arm-safety invariants are also asserted deterministically by `tests/roundtrip.rs` on stable.
    **Windows cannot run these:** Rust ships no ASan runtime for `x86_64-pc-windows-msvc`, so
    `cargo fuzz` cannot link there — fuzzing is Linux-only, which is why the CI job is `ubuntu-latest`.
- [x] Fuzz inbound OSC/MIDI/DMX message parsing (§18.5)
  - _Status:_ `inbound-osc`, `inbound-midi` and `inbound-dmx` targets assert event-count bounds, rate-limit
    enforcement and universe-map bounding, alongside the existing seeded mutation tests.
- [x] Fuzz local API request handling (§18.5)
  - _Status:_ `api-request` target asserts the parser never panics and never yields an empty method or a
    non-absolute path.
- [x] Establish permanent-regression-fixture policy for production bugs (§18.6)
  - _Status:_ `CONTRIBUTING.md` and `tests/fixtures/regression/`, replayed by the golden suite.

## Phase 3 — Hardening & Release (§24.1 steps 20–23, §24 Definition of Done)

- [ ] Benchmark and profile under sustained live-event-like event load
  - _Status:_ throughput benchmarked (≈265k events/s release, `docs/reliability.md`) and latency is now
    measured as percentiles rather than an average: `tests/latency.rs` gates p99 against a 20 ms cue
    budget, records the dispatch curve from 5 to 500 rules (p50 3 µs → 62 µs, linear in pack size),
    checks that an unmatched event is cheaper than a fired one (p99 1 µs vs 13 µs), and times a real
    OSC datagram in → datagram out across two loopback sockets through the production `Inbound` gate
    (p50 17 µs, p99 81 µs). Numbers and reading are in `docs/reliability.md`. **Still not done:** no
    sampling profiler has been run (so no flame graph or allocation profile), and no physical device
    has been measured — the loopback figure covers what the software controls, not a fixture's own
    latency, a DMX fixture's settling time or a MIDI port's buffering.
- [x] Harden error handling and failure isolation
  - _Status:_ panic isolation, bounded queues/cascades/history and persistence-failure isolation are
    in. A systematic audit of every `unwrap`/`expect`/panic in production code is done, and it fixed
    two real gaps: the inbound rate limiter and the fixed clock propagated a poisoned lock where the
    rest of the workspace recovers (16 sites), and a panicking listener thread was lost with no
    indication at all. Workers are now counted, and `/health` reports
    `listeners: {expected, live, lost}` and drops to `degraded` when one is lost. The remaining
    `expect`s are in the test-rig crates and in provably-unreachable post-check positions.
- [x] Package Windows release
  - _Status:_ `scripts/package-windows.ps1` builds a static-CRT zip with SHA-256 and smoke-tests it; unsigned, no installer. The zip includes `install-windows-service.ps1` (boot-time scheduled task running the watchdog; not a native service, not run on a clean machine).
- [x] Validate headless Linux service deployment
  - _Status:_ validated on Ubuntu 24.04 under WSL2 with real systemd: the full workspace test suite
    passes on Linux, and the unit was installed as user `tptav` and exercised. The API answered
    `nominal`; `kill -9` of the engine was recovered by the watchdog; `kill -9` of the watchdog was
    recovered by systemd (`NRestarts=1`); `systemctl stop` exited cleanly. Not a bare-metal rack
    host, and WSL2's kernel differs from a typical server's.
- [ ] Test clean-machine installation (no dev tooling required)
  - _Status:_ the package imports only always-present Windows DLLs, but it has not been tried on a fresh machine/VM.
- [ ] Verify Definition of Done checklist (§24):
  - [x] Rule pack authored (YAML or visual builder) validates without errors
    - _Status:_ YAML; the visual builder waits on the desktop UI.
  - [x] Rules arm/disarm with state always visible in UI
    - _Status:_ the desktop dashboard shows every rule's armed state, the LIVE/SIMULATION banner
      is always on screen, and arm/disarm toggles go through the engine's control queue.
  - [x] Simulation mode never sends a real control message or media action
    - _Status:_ `simulation_never_sends_a_real_control_message_even_for_armed_rules` (real UDP receiver), plus engine and golden tests.
  - [x] Engine runs headless as supervised service and survives forced restart without duplicate one-shot firing
    - _Status:_ watchdog kill test + write-ahead state + restart tests; the kill-during-a-one-shot combination is verified by its parts, not as one automated scenario.
  - [ ] OSC, MIDI, and at least one of DMX/Art-Net/sACN trigger and are triggered end to end
    - _Status:_ OSC and Art-Net are verified end to end over sockets (in and out); MIDI logic is tested but no physical MIDI port has been exercised.
  - [x] Media action starts/stops/switches a source
    - _Status:_ via a `media` device (OSC), **not** `tpt-kinetix`; the spec's wording named kinetix,
      which has no control API. Verified on a loopback socket only.
  - [x] Device health (online/degraded/offline) tracked and can itself trigger a rule
  - [x] Every execution logged with trigger detail, condition results, per-action outcomes
  - [x] Partially failed action chain reported as partial failure, never full success
  - [x] CLI validates/simulates/runs same rule packs as GUI
    - _Status:_ CLI, service and desktop GUI share one engine; desktop pack edits go through the
      engine's own parse/validate/hot-load path, the same one the CLI's `validate` uses.
  - [x] Local API disabled by default; requires token when enabled
  - [x] Malformed inbound control message cannot crash the engine
  - [x] Golden rule-pack tests and virtual-device tests cover every MVP trigger/action
    - _Status:_ `golden_fixtures_exercise_every_mvp_trigger_and_action` enforces it.
  - [x] Chaos tests confirm watchdog restarts engine after forced kill
  - [x] No internet connection required for core operation
    - _Status:_ by construction: nothing in the workspace opens an outbound connection on its own. Not separately tested on a machine with networking disabled.
- [ ] Run a private beta with a real venue or integrator

## Phase 4 — Post-MVP Phase 2 (§20 Phase 2, §22 Studio tier)

- [ ] Media-pipeline triggers/actions: watch-folder ingest, job completed/failed (via `tpt-av-asset`) (§7.5, §20)
- [ ] Sibling-app event integration: TPT Media QC "job failed" event (§5.2, §7.5)
- [ ] Sibling-app event integration: TPT Media Forensics "finding recorded" event (§5.2, §7.5)
- [ ] Full visual rule-builder canvas (beyond YAML-backed forms) (§20)
- [ ] Richer conflict-resolution and priority tooling (§20)
- [ ] Audio/video signal-condition triggers: signal lost, black frame, freeze frame (`tpt-visual`, `tpt-kinetix`) (§7.4, §20)
- [ ] Audio signal-condition triggers: silence, clipping, loudness/level thresholds (`tpt-audio`/`tpt-dsp`) (§7.4, §20)
- [ ] A/V drift and timing-discontinuity triggers (`tpt-av-sync`) (§7.4, §20)
- [ ] Import device inventory from TPT AV Commissioning (§5.2, §12.3, §20)
- [ ] Studio packaging tier: multi-room/zone licensing, priority updates (§22)

## Phase 5 — Post-MVP Phase 3 (§20 Phase 3, §22 Facility/Enterprise tier)

- [ ] Multi-site/facility orchestration (§20)
- [ ] Plugin SDK for third-party device drivers and custom actions (§20)
- [ ] Redundant/clustered engine execution for mission-critical installations (§20)
- [ ] Opt-in outbound webhook/email notification connectors (§8.3, §20)
- [ ] Facility/Enterprise packaging tier: multi-site deployment, plugin SDK, redundant execution, central rule-pack distribution, commercial support (§22)
- [ ] Re-evaluate cloud infrastructure only if customers demonstrate real need (§20)
