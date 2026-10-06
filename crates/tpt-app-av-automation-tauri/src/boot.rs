//! Desktop startup: settings, pack and device loading, and the embedded service.
//!
//! Deliberately free of any Tauri types so the whole startup path — the same engine the CLI and
//! the headless service run (spec §13, §24) — is covered by ordinary tests.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use serde::Deserialize;
use tpt_app_av_automation_core::{Error, Result};
use tpt_app_av_automation_devices::DeviceFile;
use tpt_app_av_automation_model::RulePack;
use tpt_app_av_automation_service::{Service, ServiceConfig, UiBridge};

/// The desktop settings file (`tpt-av-automation.yaml`).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesktopConfig {
    /// Rule pack to load at startup. Absent: start with an empty pack and build in the UI.
    #[serde(default)]
    pub pack: Option<PathBuf>,
    /// Device registry to load at startup. Absent: an empty registry.
    #[serde(default)]
    pub devices: Option<PathBuf>,
    /// Service settings (listeners, API, tick, exec policy). The desktop adds a default
    /// state database so packs, history and armed state survive restarts (spec §15).
    #[serde(default)]
    pub service: ServiceConfig,
}

/// Everything the embedded service needs, resolved and loaded.
#[derive(Debug, Clone)]
pub struct Loaded {
    /// The settings these came from.
    pub config: DesktopConfig,
    /// The pack the engine will run.
    pub pack: RulePack,
    /// The device registry.
    pub devices: DeviceFile,
    /// Where the state database lives (persisted by default).
    pub state_db: Option<PathBuf>,
    /// The rule pack file every edit made in the app is written to: the one named in the settings,
    /// else `pack.yaml` beside the state database (created on the first change). The next start
    /// loads it, so work done in the rule builder is never lost.
    pub pack_path: Option<PathBuf>,
    /// The device file edits made in the app are written to: the one named in the settings, else
    /// `devices.yaml` next to the state database (created on the first edit).
    pub devices_path: Option<PathBuf>,
}

impl DesktopConfig {
    /// Parses settings from YAML text.
    pub fn from_yaml_str(source: &str) -> Result<Self> {
        serde_yaml::from_str(source).map_err(|e| Error::Parse(e.to_string()))
    }

    /// Parses settings from a file.
    pub fn from_path(path: impl AsRef<Path>) -> Result<Self> {
        let text = std::fs::read_to_string(path.as_ref())
            .map_err(|e| Error::Io(format!("{}: {e}", path.as_ref().display())))?;
        Self::from_yaml_str(&text)
            .map_err(|e| Error::Parse(format!("{}: {e}", path.as_ref().display())))
    }
}

/// Resolves the default state-database location (`%APPDATA%` / XDG data home).
pub fn default_state_db() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    let base = std::env::var("APPDATA").ok().map(PathBuf::from);
    #[cfg(not(target_os = "windows"))]
    let base = std::env::var("XDG_DATA_HOME")
        .ok()
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var("HOME")
                .ok()
                .map(|h| PathBuf::from(h).join(".local/share"))
        });
    base.map(|dir| dir.join("tpt-av-automation").join("state.db"))
}

/// The file used when the settings name none: `name` beside the state database.
fn default_sibling(config: &DesktopConfig, name: &str) -> Option<PathBuf> {
    let db = config.service.state_db.as_ref()?;
    Some(db.parent()?.join(name))
}

/// Loads the pack, devices and settings named by `config`.
pub fn load(config: DesktopConfig) -> Result<Loaded> {
    let mut config = config;
    if config.service.state_db.is_none() {
        config.service.state_db = default_state_db();
    }
    let pack_path = config
        .pack
        .clone()
        .or_else(|| default_sibling(&config, "pack.yaml"));
    let pack = match &pack_path {
        Some(path) if path.exists() || config.pack.is_some() => RulePack::from_path(path)?,
        _ => RulePack::new("New Show"),
    };
    let devices_path = config
        .devices
        .clone()
        .or_else(|| default_sibling(&config, "devices.yaml"));
    let devices = match &devices_path {
        Some(path) if path.exists() => DeviceFile::from_path(path)?,
        Some(path) if config.devices.is_some() => DeviceFile::from_path(path)?,
        _ => DeviceFile::default(),
    };
    Ok(Loaded {
        state_db: config.service.state_db.clone(),
        config,
        pack,
        pack_path,
        devices,
        devices_path,
    })
}

/// Builds the service from already-loaded inputs and starts it on a background thread.
///
/// The desktop app embeds the headless service as-is: listeners, API (when enabled), persistence
/// and heartbeats all behave exactly as in `run --service`.
pub fn spawn(loaded: Loaded) -> Result<(UiBridge, JoinHandle<()>)> {
    let store = match &loaded.state_db {
        Some(path) => {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            Some(tpt_app_av_automation_service::Store::open(path)?)
        }
        None => None,
    };
    let mut service = Service::build(
        loaded.pack,
        loaded.devices,
        loaded.config.service.clone(),
        store,
        Arc::new(tpt_app_av_automation_core::SystemClock),
    )?;
    if let Some(path) = loaded.devices_path {
        service.set_devices_path(path);
    }
    if let Some(path) = loaded.pack_path {
        service.set_pack_path(path);
    }
    let bridge = UiBridge::new(service.handle());
    let handle = bridge.handle().clone();
    let worker = std::thread::Builder::new()
        .name("av-automation-service".to_string())
        .spawn(move || {
            if let Err(e) = service.run() {
                tracing::error!("service stopped with an error: {e}");
            }
        })
        .map_err(|e| Error::Control(e.to_string()))?;
    if !handle.wait_ready(Duration::from_secs(10)) {
        handle.shutdown();
        return Err(Error::Control("the embedded service did not start".into()));
    }
    Ok((bridge, worker))
}

/// Finds and parses the settings file: an explicit argument wins, then `tpt-av-automation.yaml`
/// in the working directory. No file at all is a valid "new show" setup.
pub fn load_config(args: &[String]) -> Result<DesktopConfig> {
    match args.first() {
        Some(path) => DesktopConfig::from_path(path),
        None => match Path::new("tpt-av-automation.yaml") {
            candidate if candidate.exists() => DesktopConfig::from_path(candidate),
            _ => Ok(DesktopConfig::default()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tpt_app_av_automation_service::ApiConfig;

    #[test]
    fn settings_parse_with_defaults() {
        let config = DesktopConfig::from_yaml_str(
            "pack: rules/show.yaml\ndevices: devices.yaml\nservice:\n  tick_ms: 100\n",
        )
        .unwrap();
        assert_eq!(config.pack.as_deref(), Some(Path::new("rules/show.yaml")));
        assert_eq!(config.service.tick_ms, 100);
        assert!(!config.service.api.enabled);
    }

    #[test]
    fn unknown_settings_keys_are_rejected() {
        assert!(DesktopConfig::from_yaml_str("packs: rules/show.yaml").is_err());
    }

    #[test]
    fn missing_settings_are_a_default_new_show() {
        let config = load_config(&[]).unwrap();
        assert!(config.pack.is_none());
        assert!(config.devices.is_none());
    }

    #[test]
    fn an_explicit_settings_path_is_required_to_exist() {
        let args = vec!["definitely-not-here.yaml".to_string()];
        assert!(load_config(&args).is_err());
    }

    #[test]
    fn load_without_files_starts_an_empty_pack_and_default_database() {
        let loaded = load(DesktopConfig::default()).unwrap();
        assert!(loaded.state_db.is_some(), "the desktop persists by default");
        assert!(loaded.pack_path.is_some() && loaded.devices_path.is_some());
    }

    #[test]
    fn rules_built_in_the_app_survive_a_restart() {
        let dir = std::env::temp_dir().join(format!("tpt-av-tauri-pack-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let config = || DesktopConfig {
            service: ServiceConfig {
                tick_ms: 10,
                state_db: Some(dir.join("state.db")),
                ..ServiceConfig::default()
            },
            ..DesktopConfig::default()
        };

        let first = load(config()).unwrap();
        assert_eq!(first.pack.name, "New Show");
        assert_eq!(first.pack.rules.len(), 0);
        let (bridge, worker) = spawn(first).unwrap();
        let answer = bridge
            .upsert_rule(serde_json::json!({
                "name": "Go", "trigger": {"type": "manual"},
                "actions": [{"type": "notify.operator", "message": "hi"}]
            }))
            .unwrap();
        assert_ne!(answer["valid"], false, "{answer}");
        bridge.handle().shutdown();
        worker.join().unwrap();

        assert!(dir.join("pack.yaml").exists(), "the pack is written beside the database");
        let second = load(config()).unwrap();
        assert_eq!(second.pack.rules.len(), 1, "the rule is loaded again on the next start");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn devices_default_to_a_file_beside_the_database_and_load_when_present() {
        let dir = std::env::temp_dir().join(format!("tpt-av-tauri-dev-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let config = || DesktopConfig {
            service: ServiceConfig {
                state_db: Some(dir.join("state.db")),
                ..ServiceConfig::default()
            },
            ..DesktopConfig::default()
        };

        let first = load(config()).unwrap();
        assert_eq!(first.devices_path.as_deref(), Some(dir.join("devices.yaml").as_path()));
        assert_eq!(first.devices.devices.len(), 0, "no file yet is an empty registry");

        // The first edit made in the app creates the file; the next start loads it.
        let (bridge, worker) = spawn(first).unwrap();
        let answer = bridge
            .upsert_device(serde_json::json!({"id": "cam", "protocol": "virtual"}))
            .unwrap();
        assert_eq!(answer["valid"], true);
        bridge.handle().shutdown();
        worker.join().unwrap();

        assert!(dir.join("devices.yaml").exists());
        assert!(!dir.join("devices.yaml.bak").exists(), "nothing to back up the first time");
        let second = load(config()).unwrap();
        assert_eq!(second.devices.devices.len(), 1);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn load_reads_pack_and_device_files() {
        let dir = std::env::temp_dir().join(format!("tpt-av-tauri-load-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let pack_path = dir.join("pack.yaml");
        let devices_path = dir.join("devices.yaml");
        std::fs::write(
            &pack_path,
            "format_version: 1\nname: Desktop\nrevision: 1\nrules:\n  - id: go\n    name: Go\n    trigger: { type: manual }\n    actions:\n      - { id: n, type: notify.operator, message: hi }\n",
        )
        .unwrap();
        std::fs::write(
            &devices_path,
            "devices:\n  - { id: p, protocol: virtual }\n",
        )
        .unwrap();

        let config = DesktopConfig {
            pack: Some(pack_path.clone()),
            devices: Some(devices_path),
            service: ServiceConfig {
                api: ApiConfig::default(),
                ..ServiceConfig::default()
            },
        };
        let loaded = load(config).unwrap();
        assert_eq!(loaded.pack.rules.len(), 1);
        assert_eq!(loaded.devices.devices.len(), 1);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_embedded_service_runs_and_answers_the_bridge() {
        let config = DesktopConfig {
            service: ServiceConfig {
                tick_ms: 10,
                state_db: None,
                ..ServiceConfig::default()
            },
            ..DesktopConfig::default()
        };
        let loaded = load(config).unwrap();
        let (bridge, worker) = spawn(loaded).unwrap();
        let board = bridge.dashboard();
        assert_eq!(board["status"]["mode"], "live");
        bridge.handle().shutdown();
        worker.join().unwrap();
    }
}
