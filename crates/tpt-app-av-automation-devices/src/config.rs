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
#[serde(deny_unknown_fields)]
pub struct DeviceConfig {
    /// Identifier referenced from rules.
    pub id: String,
    /// Display name; defaults to the id.
    #[serde(default)]
    pub name: Option<String>,
    /// Category.
    #[serde(default)]
    pub kind: DeviceKind,
    /// `osc`, `artnet`, `sacn`, `midi`, `ump`, `dmx512`, `media` or `virtual`.
    pub protocol: String,
    /// `host:port` for network protocols, (part of) the MIDI port name, or the serial port name
    /// (e.g. `COM3` or `/dev/ttyUSB0`) for `dmx512`.
    #[serde(default)]
    pub address: Option<String>,
    /// Heartbeat deadline in milliseconds.
    #[serde(default)]
    pub heartbeat_ms: Option<u64>,
    /// DMX scene table.
    #[serde(default)]
    pub scenes: BTreeMap<String, SceneDef>,
    /// OSC address templates for a `media` device; defaults apply when absent.
    #[serde(default)]
    pub media: Option<crate::media::MediaTemplates>,
}

impl DeviceConfig {
    /// Builds the registry entry.
    pub fn to_device(&self) -> Device {
        let mut device = Device::new(
            self.id.as_str(),
            self.name.clone().unwrap_or_else(|| self.id.clone()),
        );
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
#[serde(deny_unknown_fields)]
pub struct DeviceFile {
    /// Devices.
    #[serde(default)]
    pub devices: Vec<DeviceConfig>,
}

const PROTOCOLS: [&str; 8] = [
    "osc", "artnet", "sacn", "midi", "ump", "dmx512", "media", "virtual",
];

/// Longest heartbeat deadline accepted: one day.
const MAX_HEARTBEAT_MS: u64 = 86_400_000;

/// True when `address` looks like `host:port` with a non-empty host and a numeric port.
fn is_host_port(address: &str) -> bool {
    match address.trim().rsplit_once(':') {
        Some((host, port)) => {
            let host = host.trim_start_matches('[').trim_end_matches(']');
            !host.is_empty() && !host.contains(char::is_whitespace) && port.parse::<u16>().is_ok()
        }
        None => false,
    }
}

/// Ids are used in rules and file names, so keep them to a plain, unambiguous character set.
fn is_valid_id(id: &str) -> bool {
    id.chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

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

    /// Renders the document as YAML (comments in a hand-written file are not preserved).
    pub fn to_yaml_string(&self) -> Result<String> {
        serde_yaml::to_string(self).map_err(|e| Error::Parse(e.to_string()))
    }

    /// Writes the file atomically: a temp file in the same folder, then a rename, so a crash
    /// never leaves a half-written device list. Parent folders are created.
    pub fn write_to_path(&self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        let text = self.to_yaml_string()?;
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)
                .map_err(|e| Error::Io(format!("{}: {e}", parent.display())))?;
        }
        let mut tmp = path.as_os_str().to_owned();
        tmp.push(".tmp");
        let tmp = std::path::PathBuf::from(tmp);
        std::fs::write(&tmp, text).map_err(|e| Error::Io(format!("{}: {e}", tmp.display())))?;
        std::fs::rename(&tmp, path).map_err(|e| Error::Io(format!("{}: {e}", path.display())))
    }

    /// Copies an existing file to `<path>.bak`; a missing file is not an error.
    pub fn backup(path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        if !path.exists() {
            return Ok(());
        }
        let mut bak = path.as_os_str().to_owned();
        bak.push(".bak");
        std::fs::copy(path, &bak)
            .map(|_| ())
            .map_err(|e| Error::Io(format!("{}: {e}", path.display())))
    }

    /// Checks ids, protocols and addresses.
    pub fn validate(&self) -> Vec<Diagnostic> {
        let mut out = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for (i, d) in self.devices.iter().enumerate() {
            let loc = format!("devices[{i}]");
            if d.id.trim().is_empty() {
                out.push(Diagnostic::error(
                    format!("{loc}.id"),
                    "device id must not be empty",
                ));
            } else if !is_valid_id(&d.id) {
                out.push(Diagnostic::error(
                    format!("{loc}.id"),
                    "device id may only contain letters, digits, `-`, `_` and `.`",
                ));
            } else if !seen.insert(d.id.clone()) {
                out.push(Diagnostic::error(
                    format!("{loc}.id"),
                    format!("duplicate device id `{}`", d.id),
                ));
            }
            if !PROTOCOLS.contains(&d.protocol.as_str()) {
                out.push(Diagnostic::error(
                    format!("{loc}.protocol"),
                    format!(
                        "unknown protocol `{}`; expected one of: {}",
                        d.protocol,
                        PROTOCOLS.join(", ")
                    ),
                ));
            }
            if matches!(
                d.protocol.as_str(),
                "osc" | "artnet" | "sacn" | "ump" | "media"
            ) && d.address.is_none()
            {
                out.push(Diagnostic::error(
                    format!("{loc}.address"),
                    format!("`{}` devices need a host:port address", d.protocol),
                ));
            }
            if matches!(
                d.protocol.as_str(),
                "osc" | "artnet" | "sacn" | "ump" | "media"
            ) && d.address.as_deref().is_some_and(|a| !is_host_port(a))
            {
                out.push(Diagnostic::error(
                    format!("{loc}.address"),
                    format!(
                        "`{}` address must look like host:port, e.g. 192.168.1.50:9000",
                        d.protocol
                    ),
                ));
            }
            if d.heartbeat_ms
                .is_some_and(|ms| ms == 0 || ms > MAX_HEARTBEAT_MS)
            {
                out.push(Diagnostic::error(
                    format!("{loc}.heartbeat_ms"),
                    format!("heartbeat_ms must be between 1 and {MAX_HEARTBEAT_MS}"),
                ));
            }
            if d.protocol == "midi" && d.address.as_deref().is_none_or(|a| a.trim().is_empty()) {
                out.push(Diagnostic::error(
                    format!("{loc}.address"),
                    "`midi` devices need `address` set to (part of) the MIDI output port name",
                ));
            }
            if d.protocol == "dmx512" && d.address.as_deref().is_none_or(|a| a.trim().is_empty()) {
                out.push(Diagnostic::error(
                    format!("{loc}.address"),
                    "`dmx512` devices need `address` set to the serial port name, e.g. COM3 or /dev/ttyUSB0",
                ));
            }
            if let Some(templates) = &d.media {
                if d.protocol != "media" {
                    out.push(Diagnostic::error(
                        format!("{loc}.media"),
                        "`media` templates apply to `media` devices only",
                    ));
                }
                for message in templates.problems() {
                    out.push(Diagnostic::error(format!("{loc}.media"), message));
                }
            }
            for (name, scene) in &d.scenes {
                if usize::from(scene.start_channel) + scene.values.len() > 512 {
                    out.push(Diagnostic::error(
                        format!("{loc}.scenes.{name}"),
                        "scene runs past channel 512",
                    ));
                }
                if d.protocol == "dmx512" && scene.universe != 1 {
                    out.push(Diagnostic::error(
                        format!("{loc}.scenes.{name}.universe"),
                        format!(
                            "a `dmx512` serial port carries universe 1 only; scene uses universe {}",
                            scene.universe
                        ),
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
    fn a_dmx512_device_needs_a_serial_port_name() {
        let missing =
            DeviceFile::from_yaml_str("devices:\n  - {id: rig, protocol: dmx512}\n").unwrap();
        let diags = missing.validate();
        let locations: Vec<_> = diags.iter().map(|d| d.location.as_str()).collect();
        assert_eq!(locations, vec!["devices[0].address"]);

        let named = DeviceFile::from_yaml_str(
            "devices:\n  - {id: rig, protocol: dmx512, address: 'COM3'}\n",
        )
        .unwrap();
        assert!(
            named.validate().is_empty(),
            "a named serial port is accepted"
        );

        let blank =
            DeviceFile::from_yaml_str("devices:\n  - {id: rig, protocol: dmx512, address: '  '}\n")
                .unwrap();
        assert_eq!(
            blank.validate().len(),
            1,
            "a blank serial port name is rejected"
        );
    }

    #[test]
    fn a_dmx512_scene_on_another_universe_is_rejected() {
        let file = DeviceFile::from_yaml_str(
            "devices:\n  - id: rig\n    protocol: dmx512\n    address: 'COM3'\n    scenes:\n      s: {universe: 2, values: [1]}\n",
        )
        .unwrap();
        let diags = file.validate();
        let locations: Vec<_> = diags.iter().map(|d| d.location.as_str()).collect();
        assert_eq!(locations, vec!["devices[0].scenes.s.universe"]);
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
    fn round_trips_through_yaml_and_disk() {
        let file = DeviceFile::from_yaml_str(SAMPLE).unwrap();
        let again = DeviceFile::from_yaml_str(&file.to_yaml_string().unwrap()).unwrap();
        assert_eq!(file, again);

        let dir = std::env::temp_dir().join(format!("tpt-devcfg-{}", std::process::id()));
        let path = dir.join("nested").join("devices.yaml");
        file.write_to_path(&path).unwrap();
        assert_eq!(DeviceFile::from_path(&path).unwrap(), file);
        DeviceFile::backup(&path).unwrap();
        assert!(dir.join("nested").join("devices.yaml.bak").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rejects_bad_addresses_ids_heartbeats_and_unknown_keys() {
        let file = DeviceFile::from_yaml_str(
            "devices:
  - {id: 'bad id', protocol: osc, address: nope}
  - {id: ok, protocol: osc, address: 'h:99999'}
  - {id: hb, protocol: virtual, heartbeat_ms: 0}
",
        )
        .unwrap();
        let locations: Vec<_> = file.validate().iter().map(|d| d.location.clone()).collect();
        assert!(locations.contains(&"devices[0].id".to_string()));
        assert!(locations.contains(&"devices[0].address".to_string()));
        assert!(locations.contains(&"devices[1].address".to_string()));
        assert!(locations.contains(&"devices[2].heartbeat_ms".to_string()));
        assert!(DeviceFile::from_yaml_str(
            "devices:
  - {id: a, protocol: virtual, heartbeat_msec: 5}
"
        )
        .is_err());
        assert!(DeviceFile::from_yaml_str(
            "devces: []
"
        )
        .is_err());
    }

    #[test]
    fn garbage_is_a_parse_error() {
        assert!(matches!(
            DeviceFile::from_yaml_str(": : :\n\t- ["),
            Err(Error::Parse(_))
        ));
    }
}
