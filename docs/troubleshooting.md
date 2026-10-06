# Troubleshooting

## A rule did not run

Every execution records why it fired or did not. In the app, open the **Timeline** and look at the
rule's row; on the command line use `simulate` to replay the same event:

```sh
tpt-av-automation simulate --rules pack.yaml --devices devices.yaml --event schedule:18:55
```

Common causes, most likely first:

| Symptom | Cause | Fix |
|---------|-------|-----|
| The rule never appears in the Timeline | The rule is **disarmed**. Rules load disarmed on purpose | Arm it in the Rules tab, or set `armed: true` |
| Timeline says *skipped*, a condition shows *unknown* | A `device_health` condition on a device that has never reported (see below) | Make the device report, or add `fire_on_unknown: true` to the condition |
| Timeline says *skipped*, a condition shows *not met* | The condition really was false (time window, health, DMX level, ...) | Check the condition's explanation in the record |
| Nothing is sent but the execution says *success* | The app is in **SIMULATION** mode (the banner is always shown) | Switch to LIVE in the header |
| A schedule never fires | Wrong `utc_offset_minutes`, or a `solar` schedule without a `site:` | Set the offset in the service settings; add `site:` latitude and longitude to the pack |
| An OSC / MIDI / Art-Net trigger never fires | No listener is bound for that protocol | Add a `listeners:` entry in the service settings, e.g. `{ protocol: osc, bind: "0.0.0.0:9000" }` |
| A one-shot schedule fires again after a restart | The engine was started without `--state-dir`, so nothing was persisted | Always pass `--state-dir` to `run` |

## Device health stays "unknown"

Network devices (`osc`, `artnet`, `sacn`, `ump`) use connectionless UDP, so the engine cannot ask
them whether they are alive. A device counts as **online** once it has sent traffic to one of the
service's listeners, and stays **unknown** until then. Only `virtual` and `midi` devices are probed
on a timer.

- If a device can send to the engine (many do, for status or heartbeat messages), add a listener for
  its protocol and make sure it targets the machine running the engine.
- If it cannot, do not gate rules on `device_health` for that device.
- **Ping** on the Devices tab confirms the endpoint could be opened. For UDP devices that does not
  prove the device is listening.

## A device was rejected when saving

The message names the field. The usual ones:

- `address must look like host:port` — network protocols need both, e.g. `192.168.1.50:9000`.
- `device id may only contain letters, digits, - _ and .` — spaces and other symbols are not allowed.
- `still uses device` on delete — a rule targets it. Change or remove those rules first.
- `unknown field` — a key is misspelled. The accepted fields are listed in
  [`devices.md`](devices.md).

## The app starts empty / my changes are gone

The desktop app keeps its show beside its state database (on Windows,
`%APPDATA%\tpt-av-automation\`):

| File | Holds |
|------|-------|
| `pack.yaml` | your rules, rewritten after every change |
| `devices.yaml` | your devices, rewritten after every change |
| `state.db` | execution history, armed state and every saved version of the pack |

The first time each file is rewritten in a session the previous version is kept as `*.bak`. If a
settings file names a `pack:` or `devices:` path, that file is used instead. To roll back, restore
an earlier version from the pack history, or copy the `.bak` over the file while the app is closed.

## Windows warns when running the installer

The installer is not code-signed, so SmartScreen shows "Windows protected your PC". Choose
**More info**, then **Run anyway**. You can compare the download against the SHA-256 checksum
published with it.

## MIDI, serial DMX and media servers

These integrations are implemented but have not been tested against every piece of real hardware.
Test your own rig before relying on it for a paid show:

- Run `simulate` first to confirm what would be sent.
- Use a `virtual` device in place of the real one to rehearse the sequence.
- A `midi` device's `address` is matched as a case-insensitive *part of the port name*. If no port
  matches, the error is "no MIDI output port matching ...": check the exact name your operating
  system shows for the interface.

## Getting more detail

Set `RUST_LOG=debug` before starting the engine for detailed logs, and use
`tpt-av-automation history --state-dir <dir>` to list past executions. Exit codes are stable:
0 success, 1 partial failure, 2 failed, 3 skipped, 4 configuration error, 5 device error,
6 internal error.
