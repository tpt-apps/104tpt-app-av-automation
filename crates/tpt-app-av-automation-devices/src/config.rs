//! The `devices.yaml` file format: device registry entries plus per-device scene tables.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};
use tpt_app_av_automation_core::{Diagnostic, Error, Result};

use crate::registry::{Device, DeviceKind, ProtocolBinding};

/// A named DMX scene: a block of channel values applied on one universe.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SceneDef {
    /// Universe number.
    pub universe: u16,
    /// Zero-based first channel.
    #[serde(default)]
    pub start_channel: u16,
    /// Channel values.
    pub values: Vec<u8>,
}

/// One device entry in `devices.yaml`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeviceConfig {
    /// Identifier referenced from rules.
    pub id: String,
    /// Display name; defaults to the id.
    #[serde(default)]
    pub name: Option<String>,
    /// Category.
    #[serde(default)]
    pub kind: DeviceKind,
    /// `osc`, `artnet`, `sacn`, `midi`, `ump` or `virtual`.
    pub protocol: String,
    /// `host:port` for network protocols, or (part of) the MIDI port name.
    #[serde(default)]
    pub address: Option<String>,
    /// Heartbeat deadline in milliseconds.
    #[serde(default)]
    pub heartbeat_ms: Option<u64>,
    /// DMX scene table.
    #[serde(default)]
    pub scenes: BTreeMap<String, SceneDef>,
}

impl DeviceConfig {
    /// Builds the registry entry.
    pub fn to_device(&self) -> Device {
        let mut device = Device::new(self.id.as_str(), self.name.clone().unwrap_or_else(|| self.id.clone()));
        device.kind = self.kind;
        device.protocol = ProtocolBinding {
            protocol: self.protocol.clone(),
            address: self.address.clone(),
        };
        device.heartbeat_ms = self.heartbeat_ms;
        device
    }
}

/// The whole `devices.yaml` document.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DeviceFile {
    /// Devices.
    #[serde(default)]
    pub devices: Vec<DeviceConfig>,
}

const PROTOCOLS: [&str; 6] = ["osc", "artnet", "sacn", "midi", "ump", "virtual"];

impl DeviceFile {
    /// Parses YAML; a malformed document is a parse error, never a panic.
    pub fn from_yaml_str(source: &str) -> Result<Self> {
        serde_yaml::from_str(source).map_err(|e| Error::Parse(e.to_string()))
    }

    /// Reads and parses a file.
    pub fn from_path(path: impl AsRef<Path>) -> Result<Self> {
        let text = std::fs::read_to_string(path.as_ref())
            .map_err(|e| Error::Io(format!("{}: {e}", path.as_ref().display())))?;
        Self::from_yaml_str(&text)
    }

    /// Checks ids, protocols and addresses.
    pub fn validate(&self) -> Vec<Diagnostic> {
        let mut out = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for (i, d) in self.devices.iter().enumerate() {
            let loc = format!("devices[{i}]");
            if d.id.trim().is_empty() {
                out.push(Diagnostic::error(format!("{loc}.id"), "device id must not be empty"));
            } else if !seen.insert(d.id.clone()) {
                out.push(Diagnostic::error(
                    format!("{loc}.id"),
                    format!("duplicate device id `{}`", d.id),
                ));
            }
            if !PROTOCOLS.contains(&d.protocol.as_str()) {
                out.push(Diagnostic::error(
                    format!("{loc}.protocol"),
                    format!("unknown protocol `{}`; expected one of: {}", d.protocol, PROTOCOLS.join(", ")),
                ));
            }
            if matches!(d.protocol.as_str(), "osc" | "artnet" | "sacn" | "ump") && d.address.is_none() {
                out.push(Diagnostic::error(
                    format!("{loc}.address"),
                    format!("`{}` devices need a host:port address", d.protocol),
                ));
            }
            if d.protocol == "midi" && d.address.as_deref().is_none_or(|a| a.trim().is_empty()) {
                out.push(Diagnostic::error(
                    format!("{loc}.address"),
                    "`midi` devices need `address` set to (part of) the MIDI output port name",
                ));
            }
            for (name, scene) in &d.scenes {
                if usize::from(scene.start_channel) + scene.values.len() > 512 {
                    out.push(Diagnostic::error(
                        format!("{loc}.scenes.{name}"),
                        "scene runs past channel 512",
                    ));
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tpt_app_av_automation_core::Severity;

    const SAMPLE: &str = r#"
devices:
  - id: lighting-rack
    name: Lighting Rack
    kind: lighting
    protocol: artnet
    address: 127.0.0.1:6454
    heartbeat_ms: 10000
    scenes:
      house-to-half: { universe: 1, values: [128, 128] }
  - id: projector-1
    protocol: osc
    address: 127.0.0.1:9000
"#;

    #[test]
    fn parses_sample() {
        let file = DeviceFile::from_yaml_str(SAMPLE).unwrap();
        assert_eq!(file.devices.len(), 2);
        assert!(file.validate().is_empty());
        let d = file.devices[1].to_device();
        assert_eq!(d.name, "projector-1");
        assert_eq!(d.protocol.protocol, "osc");
    }

    #[test]
    fn flags_bad_protocols_duplicates_and_missing_addresses() {
        let file = DeviceFile::from_yaml_str(
            "devices:\n  - {id: a, protocol: osc}\n  - {id: a, protocol: bogus}\n",
        )
        .unwrap();
        let diags = file.validate();
        assert!(diags.iter().all(|d| d.severity == Severity::Error));
        let locations: Vec<_> = diags.iter().map(|d| d.location.as_str()).collect();
        assert!(locations.contains(&"devices[0].address"));
        assert!(locations.contains(&"devices[1].id"));
        assert!(locations.contains(&"devices[1].protocol"));
    }

    #[test]
    fn rejects_scenes_past_channel_512() {
        let file = DeviceFile::from_yaml_str(
            "devices:\n  - id: a\n    protocol: virtual\n    scenes:\n      s: {universe: 1, start_channel: 511, values: [1, 2]}\n",
        )
        .unwrap();
        assert_eq!(file.validate().len(), 1);
    }

    #[test]
    fn garbage_is_a_parse_error() {
        assert!(matches!(DeviceFile::from_yaml_str(": : :\n\t- ["), Err(Error::Parse(_))));
    }
}
