//! Device/endpoint registry, health monitoring and outbound endpoints (spec §6.5, §8.1, §11).
//!
//! * [`DeviceRegistry`] tracks health and heartbeats and turns health transitions into
//!   [`tpt_app_av_automation_core::Event`]s, so device health is itself a trigger source.
//! * [`Endpoint`] is the outbound side. [`VirtualEndpoint`] records commands and supports fault
//!   injection (dropped messages, going offline mid-chain) for tests (spec §18.2); [`UdpEndpoint`]
//!   talks OSC, Art-Net and sACN using the `tpt-av-control` codecs; [`MidiEndpoint`] encodes MIDI 1.0
//!   and hands the bytes to any writer.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod command;
pub mod config;
pub mod endpoint;
pub mod midi_port;
pub mod net;
pub mod registry;

pub use command::Command;
pub use config::{DeviceConfig, DeviceFile, SceneDef};
pub use endpoint::{Endpoint, EndpointError, Faults, VirtualEndpoint};
pub use net::{build_endpoint, MidiEndpoint, MidiOpener, MidiWriter, UdpEndpoint, UdpProtocol};
pub use registry::{Device, DeviceKind, DeviceRegistry, ProtocolBinding};
