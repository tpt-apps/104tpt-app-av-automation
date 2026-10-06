//! A media-control endpoint: turns `media.*` actions into OSC messages for a media server or
//! matrix (spec §8.2).
//!
//! `tpt-kinetix` has no source-control API, so media start/stop/switch is delivered the way most
//! venue media servers and routers are actually driven: as OSC at a configurable address.

use std::net::UdpSocket;

use serde::{Deserialize, Serialize};
use tpt_app_av_automation_model::MediaOperation;
use tpt_av_control_osc::OscMessage;

use crate::command::Command;
use crate::endpoint::{Endpoint, EndpointError};

const DEFAULT_VIDEO: &str = "/media/video/{source}/{operation}";
const DEFAULT_AUDIO: &str = "/media/audio/{from}/{to}/{operation}";

/// OSC address templates for a `media` device.
///
/// Placeholders: `{source}` and `{operation}` for video; `{from}`, `{to}` and `{operation}` for
/// audio. `{operation}` expands to `start`, `stop` or `switch`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MediaTemplates {
    /// Video template; defaults to `/media/video/{source}/{operation}`.
    #[serde(default)]
    pub video: Option<String>,
    /// Audio template; defaults to `/media/audio/{from}/{to}/{operation}`.
    #[serde(default)]
    pub audio: Option<String>,
}

impl MediaTemplates {
    /// Human-readable problems with the templates, empty when they are usable.
    pub fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        for (name, template, required) in [
            ("video", &self.video, &["{source}"][..]),
            ("audio", &self.audio, &["{from}", "{to}"][..]),
        ] {
            let Some(t) = template else { continue };
            if !t.starts_with('/') {
                out.push(format!("{name} template must start with `/`"));
            }
            for placeholder in required {
                if !t.contains(placeholder) {
                    out.push(format!("{name} template is missing {placeholder}"));
                }
            }
        }
        out
    }
}

/// Characters that would change the meaning of an OSC address if substituted into one.
fn safe_segment(value: &str) -> bool {
    !value.is_empty()
        && !value
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || "/#*?,[]{}".contains(c))
}

fn label(op: MediaOperation) -> &'static str {
    match op {
        MediaOperation::Start => "start",
        MediaOperation::Stop => "stop",
        MediaOperation::Switch => "switch",
    }
}

fn expand(template: &str, pairs: &[(&str, &str)]) -> Result<String, EndpointError> {
    let mut out = template.to_string();
    for (key, value) in pairs {
        if *key != "operation" && !safe_segment(value) {
            return Err(EndpointError::Rejected(format!(
                "`{value}` cannot be used as an OSC address segment"
            )));
        }
        out = out.replace(&format!("{{{key}}}"), value);
    }
    Ok(out)
}

/// Sends media commands as OSC to one device.
#[derive(Debug)]
pub struct MediaEndpoint {
    socket: UdpSocket,
    templates: MediaTemplates,
}

impl MediaEndpoint {
    /// Opens a socket "connected" to `address` (`host:port`).
    pub fn new(address: &str, templates: Option<MediaTemplates>) -> Result<Self, EndpointError> {
        let socket = UdpSocket::bind("0.0.0.0:0")
            .map_err(|e| EndpointError::Unreachable(format!("bind failed: {e}")))?;
        socket
            .connect(address)
            .map_err(|e| EndpointError::Unreachable(format!("`{address}`: {e}")))?;
        Ok(Self {
            socket,
            templates: templates.unwrap_or_default(),
        })
    }

    /// The OSC address a command maps to.
    pub fn address_for(&self, command: &Command) -> Result<String, EndpointError> {
        match command {
            Command::VideoSource { source, operation } => expand(
                self.templates.video.as_deref().unwrap_or(DEFAULT_VIDEO),
                &[("source", source), ("operation", label(*operation))],
            ),
            Command::AudioRoute {
                from,
                to,
                operation,
            } => expand(
                self.templates.audio.as_deref().unwrap_or(DEFAULT_AUDIO),
                &[("from", from), ("to", to), ("operation", label(*operation))],
            ),
            other => Err(EndpointError::Rejected(format!(
                "`{}` is not a media command",
                other.describe()
            ))),
        }
    }
}

impl Endpoint for MediaEndpoint {
    fn send(&self, command: &Command) -> Result<(), EndpointError> {
        let address = self.address_for(command)?;
        let message =
            OscMessage::new(address, &[]).map_err(|e| EndpointError::Rejected(format!("{e:?}")))?;
        self.socket
            .send(&message.encode())
            .map(|_| ())
            .map_err(|e| EndpointError::Unreachable(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn listener() -> (UdpSocket, String) {
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let addr = socket.local_addr().unwrap().to_string();
        (socket, addr)
    }

    fn received_address(socket: &UdpSocket) -> String {
        let mut buf = [0u8; 512];
        let n = socket.recv(&mut buf).unwrap();
        let end = buf[..n].iter().position(|b| *b == 0).unwrap();
        String::from_utf8(buf[..end].to_vec()).unwrap()
    }

    #[test]
    fn video_commands_arrive_as_osc_at_the_default_address() {
        let (rx, addr) = listener();
        let ep = MediaEndpoint::new(&addr, None).unwrap();
        ep.send(&Command::VideoSource {
            source: "cam1".into(),
            operation: MediaOperation::Switch,
        })
        .unwrap();
        assert_eq!(received_address(&rx), "/media/video/cam1/switch");
    }

    #[test]
    fn audio_routes_use_a_custom_template() {
        let (rx, addr) = listener();
        let templates = MediaTemplates {
            video: None,
            audio: Some("/matrix/{from}/to/{to}/{operation}".into()),
        };
        let ep = MediaEndpoint::new(&addr, Some(templates)).unwrap();
        ep.send(&Command::AudioRoute {
            from: "desk".into(),
            to: "house".into(),
            operation: MediaOperation::Start,
        })
        .unwrap();
        assert_eq!(received_address(&rx), "/matrix/desk/to/house/start");
    }

    #[test]
    fn names_that_would_alter_the_osc_address_are_rejected() {
        let (_rx, addr) = listener();
        let ep = MediaEndpoint::new(&addr, None).unwrap();
        for bad in ["", "a/b", "a b", "cam*", "x{y}"] {
            let err = ep
                .send(&Command::VideoSource {
                    source: bad.into(),
                    operation: MediaOperation::Start,
                })
                .unwrap_err();
            assert!(matches!(err, EndpointError::Rejected(_)), "{bad:?}");
        }
    }

    #[test]
    fn non_media_commands_are_rejected() {
        let (_rx, addr) = listener();
        let ep = MediaEndpoint::new(&addr, None).unwrap();
        let err = ep
            .send(&Command::Osc {
                address: "/x".into(),
                args: vec![],
            })
            .unwrap_err();
        assert!(matches!(err, EndpointError::Rejected(_)));
    }

    #[test]
    fn templates_are_checked() {
        let bad = MediaTemplates {
            video: Some("video/{source}".into()),
            audio: Some("/a/{from}".into()),
        };
        assert_eq!(bad.problems().len(), 2);
        assert!(MediaTemplates::default().problems().is_empty());
    }
}
