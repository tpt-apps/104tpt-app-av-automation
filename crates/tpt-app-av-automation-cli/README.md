# tpt-app-av-automation-cli

The `tpt-av-automation` command-line tool for [TPT AV Automation](../../README.md): validate,
simulate and run rule packs. It is a thin shell over the same engine the desktop application uses.

## Install

```sh
cargo install --path crates/tpt-app-av-automation-cli      # installs `tpt-av-automation`
# or, from a checkout:
cargo build --release -p tpt-app-av-automation-cli          # target/release/tpt-av-automation
```

## Commands

| Command | Purpose |
|---------|---------|
| `init <dir>` | Create a ready-to-run show folder (pack, devices on virtual endpoints, service settings) from a built-in template. `init --list` shows them. |
| `devices` | List the devices in a device file, check them and optionally test that each can be opened. |
| `validate` | Check a rule pack, and optionally its device file and service config, without running anything. |
| `simulate` | Evaluate events against a pack and show what would happen. **Sends nothing, ever.** |
| `run` | Run the engine; with `--service` it runs headless until stopped, with persistence under `--state-dir`. |
| `watchdog` | Supervise `run` (or any engine command) and restart it if it exits unexpectedly. |
| `history` | Show recorded executions from the state database. |

```sh
tpt-av-automation init my-show
tpt-av-automation validate --rules pack.yaml --devices devices.yaml
tpt-av-automation simulate --rules pack.yaml --devices devices.yaml --event schedule:18:55
tpt-av-automation run --service --rules pack.yaml --devices devices.yaml --state-dir ./state
tpt-av-automation watchdog -- run --service --rules pack.yaml --devices devices.yaml --state-dir ./state
```

Rules load **disarmed**, so nothing is sent until you arm one. `--format json` produces one
machine-readable result per execution.

## Exit codes (stable)

| Code | Meaning |
|-----:|---------|
| 0 | Success |
| 1 | Partial failure |
| 2 | Failed |
| 3 | Skipped (nothing matched, or a condition was not met) |
| 4 | Configuration error |
| 5 | Device error |
| 6 | Internal error |

## Related

- [docs/rule-format.md](../../docs/rule-format.md), [docs/devices.md](../../docs/devices.md)
- [docs/deployment.md](../../docs/deployment.md) for running as a service at boot.
- [docs/troubleshooting.md](../../docs/troubleshooting.md)

## Licence

Dual-licensed under [MIT](../../LICENSE-MIT) or [Apache-2.0](../../LICENSE-APACHE), at your option.
