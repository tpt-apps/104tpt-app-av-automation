# tpt-app-av-automation-tauri

The desktop application for [TPT AV Automation](../../README.md): dashboard, rule builder, device
manager and timeline, as a native [Tauri 2](https://tauri.app/) shell around the embedded service.

This crate is **not published** (`publish = false`) and is **excluded from the default workspace**
so that working on the engine does not require the Tauri CLI or a webview toolchain.

## Architecture

The webview renders; it holds no business logic. Every Tauri command is a thin delegation to
`UiBridge` from the `service` crate, which is the same command surface the local API exposes. The
GUI, the CLI and the API therefore share one engine and one rule format.

```text
ui/ (webview)  ──invoke──▶  src/main.rs (#[tauri::command])  ──▶  UiBridge  ──▶  engine
```

## Features

- **Dashboard:** live status, armed rules, device health, recent executions.
- **Rule builder:** create and edit rules, validate, simulate, arm and disarm; pack version history
  with restore.
- **Device manager:** add, edit and delete devices. Changes apply live and are written to the
  devices file at once (the original is kept once as `devices.yaml.bak`); a device a rule still
  uses cannot be deleted.
- **Timeline:** the execution history, with the reason each rule fired.
- **Templates:** start from a built-in show template.

## Building

Prerequisites: the Rust toolchain, the Tauri CLI (`cargo install tauri-cli --version "^2"`) and a
platform webview (WebView2 on Windows).

```sh
# run in development
cargo tauri dev --manifest-path crates/tpt-app-av-automation-tauri/Cargo.toml

# plain compile check
cargo build --manifest-path crates/tpt-app-av-automation-tauri/Cargo.toml
```

Release bundles use the `custom-protocol` feature (enabled by the Tauri bundler). Configuration is
in `tauri.conf.json` and `capabilities/`.

## Licence

Dual-licensed under [MIT](../../LICENSE-MIT) or [Apache-2.0](../../LICENSE-APACHE), at your option.
