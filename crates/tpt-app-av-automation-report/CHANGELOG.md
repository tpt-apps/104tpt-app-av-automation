# Changelog

All notable changes to `tpt-app-av-automation-report` are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/) and the crate uses semantic versioning. Workspace-wide
changes are recorded in the [root changelog](../../CHANGELOG.md).

## [Unreleased]

Nothing yet.

## [0.1.0]

Initial development release.

### Added
- `render_record` human output that always states simulation versus live.
- `machine_result` stable JSON per execution.
- `ExitCode` / `exit_code` stable 0-6 process exit-code contract.
- `Filter` for execution log queries and `describe_event`.

