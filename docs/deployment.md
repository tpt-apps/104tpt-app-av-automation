# Deployment

## Windows

```powershell
./scripts/package-windows.ps1 -Version 0.1.0
```

builds `dist/tpt-av-automation-<version>-windows-x64.zip` (+ `.sha256`). The executable is linked with
a static C runtime, so it needs no Visual C++ redistributable and no development tooling. The script
smoke-tests the packaged executable from an empty directory (`--version`, `validate`, `simulate`)
before writing the archive.

**Status:** the packaging script has been run and the packaged executable smoke-tested on the
development machine. A *clean-machine* installation (a fresh Windows install or VM with no Rust/MSVC)
has **not** been tested yet — that is an open item in `todo.md`.

Run it as a supervised service:

```powershell
tpt-av-automation watchdog -- run --service --rules main-hall.yaml --devices devices.yaml `
    --config service.yaml --state-dir C:\ProgramData\TptAvAutomation
```

Run that under the Windows Task Scheduler ("At startup", "Run whether user is logged on or not") or a
service wrapper of your choice; a native Windows-service integration is not built yet.

## Linux (headless, rack-mounted)

Build on Linux with `cargo build --release -p tpt-app-av-automation-cli` (the `tpt-av-control`
repository must be checked out beside this one), then use
[`scripts/tpt-av-automation.service`](../scripts/tpt-av-automation.service), a hardened systemd unit
that runs the watchdog, which runs the engine.

**Status:** the unit file and the code paths it relies on (the watchdog, `--state-dir`, signal
handling through `ctrlc` with the `termination` feature so `SIGTERM` stops the engine cleanly) are
written, but **not validated on a Linux host** — the development machine is Windows. The unit-test and
integration suites are written to be portable (no Windows-only APIs outside `cfg(windows)` helpers), but
have only been run on Windows. Treat Linux deployment as unverified until that item in `todo.md` is
closed.

## Files and ports

| Path / port | Purpose |
|-------------|---------|
| `--state-dir` | `automation.db` (SQLite, WAL), `engine.pid` |
| listener ports from `service.yaml` (e.g. UDP 9000, 6454, 5568) | inbound OSC / Art-Net / sACN |
| `127.0.0.1:8787` (only when `api.enabled`) | local API |

The engine makes **no** outbound connection on its own; firewall rules only need to allow the inbound
UDP ports you configure and the devices' own ports for outbound control traffic.

## Upgrading

Packs carry `format_version`; a newer pack than the binary understands is refused at load. The
database records its schema version and refuses a mismatch rather than guessing. Back up
`automation.db` (and its `-wal`/`-shm` files, or stop the service first) before upgrading.
