//! Trigger sources: the schedule driver and the inbound control-message gate (spec §7, §16).
//!
//! Matching a normalized [`tpt_app_av_automation_core::Event`] against a rule's trigger is a pure
//! function that lives in the model crate. This crate produces those events:
//!
//! * [`Scheduler`] turns the passage of time into schedule events. It is edge-triggered,
//!   idempotent within a minute, and its [`SchedulerState`] can be persisted so a restart never
//!   re-fires a one-shot (spec §11).
//! * [`Inbound`] parses and validates raw OSC, MIDI, Art-Net and sACN datagrams — all of which are
//!   unauthenticated and therefore untrusted (spec §16) — and rate limits each source.
//!
//! Foundation crates used: `tpt-av-control-osc`, `tpt-av-control-midi`, `tpt-av-control-dmx`.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod inbound;
pub mod scheduler;

pub use inbound::{DmxTracker, Inbound, InboundLimits};
pub use scheduler::{LocalClock, LocalMoment, Scheduler, SchedulerState};
