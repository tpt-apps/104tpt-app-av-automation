# TPT AV Automation — Project Todo

Source of truth: `spec.txt`. Section references (`§`) point back to it.

## Notes (not tasks)

- Product: TPT AV Automation — offline-first automation/orchestration for professional AV, live production, and media-pipeline workflows (§1).
- Publisher: TPT Solutions. License: dual MIT / Apache-2.0.
- Pricing hypothesis (§2.1, to validate): Standalone $999, Studio $2,499, Facility/Enterprise $4,999+. Perpetual licence, no mandatory recurring cloud costs.
- Competitive differentiation (§2.2): native Rust, offline-first, unified live-control + media-pipeline rule engine, deterministic/versioned rules, simulation/dry-run, reliability as a feature, vendor-neutral, shared TPT foundation.
- Strategic boundary (§23, §25): do NOT become a lighting console, DAW, video switcher, home-automation hub, or full BMS. Do not try to match Crestron/Q-SYS feature-for-feature in v1.

---

## Phase 0 — Repository & Licensing Setup (§4, §17)

- [ ] Initialize git repository and `.gitignore`
- [ ] Create `LICENSE-MIT` (copyright holder: TPT Solutions)
- [ ] Create `LICENSE-APACHE` (Apache-2.0, copyright holder: TPT Solutions)
- [ ] Set `license = "MIT OR Apache-2.0"` in workspace `Cargo.toml`
- [ ] Scaffold repo structure per §4: `crates/`, `rules/{examples,live-event,media-pipeline,failover}/`, `tests/{fixtures,virtual-devices,integration,chaos}/`, `docs/`
- [ ] Add root `README.md`, `CHANGELOG.md`, `CONTRIBUTING.md`
- [ ] Stub `docs/architecture.md`
- [ ] Stub `docs/rule-model.md`
- [ ] Stub `docs/trigger-catalogue.md`
- [ ] Stub `docs/action-catalogue.md`
- [ ] Stub `docs/rule-format.md`
- [ ] Stub `docs/reliability.md`
- [ ] Stub `docs/integrations.md`
- [ ] Confirm commercial/open-source boundary (§17): `tpt-app-av-automation` (commercial) stays separate from open TPT foundation crates (`tpt-av-control`, `tpt-kinetix`, `tpt-cadence`, `tpt-audio`, `tpt-dsp`, `tpt-visual`, `tpt-av-sync`, `tpt-av-asset`, `tpt-av-test`)

## Phase 1 — MVP Build (§19, §24.1 steps 1–15)

- [ ] Establish Cargo workspace and application shell (crates listed in §4: core, model, triggers, conditions, actions, engine, devices, report, cli, service, tauri, test)
- [ ] Integrate `tpt-av-control` and enumerate supported protocols (§5.1)
- [ ] Implement the device/endpoint registry (§6.5)
- [ ] Implement the rule/trigger/condition/action domain model (§6.1–§6.4, §6.6)
- [ ] Implement the YAML rule-pack parser and validator, with versioning (§9)
- [ ] Implement the deterministic evaluation core with virtual-device test support (§3.2, §10)
- [ ] Implement simulation mode end to end — armed/disarmed default-disarmed behaviour (§3.5, §12.5)
- [ ] Implement OSC inbound/outbound trigger + action (§7.2, §8.1)
- [ ] Implement MIDI (1.0/2.0) inbound/outbound trigger + action (§7.2, §8.1)
- [ ] Implement DMX/Art-Net/sACN channel-change trigger + output action (§7.2, §8.1)
- [ ] Implement schedule trigger (cron-like, interval, one-shot) (§7.1)
- [ ] Implement device online/offline/degraded trigger + heartbeat-missed trigger (§7.3, §11)
- [ ] Implement manual/API-invoked trigger (§7.6)
- [ ] Implement notify-operator action (§8.3)
- [ ] Implement log-incident action (§8.3)
- [ ] Integrate `tpt-kinetix` for media start/stop/switch actions (§5.1, §8.2)
- [ ] Implement SQLite persistence for rule packs + version history, device registry, execution history, user preferences (§15)
- [ ] Implement headless service/daemon mode (§3.6, §11)
- [ ] Implement lightweight watchdog process supervising the engine (§11)
- [ ] Implement device heartbeat monitoring feeding offline/degraded triggers (§11)
- [ ] Implement state persistence across restart (no duplicate one-shot firing) (§11)
- [ ] Implement conflict resolution for concurrent/conflicting action targets (§10.1)
- [ ] Implement running-chain cancellation by operator (§10.2)
- [ ] Implement CLI: `validate`, `simulate`, `run --service`, stable exit-code contract (0–6) (§13)
- [ ] Implement desktop dashboard (active rules, device health, recent executions, active chains, system status) (§12.1)
- [ ] Implement execution log / timeline UI with filters and rule drill-down (§12.4)
- [ ] Implement device manager UI (list, health, config, last-seen, manual ping) (§12.3)
- [ ] Implement YAML-backed visual rule builder (trigger→condition→action canvas, round-trips to YAML) (§12.2)
- [ ] Implement local API, disabled by default, `127.0.0.1`-only, token auth when enabled (`/rules`, `/devices`, `/executions`, `/health`, `/events` WS) (§14)
- [ ] Implement inbound control-message validation + rate limiting/backoff (§16)
- [ ] Implement sandboxed, timeout-bounded external-script/subprocess action (§8.4)

## Phase 2 — Testing Strategy (§18)

- [ ] Unit tests for every trigger/condition/action: valid, invalid, boundary, malformed input cases (§18.1)
- [ ] Build virtual/simulated device fixtures: OSC, MIDI, DMX, Art-Net, sACN, media-sources, with fault injection (dropped/delayed/offline) (§18.2)
- [ ] Build golden rule-pack regression tests with recorded event sequences and expected execution traces (§18.3)
- [ ] Chaos test: device goes offline mid-action-chain (§18.4)
- [ ] Chaos test: engine process killed mid-execution, restarted via watchdog (§18.4)
- [ ] Chaos test: conflicting rules firing on same event (§18.4)
- [ ] Chaos test: malformed inbound control message (§18.4)
- [ ] Chaos test: network interruption during action execution (§18.4)
- [ ] Fuzz the YAML rule parser (§18.5)
- [ ] Fuzz inbound OSC/MIDI/DMX message parsing (§18.5)
- [ ] Fuzz local API request handling (§18.5)
- [ ] Establish permanent-regression-fixture policy for production bugs (§18.6)

## Phase 3 — Hardening & Release (§24.1 steps 20–23, §24 Definition of Done)

- [ ] Benchmark and profile under sustained live-event-like event load
- [ ] Harden error handling and failure isolation
- [ ] Package Windows release
- [ ] Validate headless Linux service deployment
- [ ] Test clean-machine installation (no dev tooling required)
- [ ] Verify Definition of Done checklist (§24):
  - [ ] Rule pack authored (YAML or visual builder) validates without errors
  - [ ] Rules arm/disarm with state always visible in UI
  - [ ] Simulation mode never sends a real control message or media action
  - [ ] Engine runs headless as supervised service and survives forced restart without duplicate one-shot firing
  - [ ] OSC, MIDI, and at least one of DMX/Art-Net/sACN trigger and are triggered end to end
  - [ ] Media action starts/stops/switches a source via `tpt-kinetix`
  - [ ] Device health (online/degraded/offline) tracked and can itself trigger a rule
  - [ ] Every execution logged with trigger detail, condition results, per-action outcomes
  - [ ] Partially failed action chain reported as partial failure, never full success
  - [ ] CLI validates/simulates/runs same rule packs as GUI
  - [ ] Local API disabled by default; requires token when enabled
  - [ ] Malformed inbound control message cannot crash the engine
  - [ ] Golden rule-pack tests and virtual-device tests cover every MVP trigger/action
  - [ ] Chaos tests confirm watchdog restarts engine after forced kill
  - [ ] No internet connection required for core operation
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
