# Starter templates

Each template is a pair of files: a rule pack (`name.yaml`) and the devices it uses
(`name.devices.yaml`). Every rule starts **disarmed**, so nothing is ever sent until you arm it.

Try one without any hardware:

```sh
# 1. Check it
tpt-av-automation validate --rules rules/templates/classroom.yaml \
                           --devices rules/templates/classroom.devices.yaml

# 2. See what it would do. Nothing is sent in a simulation.
tpt-av-automation simulate --rules rules/templates/classroom.yaml \
                           --devices rules/templates/classroom.devices.yaml \
                           --event schedule:07:45
```

To use one for real, copy both files, change the device addresses in the devices file to match your
equipment (or set `protocol: virtual` on a device to rehearse without it), and load them in the app
or pass them to `run --service --rules ... --devices ...`.

| Template | Use it for | Starts with |
|----------|-----------|-------------|
| `classroom` | Power a classroom up and down on school days, with a manual "start lesson" button | schedule |
| `meeting-room` | A control panel's join/leave cue wakes the display and sets the lights | OSC |
| `worship` | Sunday service start and wind-down with an operator notice | schedule |
| `theatre` | A cue stack on MIDI notes recalling lighting presets, plus a priority blackout | MIDI |
| `museum` | Unattended open/close of a gallery with a media loop, escalating if a step fails | schedule |
| `exterior-lights` | Facade lights that follow sunset and sunrise at your venue's location | solar schedule |
| `failover` | Switch to a backup input when the main display drops, and back when it returns | device health |

Notes:

- `theatre` needs a MIDI listener in the service settings (`listeners: [{ protocol: midi, bind:
  "Your MIDI Port" }]`).
- `failover` reacts to device health. Network devices count as online when they send traffic to a
  listener, so see the health section of [`docs/devices.md`](../../docs/devices.md).
- `exterior-lights` needs your venue's coordinates in the pack's `site:` block.
