# TPT AV Automation

Offline-first automation for professional AV, live production and media-pipeline workflows.

> **WHEN** something happens in your AV/media environment, **IF** the right conditions hold,
> **THEN** run a defined, auditable sequence of actions — reliably, offline, without a programmer
> in the room.

Rules live in human-readable, versioned YAML *rule packs*. Every rule can be **simulated** before it
is **armed**; every execution records *why* it fired, what each condition said, and what each action
did; a partially failed chain is never reported as a success.

Publisher: TPT Solutions · License: MIT OR Apache-2.0 · Specification: [`spec.txt`](spec.txt) ·
Work tracker: [`todo.md`](todo.md)

## Status

This repository contains the engine, CLI, headless service and the desktop application. **Not built
yet:** media actions through `tpt-kinetix`/`tpt-cadence` (media actions go out as OSC instead), and Phase 2/3 features. See
[`todo.md`](todo.md) for the exact state of every item.

| Area | State |
|------|-------|
| Rule model, YAML format, validation | done |
| Deterministic engine, simulation, arm/disarm, conflicts, cancellation | done |
| Triggers: schedule, OSC, MIDI 1.0/2.0, DMX/Art-Net/sACN, device health, heartbeat, manual/API | done |
| Actions: OSC, MIDI, DMX/Art-Net/sACN/serial DMX512, notify, incident log, wait, invoke rule, sandboxed exec | done |
| MIDI ports (live in/out by name via `midir`) | implemented, **not tested against hardware** |
| DMX512 serial output (`dmx512` devices, `serialport`) | implemented, **not tested against a physical adapter** |
| Media actions (`media.video_source`, `media.audio_route`) | sent as OSC by a `media` device (templated addresses); **not tested against a real media server**, no `tpt-kinetix` backend |
| CLI: `validate`, `simulate`, `run`, `watchdog`, `history` | done |
| Headless service, SQLite persistence, restart-safe one-shots, watchdog | done |
| Local API (REST + WebSocket), disabled by default, token + loopback only | done |
| Desktop UI: dashboard, rule builder, device manager, timeline | done (see below) |

## Quick start

```sh
cargo build --release -p tpt-app-av-automation-cli      # produces target/release/tpt-av-automation

# 1. Check a pack (and that every device it names exists)
tpt-av-automation validate --rules rules/live-event/main-hall-event-start.yaml \
                           --devices rules/examples/devices.yaml

# 2. See what it would do. Nothing is ever sent in a simulation.
tpt-av-automation simulate --rules rules/live-event/main-hall-event-start.yaml \
                           --event schedule:18:55 --health projector-1=online

# 3. Run it, headless, with state that survives restarts
tpt-av-automation run --service --rules main-hall.yaml --devices devices.yaml \
                      --state-dir ./state

# 4. ...supervised: restart the engine if it is ever killed
tpt-av-automation watchdog -- run --service --rules main-hall.yaml --devices devices.yaml \
                                  --state-dir ./state
```

Rules load **disarmed** unless the YAML says `armed: true`; a disarmed rule is evaluated and logged
but never sends anything.

### Desktop app

The desktop shell embeds the same engine: dashboard with armed states and device health, a visual
rule builder that round-trips to the pack YAML, a device manager with manual pings, and the
execution timeline with filters. The LIVE / SIMULATION banner is always on screen.

```sh
cargo build --manifest-path crates/tpt-app-av-automation-tauri/Cargo.toml   # not in the default workspace
cargo run --manifest-path crates/tpt-app-av-automation-tauri/Cargo.toml -- rules/main-hall.yaml
```

A settings file (`tpt-av-automation.yaml`) can name `pack`, `devices` and a `service:` block
(listeners, API, state database); run with no arguments for a fresh "new show" setup. The desktop
crate is excluded from the default workspace because it needs the Tauri/WebView2 toolchain.

### Exit codes (stable)

| Code | Meaning |
|------|---------|
| 0 | success |
| 1 | partial failure |
| 2 | failed |
| 3 | skipped (nothing ran) |
| 4 | configuration error |
| 5 | device error |
| 6 | internal error |

## Repository layout

| Crate | Responsibility |
|-------|----------------|
| `core` | ids, errors, injectable clock, normalized `Event`, rate limiter |
| `model` | rule pack data model, YAML format, validation — no I/O |
| `triggers` | schedule driver, inbound OSC/MIDI/Art-Net/sACN validation |
| `conditions` | three-valued condition evaluation |
| `actions` | action execution, simulation, sandboxed subprocess |
| `devices` | registry, heartbeat health, endpoints (virtual, UDP, MIDI) |
| `engine` | deterministic evaluation, conflict resolution, cancellation |
| `report` | human/JSON rendering, exit codes, log filters |
| `service` | SQLite store, listeners, local API, watchdog, the desktop UI's command bridge |
| `cli` | the `tpt-av-automation` binary |
| `test` | shared fixtures for golden and chaos tests |
| `tauri` | the desktop app (own manifest; embeds the service) |

Design documents are in [`docs/`](docs/); deployment notes (Windows package, systemd unit) are in
[`docs/deployment.md`](docs/deployment.md).

## Offline by design

Nothing in this repository opens an outbound connection on its own. There is no telemetry, no
account, no licence check, no cloud relay. The local API is off unless enabled, binds only to
loopback, and requires a token.

## Licensing

Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option. The open TPT
foundation crates (`tpt-av-control`, `tpt-kinetix`, …) are separate repositories; product-specific
rule logic stays here.
