//! The device/endpoint registry and heartbeat monitoring (spec §6.5, §11).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use tpt_app_av_automation_core::{
    DeviceHealth, DeviceId, DmxLevel, Error, Event, Result, Timestamp,
};

/// Coarse device category, used for display and filtering only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceKind {
    /// Lighting node/console/rack.
    Lighting,
    /// Projector or display.
    Display,
    /// Audio mixer, DSP or router.
    Audio,
    /// Video switcher or router.
    Switcher,
    /// Media server or player.
    Media,
    /// Anything else.
    #[default]
    Other,
}

/// How a device is reached.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ProtocolBinding {
    /// Protocol name: `osc`, `artnet`, `sacn`, `midi` or `virtual`.
    pub protocol: String,
    /// Network address or port name, when applicable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
}

/// A registered device (spec §6.5).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Device {
    /// Identifier referenced from rules.
    pub id: DeviceId,
    /// Display name.
    pub name: String,
    /// Category.
    pub kind: DeviceKind,
    /// Transport binding.
    pub protocol: ProtocolBinding,
    /// Last time the device was heard from.
    pub last_seen: Option<Timestamp>,
    /// Current health.
    pub health: DeviceHealth,
    /// Heartbeat deadline; `None` disables heartbeat monitoring for this device.
    #[serde(default)]
    pub heartbeat_ms: Option<u64>,
}

impl Device {
    /// Builds a device with unknown health.
    pub fn new(id: impl Into<DeviceId>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            kind: DeviceKind::default(),
            protocol: ProtocolBinding::default(),
            last_seen: None,
            health: DeviceHealth::Unknown,
            heartbeat_ms: None,
        }
    }
}

/// Registry of devices plus the last observed parameter and DMX state.
///
/// Every mutating method returns the [`Event`]s it caused; the caller feeds them to the engine. The
/// registry never reads the clock itself — time is always passed in (spec §3.2).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DeviceRegistry {
    devices: BTreeMap<String, Device>,
    #[serde(skip)]
    parameters: BTreeMap<(String, String), f64>,
    #[serde(skip)]
    dmx: BTreeMap<(u16, u16), u8>,
}

impl DeviceRegistry {
    /// An empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers or replaces a device.
    pub fn register(&mut self, device: Device) {
        self.devices.insert(device.id.as_str().to_owned(), device);
    }

    /// Looks a device up.
    pub fn get(&self, id: &str) -> Option<&Device> {
        self.devices.get(id)
    }

    /// Whether the device exists.
    pub fn contains(&self, id: &str) -> bool {
        self.devices.contains_key(id)
    }

    /// Devices ordered by id.
    pub fn devices(&self) -> impl Iterator<Item = &Device> {
        self.devices.values()
    }

    /// Number of registered devices.
    pub fn len(&self) -> usize {
        self.devices.len()
    }

    /// Whether the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.devices.is_empty()
    }

    /// Current health of a device, `None` when it is not registered.
    pub fn health(&self, id: &str) -> Option<DeviceHealth> {
        self.devices.get(id).map(|d| d.health)
    }

    fn not_found(id: &str) -> Error {
        Error::NotFound {
            kind: "device",
            id: id.to_owned(),
        }
    }

    /// Changes health, returning the transition event when the state actually changed.
    pub fn set_health(&mut self, id: &str, health: DeviceHealth) -> Result<Option<Event>> {
        let device = self
            .devices
            .get_mut(id)
            .ok_or_else(|| Self::not_found(id))?;
        if device.health == health {
            return Ok(None);
        }
        let previous = device.health;
        device.health = health;
        Ok(Some(Event::DeviceState {
            device: id.to_owned(),
            previous,
            current: health,
        }))
    }

    /// Records a heartbeat: refreshes `last_seen` and brings the device online.
    pub fn heartbeat(&mut self, id: &str, now: Timestamp) -> Result<Vec<Event>> {
        let device = self
            .devices
            .get_mut(id)
            .ok_or_else(|| Self::not_found(id))?;
        device.last_seen = Some(now);
        let mut events = Vec::new();
        if device.health != DeviceHealth::Online {
            let previous = device.health;
            device.health = DeviceHealth::Online;
            events.push(Event::DeviceState {
                device: id.to_owned(),
                previous,
                current: DeviceHealth::Online,
            });
        }
        Ok(events)
    }

    /// Finds devices whose heartbeat deadline has passed and marks them offline.
    ///
    /// A device that has never been heard from stays `Unknown`: there is no baseline to have
    /// missed. The result is ordered by device id so the event order is deterministic.
    pub fn check_heartbeats(&mut self, now: Timestamp) -> Vec<Event> {
        let mut events = Vec::new();
        for (id, device) in self.devices.iter_mut() {
            let (Some(deadline), Some(seen)) = (device.heartbeat_ms, device.last_seen) else {
                continue;
            };
            let silent_for = now.millis_since(seen);
            if silent_for > deadline && device.health != DeviceHealth::Offline {
                let previous = device.health;
                device.health = DeviceHealth::Offline;
                events.push(Event::HeartbeatMissed {
                    device: id.clone(),
                    missed_millis: silent_for,
                });
                events.push(Event::DeviceState {
                    device: id.clone(),
                    previous,
                    current: DeviceHealth::Offline,
                });
            }
        }
        events
    }

    /// Records a device-reported parameter and returns the matching event.
    pub fn report_parameter(&mut self, id: &str, parameter: &str, value: f64) -> Result<Event> {
        if !self.devices.contains_key(id) {
            return Err(Self::not_found(id));
        }
        self.parameters
            .insert((id.to_owned(), parameter.to_owned()), value);
        Ok(Event::DeviceParameter {
            device: id.to_owned(),
            parameter: parameter.to_owned(),
            value,
        })
    }

    /// Last reported value of a parameter.
    pub fn parameter(&self, id: &str, parameter: &str) -> Option<f64> {
        self.parameters
            .get(&(id.to_owned(), parameter.to_owned()))
            .copied()
    }

    /// Records a DMX observation and returns the previous value for that channel, if any.
    pub fn observe_dmx(&mut self, level: DmxLevel) -> Option<u8> {
        self.dmx
            .insert((level.universe, level.channel), level.value)
    }

    /// Last observed value of a DMX channel.
    pub fn dmx_value(&self, universe: u16, channel: u16) -> Option<u8> {
        self.dmx.get(&(universe, channel)).copied()
    }

    /// Every DMX channel observed so far, keyed by `(universe, channel)`.
    pub fn dmx_snapshot(&self) -> BTreeMap<(u16, u16), u8> {
        self.dmx.clone()
    }

    /// Every reported parameter, keyed by `(device, parameter)`.
    pub fn parameter_snapshot(&self) -> BTreeMap<(String, String), f64> {
        self.parameters.clone()
    }

    /// Overall system status summary, for dashboards and the CLI: worst health wins.
    pub fn worst_health(&self) -> DeviceHealth {
        self.devices
            .values()
            .map(|d| d.health)
            .filter(|h| *h != DeviceHealth::Unknown)
            .max()
            .unwrap_or(DeviceHealth::Unknown)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry() -> DeviceRegistry {
        let mut r = DeviceRegistry::new();
        let mut d = Device::new("proj", "Projector");
        d.heartbeat_ms = Some(1_000);
        r.register(d);
        r
    }

    #[test]
    fn heartbeat_brings_device_online_and_emits_transition() {
        let mut r = registry();
        let events = r.heartbeat("proj", Timestamp::from_millis(100)).unwrap();
        assert_eq!(
            events,
            vec![Event::DeviceState {
                device: "proj".into(),
                previous: DeviceHealth::Unknown,
                current: DeviceHealth::Online
            }]
        );
        assert!(r
            .heartbeat("proj", Timestamp::from_millis(200))
            .unwrap()
            .is_empty());
    }

    #[test]
    fn missed_heartbeat_goes_offline_exactly_once() {
        let mut r = registry();
        r.heartbeat("proj", Timestamp::from_millis(0)).unwrap();
        assert!(r.check_heartbeats(Timestamp::from_millis(1_000)).is_empty());
        let events = r.check_heartbeats(Timestamp::from_millis(1_500));
        assert_eq!(events.len(), 2);
        assert!(matches!(
            events[0],
            Event::HeartbeatMissed {
                missed_millis: 1_500,
                ..
            }
        ));
        assert_eq!(r.health("proj"), Some(DeviceHealth::Offline));
        assert!(r.check_heartbeats(Timestamp::from_millis(9_000)).is_empty());
    }

    #[test]
    fn never_seen_devices_stay_unknown() {
        let mut r = registry();
        assert!(r
            .check_heartbeats(Timestamp::from_millis(99_999))
            .is_empty());
        assert_eq!(r.health("proj"), Some(DeviceHealth::Unknown));
    }

    #[test]
    fn unknown_device_is_an_error() {
        let mut r = registry();
        assert!(matches!(
            r.heartbeat("nope", Timestamp::from_millis(0)),
            Err(Error::NotFound { .. })
        ));
        assert!(r.set_health("nope", DeviceHealth::Online).is_err());
    }

    #[test]
    fn set_health_is_idempotent() {
        let mut r = registry();
        assert!(r
            .set_health("proj", DeviceHealth::Degraded)
            .unwrap()
            .is_some());
        assert!(r
            .set_health("proj", DeviceHealth::Degraded)
            .unwrap()
            .is_none());
        assert_eq!(r.worst_health(), DeviceHealth::Degraded);
    }

    #[test]
    fn dmx_observation_returns_previous_value() {
        let mut r = registry();
        let lvl = DmxLevel {
            universe: 1,
            channel: 4,
            value: 10,
        };
        assert_eq!(r.observe_dmx(lvl), None);
        assert_eq!(r.observe_dmx(DmxLevel { value: 99, ..lvl }), Some(10));
        assert_eq!(r.dmx_value(1, 4), Some(99));
    }

    #[test]
    fn parameters_round_trip() {
        let mut r = registry();
        r.report_parameter("proj", "lamp_hours", 120.0).unwrap();
        assert_eq!(r.parameter("proj", "lamp_hours"), Some(120.0));
        assert!(r.report_parameter("nope", "x", 1.0).is_err());
    }
}
