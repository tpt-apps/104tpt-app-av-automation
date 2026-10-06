# Changelog

All notable changes to `tpt-app-av-automation-devices` are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/) and the crate uses semantic versioning. Workspace-wide
changes are recorded in the [root changelog](../../CHANGELOG.md).

## [Unreleased]

### Fixed / Changed
- A mistyped key in `devices.yaml` was silently ignored; it is now an error.

## [0.1.0]

Initial development release.

### Added
- `DeviceRegistry` with heartbeat monitoring; health transitions become events.
- `UdpEndpoint` for OSC, Art-Net and sACN on the `tpt-av-control` codecs.
- `MidiEndpoint` with live port access by name through `midir` and lazy reconnection.
- `UmpEndpoint` for MIDI 2.0 Universal MIDI Packets, including the ten MIDI 2.0-only kinds.
- `DmxSerialEndpoint` for DMX512-A output to a serial adapter, and a DMX512 serial reader.
- `MediaEndpoint`: `media.video_source` / `media.audio_route` as OSC at templated addresses.
- `VirtualEndpoint` with fault injection.
- `DeviceFile` format with stricter validation: `host:port` addresses, id characters and heartbeat range.

