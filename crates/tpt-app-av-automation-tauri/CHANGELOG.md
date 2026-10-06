# Changelog

All notable changes to `tpt-app-av-automation-tauri` are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/) and the crate uses semantic versioning. Workspace-wide
changes are recorded in the [root changelog](../../CHANGELOG.md).

## [Unreleased]

### Fixed / Changed
- Desktop device manager: add, edit and delete devices from the Devices tab; changes apply live and are written to the devices file at once.

## [0.1.0]

Initial development release.

### Added
- Tauri 2 desktop shell delegating every command to `UiBridge`.
- Dashboard, rule builder, device manager and execution timeline.
- Pack version history with restore, and built-in show templates.

