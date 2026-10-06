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

The package ships `install-windows-service.ps1`, which registers that command as a Task Scheduler
task that starts at boot as SYSTEM, whether or not anyone is logged on, and is retried every minute
if it exits (run from an elevated PowerShell; `-Uninstall` removes it):

```powershell
./install-windows-service.ps1 -Rules C:\show\main-hall.yaml -Devices C:\show\devices.yaml
```

This is a scheduled task, not a native Windows service: it does not appear in `services.msc`.
**Not yet exercised on a clean machine.** The executable and scripts are unsigned, so SmartScreen
will warn on first run (`Unblock-File` or "Run anyway").

## Linux (headless, rack-mounted)

Build on Linux with `cargo build --release -p tpt-app-av-automation-cli` (the `tpt-av-control`
repository must be checked out beside this one), then use
[`scripts/tpt-av-automation.service`](../scripts/tpt-av-automation.service), a hardened systemd unit
that runs the watchdog, which runs the engine.

**Status:** validated on Ubuntu 24.04 under WSL2 with real systemd (full test suite on Linux; kill
engine -> watchdog restart; kill watchdog -> systemd restart; clean `systemctl stop`). Not yet run on
a bare-metal host.

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
