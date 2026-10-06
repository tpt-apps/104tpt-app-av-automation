# Changelog

All notable changes to `tpt-app-av-automation-cli` are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/) and the crate uses semantic versioning. Workspace-wide
changes are recorded in the [root changelog](../../CHANGELOG.md).

## [Unreleased]

### Fixed / Changed
- `run` now warns when started without `--state-dir`.

## [0.1.0]

Initial development release.

### Added
- `tpt-av-automation` binary with `validate`, `simulate`, `run`, `watchdog` and `history`.
- `init` to scaffold a show folder from a built-in template (`--list` shows them).
- `devices` to list, check and test-open the devices in a device file.
- Stable exit-code contract 0-6 and `--format json` machine output.

