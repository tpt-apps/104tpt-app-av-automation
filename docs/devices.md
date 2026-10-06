# Devices

A **device** is anything a rule controls or watches: a projector, a lighting node, an audio matrix,
a media server, a MIDI interface. Rules refer to a device by its `id`, so a rule pack can be moved
between venues and only the device list changes.

You can manage devices two ways. Both produce the same file.

- **In the desktop app:** open the **Devices** tab, click **+ Add device**, fill in the form and
  save. Use **Edit** and **Delete** on a row to change or remove one. Changes apply immediately, no
  restart, and are written to the devices file straight away.
- **By hand:** write a `devices.yaml` and pass it to the CLI or service with `--devices`.

## Where the file lives

| How you run it | Devices file |
|----------------|--------------|
| Desktop app, settings file names `devices:` | that path |
| Desktop app, nothing configured | `devices.yaml` next to the state database (`%APPDATA%\tpt-av-automation\` on Windows), created when you add the first device |
| CLI / service | the path you give `--devices` |

The first time the desktop app rewrites an existing file in a session it keeps the original as
`devices.yaml.bak`. Rewriting does not preserve hand-written comments, so keep notes somewhere else
or in the `name` field.

## File format

```yaml
devices:
  - id: projector-1          # required; letters, digits, - _ .  (rules refer to this)
    name: Projector 1        # optional display name; defaults to the id
    kind: display            # lighting | display | audio | switcher | media | other
    protocol: osc            # see the table below
    address: 192.168.1.50:9000
    heartbeat_ms: 15000      # optional; 1 to 86,400,000
```

Unknown keys are rejected, so a typo such as `heartbeat_msec` is reported instead of silently
ignored.

### Protocols and what `address` means

| `protocol` | Talks to | `address` |
|------------|----------|-----------|
| `osc` | Anything that accepts OSC over UDP | `host:port` |
| `artnet` | Art-Net lighting nodes | `host:port` (default port 6454) |
| `sacn` | sACN / E1.31 receivers | `host:port` (default port 5568) |
| `ump` | MIDI 2.0 (UMP) gateway over UDP | `host:port` |
| `media` | A media server; video and audio actions are sent as OSC | `host:port` |
| `midi` | A local MIDI 1.0 output port | part of the port name, e.g. `UMC` |
| `dmx512` | A serial DMX512 adapter | serial port, e.g. `COM3` or `/dev/ttyUSB0` |
| `virtual` | Nothing. Commands are recorded in memory | none |

`virtual` devices are the quickest way to try a rule pack with no hardware: every action
"succeeds" and you can see exactly what would have been sent.

## One example per protocol

```yaml
devices:
  - { id: projector-1,  kind: display,  protocol: osc,     address: "192.168.1.50:9000" }
  - { id: lighting-rack, kind: lighting, protocol: artnet,  address: "192.168.1.60:6454" }
  - { id: stage-sacn,   kind: lighting, protocol: sacn,    address: "192.168.1.61:5568" }
  - { id: midi-gateway, kind: other,    protocol: ump,     address: "192.168.1.70:5004" }
  - { id: foh-midi,     kind: audio,    protocol: midi,    address: "UMC" }
  - { id: house-lights, kind: lighting, protocol: dmx512,  address: "COM3" }
  - { id: rehearsal,    kind: other,    protocol: virtual }
  - id: media-server
    kind: media
    protocol: media
    address: "192.168.1.80:9002"
    media:                                   # optional OSC address templates
      video: /media/video/{source}/{operation}
      audio: /media/audio/{from}/{to}/{operation}
```

### DMX scenes

Lighting devices (`artnet`, `sacn`, `dmx512`) can define named scenes that rules recall with
`control.dmx_scene`:

```yaml
  - id: lighting-rack
    kind: lighting
    protocol: artnet
    address: "192.168.1.60:6454"
    scenes:
      house-to-half: { universe: 1, start_channel: 0, values: [128, 128, 128, 128] }
```

A scene must fit inside 512 channels (`start_channel` + number of values). A `dmx512` serial port
carries universe 1 only. In the app, scenes are typed one per line as
`name | universe | start channel | values`.

## What is checked

Saving a device in the app, or running `validate`, rejects:

- an empty, duplicated or oddly-spelled `id`
- an unknown `protocol`
- a missing address, or one that is not `host:port` for the network protocols
- a `heartbeat_ms` of 0 or more than a day
- a scene running past channel 512, or a `dmx512` scene outside universe 1
- `media` templates on a non-`media` device, or templates missing their placeholders

Deleting a device is refused while any rule still uses it. The message lists those rules.

```sh
tpt-av-automation validate --rules my-pack.yaml --devices devices.yaml
```

## Health and heartbeats

Each device has a health state: `online`, `degraded`, `offline` or `unknown`.

- `virtual` and `midi` devices are probed on an interval, so they report health by themselves.
- Network devices (`osc`, `artnet`, `sacn`, `ump`) cannot be probed over a connectionless protocol.
  They count as online when they **send traffic to a listener** the service is bound to. A device
  that has never been heard from stays `unknown`.
- A rule with a condition such as `device_health projector-1 equals online` is therefore skipped
  while the projector is still `unknown`. Either configure a listener the device reports to, or
  leave the condition out. `simulate --health projector-1=online` shows what the rule would do once
  the device is known to be up.
- **Ping** in the Devices tab only proves the endpoint could be opened, not that the device
  answered.
