//! Shared primitives for TPT AV Automation.
//!
//! This crate is intentionally free of AV/media/protocol knowledge. It provides the identifiers,
//! error taxonomy, injectable clock and normalized [`Event`] type that the deterministic rule core
//! and the application shells (CLI, service, desktop) all depend on.
//!
//! Time is injected rather than read from the operating system, so rule evaluation is reproducible
//! in simulation, golden tests and chaos tests (spec §3.2).

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod error;
mod event;
mod ids;
mod rate_limit;
mod time;

pub use error::{Diagnostic, Error, Result, Severity};
pub use event::{DeviceHealth, DmxLevel, Event, MidiLevel, Protocol};
pub use ids::{slugify_rule_id, ActionId, ConditionId, DeviceId, ExecutionId, RuleId};
pub use rate_limit::{BackoffPolicy, RateLimiter};
pub use time::{Clock, FixedClock, SystemClock, Timestamp};
