//! TPT AV Automation desktop app (spec §12).
//!
//! A native shell around the embedded service: every command is a thin delegation to the
//! [`UiBridge`], which is the same command surface the local API exposes — the GUI, the CLI and
//! the API share one engine and one rule format (spec §13, §24). The webview renders; it holds no
//! business logic.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod boot;

use std::path::PathBuf;
use std::sync::mpsc::RecvTimeoutError;
use std::time::Duration;

use serde_json::{json, Value};
use tauri::{Emitter, Manager, State};
use tpt_app_av_automation_service::UiBridge;

/// Where `save-pack` writes: the pack file the app started from, if any.
struct PackFile(Option<PathBuf>);

/// Where device edits are written (shown in the Devices panel).
struct DevicesFile(Option<PathBuf>);

#[tauri::command]
fn dashboard(bridge: State<'_, UiBridge>) -> Value {
    bridge.dashboard()
}

#[tauri::command]
fn devices(bridge: State<'_, UiBridge>) -> Value {
    bridge.devices()
}

#[tauri::command]
// One argument per timeline filter; collapsing them into a struct would only move the count.
#[allow(clippy::too_many_arguments)]
fn executions(
    bridge: State<'_, UiBridge>,
    rule: Option<String>,
    status: Option<String>,
    device: Option<String>,
    since: Option<u64>,
    until: Option<u64>,
    simulated: Option<bool>,
    limit: Option<usize>,
) -> Result<Value, String> {
    bridge.executions(rule, status, device, since, until, simulated, limit)
}

#[tauri::command]
fn incidents(bridge: State<'_, UiBridge>, limit: Option<usize>) -> Result<Value, String> {
    bridge.incidents(limit)
}

#[tauri::command]
fn pack_history(bridge: State<'_, UiBridge>) -> Result<Value, String> {
    bridge.pack_history()
}

#[tauri::command]
fn restore_pack_version(bridge: State<'_, UiBridge>, id: i64) -> Result<Value, String> {
    bridge.restore_pack_version(id)
}

#[tauri::command]
fn pack_yaml(bridge: State<'_, UiBridge>) -> Result<String, String> {
    Ok(bridge.pack_yaml()?.as_str().unwrap_or_default().to_owned())
}

#[tauri::command]
fn pack_json(bridge: State<'_, UiBridge>) -> Result<Value, String> {
    bridge.pack_json()
}

#[tauri::command]
fn validate_pack(bridge: State<'_, UiBridge>, yaml: String) -> Result<Value, String> {
    bridge.validate_pack(&yaml)
}

#[tauri::command]
fn apply_pack(bridge: State<'_, UiBridge>, yaml: String) -> Result<Value, String> {
    bridge.apply_pack(&yaml)
}

#[tauri::command]
fn upsert_rule(bridge: State<'_, UiBridge>, rule: Value) -> Result<Value, String> {
    bridge.upsert_rule(rule)
}

#[tauri::command]
fn remove_rule(bridge: State<'_, UiBridge>, id: String) -> Result<Value, String> {
    bridge.remove_rule(&id)
}

#[tauri::command]
fn arm_rule(bridge: State<'_, UiBridge>, id: String) -> Result<Value, String> {
    bridge.arm(&id)
}

#[tauri::command]
fn disarm_rule(bridge: State<'_, UiBridge>, id: String) -> Result<Value, String> {
    bridge.disarm(&id)
}

#[tauri::command]
fn run_rule(bridge: State<'_, UiBridge>, id: String) -> Result<Value, String> {
    bridge.run_rule(&id)
}

#[tauri::command]
fn cancel_execution(bridge: State<'_, UiBridge>, id: String) -> Result<Value, String> {
    bridge.cancel_execution(&id)
}

#[tauri::command]
fn set_simulation(bridge: State<'_, UiBridge>, on: bool) -> Result<Value, String> {
    bridge.set_simulation(on)
}

#[tauri::command]
fn ping_device(bridge: State<'_, UiBridge>, id: String) -> Result<Value, String> {
    bridge.ping_device(&id)
}

#[tauri::command]
fn templates() -> Value {
    UiBridge::templates()
}

#[tauri::command]
fn apply_template(bridge: State<'_, UiBridge>, name: String) -> Result<Value, String> {
    bridge.apply_template(&name)
}

#[tauri::command]
fn device_configs(bridge: State<'_, UiBridge>) -> Result<Value, String> {
    bridge.device_configs()
}

#[tauri::command]
fn upsert_device(bridge: State<'_, UiBridge>, device: Value) -> Result<Value, String> {
    bridge.upsert_device(device)
}

#[tauri::command]
fn remove_device(bridge: State<'_, UiBridge>, id: String) -> Result<Value, String> {
    bridge.remove_device(&id)
}

#[tauri::command]
fn catalogue() -> Value {
    serde_json::to_value(UiBridge::catalogue()).unwrap_or_else(|_| json!([]))
}

/// Writes the current pack back to the file it was loaded from.
///
/// The engine and the version history are the live truth; this keeps the file on disk in sync so
/// the next start loads what the operator sees.
#[tauri::command]
fn save_pack_file(bridge: State<'_, UiBridge>, file: State<'_, PackFile>) -> Result<Value, String> {
    let yaml = bridge.pack_yaml()?;
    let path = file
        .0
        .as_ref()
        .ok_or_else(|| "no pack file is loaded; this pack lives only in the running engine and its version history".to_string())?;
    std::fs::write(path, yaml.as_str().unwrap_or_default())
        .map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(json!({ "saved": path.display().to_string() }))
}

/// Paths and versions the header and settings screens show.
#[tauri::command]
fn app_info(file: State<'_, PackFile>, devices: State<'_, DevicesFile>) -> Value {
    json!({
        "version": env!("CARGO_PKG_VERSION"),
        "pack_file": file.0.as_ref().map(|p| p.display().to_string()),
        "devices_file": devices.0.as_ref().map(|p| p.display().to_string()),
    })
}

/// Forwards live execution records from the service to the webview as `execution` events.
fn pump_events(app: tauri::AppHandle, rx: std::sync::mpsc::Receiver<String>) {
    loop {
        match rx.recv_timeout(Duration::from_secs(1)) {
            Ok(line) => {
                let _ = app.emit("execution", line);
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

fn fail(message: String) -> ! {
    eprintln!("tpt-av-automation: {message}");
    std::process::exit(2);
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let args: Vec<String> = std::env::args().skip(1).collect();
    let config = boot::load_config(&args).unwrap_or_else(|e| fail(e.to_string()));
    let loaded = boot::load(config).unwrap_or_else(|e| fail(e.to_string()));
    let pack_file = PackFile(loaded.pack_path.clone());
    let devices_file = DevicesFile(loaded.devices_path.clone());
    let (bridge, worker) = boot::spawn(loaded).unwrap_or_else(|e| fail(e.to_string()));

    let result = tauri::Builder::default()
        .manage(bridge.clone())
        .manage(pack_file)
        .manage(devices_file)
        .invoke_handler(tauri::generate_handler![
            dashboard,
            devices,
            executions,
            incidents,
            pack_history,
            restore_pack_version,
            pack_yaml,
            pack_json,
            validate_pack,
            apply_pack,
            upsert_rule,
            remove_rule,
            arm_rule,
            disarm_rule,
            run_rule,
            cancel_execution,
            set_simulation,
            ping_device,
            templates,
            apply_template,
            device_configs,
            upsert_device,
            remove_device,
            catalogue,
            save_pack_file,
            app_info,
        ])
        .setup({
            let bridge = bridge.clone();
            move |app| {
                // Clone the handle out of the non-Send `App` before leaving this thread.
                let app = app.handle().clone();
                let rx = bridge.handle().subscribe();
                std::thread::Builder::new()
                    .name("execution-events".to_string())
                    .spawn(move || pump_events(app, rx))
                    .expect("event pump thread");
                Ok(())
            }
        })
        .on_window_event(|window, event| {
            // Closing the window stops the embedded service cleanly, so state and armed rules
            // persist (spec §11).
            if matches!(event, tauri::WindowEvent::CloseRequested { .. }) {
                window.app_handle().state::<UiBridge>().handle().shutdown();
            }
        })
        .run(tauri::generate_context!());
    if let Err(e) = result {
        fail(e.to_string());
    }

    bridge.handle().shutdown();
    let _ = worker.join();
}
