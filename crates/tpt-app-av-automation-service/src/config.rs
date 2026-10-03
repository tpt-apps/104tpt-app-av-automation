//! Service configuration (`service.yaml`).

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tpt_app_av_automation_core::{Diagnostic, Error, Result};

/// A UDP listener for inbound control traffic.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListenerConfig {
    /// `osc`, `artnet`, `sacn`, `ump` or `midi`.
    ///
    /// `ump` is MIDI 2.0 over UDP: raw Universal MIDI Packets, which is how a MIDI 2.0 gateway
    /// on the venue network delivers them. `midi` is a local MIDI 1.0 port.
    pub protocol: String,
    /// Address to bind, e.g. `0.0.0.0:9000`; for `midi`, (part of) the MIDI input port name.
    pub bind: String,
}

/// The local API (spec §14). Disabled by default.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiConfig {
    /// Master switch. Off unless explicitly enabled.
    #[serde(default)]
    pub enabled: bool,
    /// Bind address. Must be a loopback address.
    #[serde(default = "default_api_bind")]
    pub bind: String,
    /// Bearer token. Required whenever the API is enabled.
    #[serde(default)]
    pub token: Option<String>,
}

fn default_api_bind() -> String {
    "127.0.0.1:8787".to_string()
}

impl Default for ApiConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            bind: default_api_bind(),
            token: None,
        }
    }
}

/// Everything the service needs besides the rule pack and device file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ServiceConfig {
    /// SQLite database path. `None` disables persistence (state is lost on restart).
    #[serde(default)]
    pub state_db: Option<PathBuf>,
    /// Site UTC offset in minutes, for schedules and time windows.
    #[serde(default)]
    pub utc_offset_minutes: i32,
    /// Engine tick interval (schedules and heartbeat deadlines), in milliseconds.
    #[serde(default = "default_tick_ms")]
    pub tick_ms: u64,
    /// How often devices with probe-capable endpoints are pinged, in milliseconds.
    #[serde(default = "default_ping_ms")]
    pub heartbeat_interval_ms: u64,
    /// Inbound UDP listeners.
    #[serde(default)]
    pub listeners: Vec<ListenerConfig>,
    /// Local API.
    #[serde(default)]
    pub api: ApiConfig,
    /// Programs `workflow.exec` may run. Empty disables external programs entirely.
    #[serde(default)]
    pub exec_allow: Vec<String>,
    /// Force simulation regardless of rule arming.
    #[serde(default)]
    pub simulate: bool,
    /// Executions kept in the database.
    #[serde(default = "default_keep")]
    pub keep_executions: u64,
}

fn default_tick_ms() -> u64 {
    250
}

fn default_ping_ms() -> u64 {
    5_000
}

fn default_keep() -> u64 {
    100_000
}

impl Default for ServiceConfig {
    fn default() -> Self {
        Self {
            state_db: None,
            utc_offset_minutes: 0,
            tick_ms: default_tick_ms(),
            heartbeat_interval_ms: default_ping_ms(),
            listeners: Vec::new(),
            api: ApiConfig::default(),
            exec_allow: Vec::new(),
            simulate: false,
            keep_executions: default_keep(),
        }
    }
}

impl ServiceConfig {
    /// Parses YAML.
    pub fn from_yaml_str(source: &str) -> Result<Self> {
        serde_yaml::from_str(source).map_err(|e| Error::Parse(e.to_string()))
    }

    /// Reads a config file.
    pub fn from_path(path: impl AsRef<Path>) -> Result<Self> {
        let text = std::fs::read_to_string(path.as_ref())
            .map_err(|e| Error::Io(format!("{}: {e}", path.as_ref().display())))?;
        Self::from_yaml_str(&text)
    }

    /// Checks the configuration, in particular the API safety rules (spec §14, §16).
    pub fn validate(&self) -> Vec<Diagnostic> {
        let mut out = Vec::new();
        if self.tick_ms == 0 {
            out.push(Diagnostic::error("tick_ms", "must be greater than zero"));
        }
        if self.heartbeat_interval_ms == 0 {
            out.push(Diagnostic::error(
                "heartbeat_interval_ms",
                "must be greater than zero",
            ));
        }
        for (i, l) in self.listeners.iter().enumerate() {
            if !matches!(
                l.protocol.as_str(),
                "osc" | "artnet" | "sacn" | "ump" | "midi" | "dmx512"
            ) {
                out.push(Diagnostic::error(
                    format!("listeners[{i}].protocol"),
                    format!(
                        "unknown protocol `{}`; expected osc, artnet, sacn, ump, midi or dmx512",
                        l.protocol
                    ),
                ));
            }
            if l.protocol == "midi" {
                if l.bind.trim().is_empty() {
                    out.push(Diagnostic::error(
                        format!("listeners[{i}].bind"),
                        "a midi listener needs (part of) the MIDI input port name in `bind`",
                    ));
                }
            } else if l.protocol == "dmx512" {
                if l.bind.trim().is_empty() {
                    out.push(Diagnostic::error(
                        format!("listeners[{i}].bind"),
                        "a dmx512 listener needs a serial port name in `bind`",
                    ));
                }
            } else if l.bind.parse::<SocketAddr>().is_err() {
                out.push(Diagnostic::error(
                    format!("listeners[{i}].bind"),
                    format!("`{}` is not a valid host:port address", l.bind),
                ));
            }
        }
        if self.api.enabled {
            match self.api.bind.parse::<SocketAddr>() {
                Ok(addr) if addr.ip().is_loopback() => {}
                Ok(addr) => out.push(Diagnostic::error(
                    "api.bind",
                    format!(
                        "the local API only binds to loopback addresses, not {}",
                        addr.ip()
                    ),
                )),
                Err(_) => out.push(Diagnostic::error(
                    "api.bind",
                    format!("`{}` is not a valid host:port address", self.api.bind),
                )),
            }
            match self.api.token.as_deref() {
                None | Some("") => out.push(Diagnostic::error(
                    "api.token",
                    "a token is required when the API is enabled",
                )),
                Some(t) if t.len() < 16 => out.push(Diagnostic::error(
                    "api.token",
                    "the API token must be at least 16 characters",
                )),
                Some(_) => {}
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tpt_app_av_automation_core::Severity;

    fn errors(config: &ServiceConfig) -> Vec<String> {
        config
            .validate()
            .into_iter()
            .filter(|d| d.severity == Severity::Error)
            .map(|d| d.location)
            .collect()
    }

    #[test]
    fn defaults_are_safe() {
        let config = ServiceConfig::default();
        assert!(!config.api.enabled, "API is off by default");
        assert!(
            config.exec_allow.is_empty(),
            "external programs are off by default"
        );
        assert!(config.state_db.is_none());
        assert!(errors(&config).is_empty());
    }

    #[test]
    fn enabling_the_api_requires_a_loopback_bind_and_a_strong_token() {
        let mut config = ServiceConfig::default();
        config.api.enabled = true;
        assert_eq!(errors(&config), ["api.token"]);
        config.api.token = Some("short".into());
        assert_eq!(errors(&config), ["api.token"]);
        config.api.token = Some("0123456789abcdef".into());
        assert!(errors(&config).is_empty());
        config.api.bind = "0.0.0.0:8787".into();
        assert_eq!(errors(&config), ["api.bind"]);
        config.api.bind = "192.168.1.5:8787".into();
        assert_eq!(errors(&config), ["api.bind"]);
        config.api.bind = "[::1]:8787".into();
        assert!(errors(&config).is_empty());
        config.api.bind = "localhost".into();
        assert_eq!(errors(&config), ["api.bind"]);
    }

    #[test]
    fn a_disabled_api_is_not_policed() {
        let mut config = ServiceConfig::default();
        config.api.bind = "0.0.0.0:1".into();
        assert!(errors(&config).is_empty());
    }

    #[test]
    fn listeners_are_validated() {
        let config = ServiceConfig::from_yaml_str(
            "listeners:\n  - {protocol: osc, bind: \"0.0.0.0:9000\"}\n  - {protocol: bogus, bind: nope}\n",
        )
        .unwrap();
        assert_eq!(
            errors(&config),
            ["listeners[1].protocol", "listeners[1].bind"]
        );
    }

    #[test]
    fn yaml_parses_with_defaults_and_rejects_garbage() {
        let config = ServiceConfig::from_yaml_str("utc_offset_minutes: 720\n").unwrap();
        assert_eq!(config.utc_offset_minutes, 720);
        assert_eq!(config.tick_ms, 250);
        assert!(ServiceConfig::from_yaml_str("tick_ms: [").is_err());
    }
}
