# Changelog

All notable changes to `tpt-app-av-automation-model` are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/) and the crate uses semantic versioning. Workspace-wide
changes are recorded in the [root changelog](../../CHANGELOG.md).

## [Unreleased]

### Fixed / Changed
- `solar` schedules were modelled but rejected when loading YAML; they now load.

## [0.1.0]

Initial development release.

### Added
- `RulePack`, `Rule`, `Trigger`, `Condition` and `ActionStep` types with `deny_unknown_fields` YAML loading.
- Schedule triggers: cron-like, interval, one-shot and solar with offsets.
- OSC, MIDI 1.0/2.0, DMX comparison, device health, heartbeat-missed, device parameter, manual and API triggers.
- Action forms: OSC, MIDI, DMX, media, notify, incident log, wait, invoke rule, library fragments and sandboxed exec.
- `validate_pack` producing located diagnostics, `format_version: 1` and `MAX_CHAIN_LENGTH`.
- Execution record and outcome types.

