# tpt-app-av-automation-devices

Device registry, health monitoring and outbound endpoints for
[TPT AV Automation](../../README.md).

## Registry and health

`DeviceRegistry` tracks every configured device (`Device`, `DeviceKind`, `ProtocolBinding`), its
health and its last heartbeat. Health transitions become normalized `Event`s, so **device health is
itself a trigger source** (e.g. fail over to a backup projector when the primary goes offline).
A device we have never heard from is `Unknown`, not `Offline`.

## Endpoints

`Endpoint` is the outbound trait. Implementations:

| Endpoint | Protocol value | Notes |
|----------|----------------|-------|
| `UdpEndpoint` | `osc`, `artnet`, `sacn` | Uses the `tpt-av-control` codecs. |
| `MidiEndpoint` | `midi` | MIDI 1.0 encoding; live ports by name via `midir` with lazy reconnection. |
| `UmpEndpoint` | `ump` | MIDI 2.0 Universal MIDI Packets, including all ten MIDI 2.0-only message kinds. A MIDI 1.0 port rejects those fields rather than truncating them. |
| `DmxSerialEndpoint` | `dmx512` | DMX512-A frames (break/MAB timing, 250 kbaud) to a serial adapter via `serialport`. |
| `MediaEndpoint` | `media` | `media.video_source` / `media.audio_route` sent as OSC at per-device address templates. |
| `VirtualEndpoint` | `virtual` | Records every command; supports fault injection (dropped messages, going offline mid-chain). |

`build_endpoint(&DeviceConfig)` constructs the right one from a device file entry.

## Device files

`DeviceFile` / `DeviceConfig` / `SceneDef` define the `devices.yaml` format. Loading is strict:
unknown keys are errors, ids and `host:port` addresses are checked, heartbeat intervals are range
checked. See [docs/devices.md](../../docs/devices.md) for the format and per-protocol addresses.

## Hardware status

MIDI ports and the DMX512 serial output are implemented against traits (`MidiWriter`,
`DmxSerialPort`) and are covered by tests with fakes, but have **not been tested against physical
hardware**. Use `VirtualEndpoint` for rehearsal and CI.

## Example

```rust,ignore
use tpt_app_av_automation_devices::{build_endpoint, DeviceFile};

let file: DeviceFile = serde_yaml::from_str(&std::fs::read_to_string("devices.yaml")?)?;
let diagnostics = file.validate();
for cfg in &file.devices {
    let endpoint = build_endpoint(cfg)?;
}
```

## Licence

Dual-licensed under [MIT](../../LICENSE-MIT) or [Apache-2.0](../../LICENSE-APACHE), at your option.
