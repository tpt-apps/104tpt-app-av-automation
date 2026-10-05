//! Headless service mode: persistence, inbound listeners, heartbeat monitoring, the local API and
//! the watchdog (spec §3.6, §11, §14, §15).

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod api;
pub mod config;
pub mod runtime;
pub mod store;
pub mod ui;
pub mod watchdog;
mod ws;

pub use config::{ApiConfig, ListenerConfig, ServiceConfig};
pub use runtime::{
    ControlReply, ControlRequest, DeviceInfo, RuleInfo, Service, ServiceHandle, Snapshot,
};
pub use store::{PackVersion, Store, StoredIncident};
pub use ui::{CatalogueEntry, CatalogueGroup, UiBridge};
pub use watchdog::{supervise, Outcome, WatchdogEvent, WatchdogPolicy};
