# tpt-app-av-automation-service

Headless service mode for [TPT AV Automation](../../README.md): persistence, inbound listeners,
heartbeat monitoring, the local API and the watchdog. It wraps the deterministic `engine` in
everything needed to run unattended for a whole show season.

## Components

| Module | What it does |
|--------|--------------|
| `runtime` | `Service` / `ServiceHandle`: owns the engine thread, the UDP/MIDI listeners, the tick loop and heartbeat checks. `Snapshot`, `RuleInfo`, `DeviceInfo` and `InboundStats` describe live state. |
| `store` | `Store`: SQLite persistence (bundled `rusqlite`) for execution history, the incident log, pack version history (with restore) and scheduler state, so one-shots are restart-safe. |
| `api` | Local REST API and a WebSocket event stream. |
| `watchdog` | `supervise`: restarts a child engine process that exits unexpectedly, with a `WatchdogPolicy` (restart budget, backoff). |
| `config` | `ServiceConfig`, `ListenerConfig`, `ApiConfig`: the service settings file. Unknown keys are errors. |
| `templates` | Built-in starter packs (classroom, meeting room, worship, theatre, museum, exterior lights, failover). |
| `ui` | `UiBridge`: the command surface shared by the desktop app and the API (dashboard, rule editing, arm/disarm, device manager, history, templates). |

## Local API

Disabled by default. When enabled it binds **loopback only** and requires a bearer token.

| Method | Path | Returns |
|--------|------|---------|
| GET | `/health` | status, mode, rule/device counts, rejected-traffic counters, listener liveness |
| GET | `/rules` | id, name, version, armed, priority, trigger, tags |
| GET | `/devices` | id, protocol, health, last seen |
| GET | `/executions` | filtered execution history (`rule`, `status`, `device`, `simulated`, `since`, `until`, `limit`) |
| GET | `/incidents` | the persistent incident log, newest first |
| POST | `/rules/:id/arm`, `/disarm`, `/run` | arm, disarm or manually run a rule |
| POST | `/executions/:id/cancel` | cancel a running chain |
| GET | `/events` | WebSocket: one JSON execution record per message |

Full details: [docs/integrations.md](../../docs/integrations.md).

## Reliability

- The watchdog restarts a crashed engine; the state database means schedules, armed set and history
  survive the restart.
- Listener worker threads are tracked (`expected` / `live` / `lost`) and surfaced in `/health`.
- Inbound traffic is rate limited per source; rejected packets are counted, not acted on.
- Sandboxed external programs stay off unless `exec_allow` lists them.

See [docs/reliability.md](../../docs/reliability.md) and [docs/deployment.md](../../docs/deployment.md).

## Example

```rust,ignore
use std::sync::Arc;
use tpt_app_av_automation_core::SystemClock;
use tpt_app_av_automation_service::{Service, ServiceConfig, Store};

let config: ServiceConfig = serde_yaml::from_str(&std::fs::read_to_string("service.yaml")?)?;
let store = Store::open("state/automation.db")?;
// `build` rejects pack, device-file and config errors up front.
let service = Service::build(pack, devices, config, Some(store), Arc::new(SystemClock))?;
let handle = service.handle();   // cloneable; used by the API and the desktop app
service.run()?;                  // blocks until stopped
```

## Licence

Dual-licensed under [MIT](../../LICENSE-MIT) or [Apache-2.0](../../LICENSE-APACHE), at your option.
