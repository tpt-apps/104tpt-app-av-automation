//! Rule pack domain model for TPT AV Automation (spec §6, §9).
//!
//! This crate is pure data plus validation. It performs no I/O, no networking and no clock reads,
//! which is what lets the deterministic engine, the CLI and the desktop UI share exactly one
//! implementation of the rule format (spec §3.6).
//!
//! # Example
//!
//! ```
//! use tpt_app_av_automation_model::RulePack;
//!
//! let yaml = r#"
//! format_version: 1
//! name: Main Hall
//! revision: 1
//! rules:
//!   - id: event-start
//!     name: Main Hall - Event Start
//!     version: 1
//!     armed: false
//!     trigger:
//!       type: schedule
//!       at: "18:55"
//!       days: [mon, tue, wed, thu, fri]
//!     actions:
//!       - id: house-lights
//!         type: notify.operator
//!         message: "House lights to half"
//! "#;
//!
//! let pack = RulePack::from_yaml_str(yaml).unwrap();
//! assert_eq!(pack.rules.len(), 1);
//! assert!(!pack.rules[0].armed, "rules load disarmed by default");
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod action;
pub mod condition;
pub mod outcome;
pub mod pack;
pub mod rule;
pub mod trigger;
pub mod validate;

pub use action::{ActionSpec, ActionStep, AlertSeverity, DmxTransport, FailurePolicy, MediaOperation};
pub use condition::{Comparison, Condition, ConditionSpec};
pub use outcome::{
    ActionOutcome, ActionResultRecord, ConditionResult, ConditionResultRecord, ExecutionRecord,
    ExecutionStatus,
};
pub use pack::{FORMAT_VERSION, MAX_CHAIN_LENGTH, RulePack, YamlPackSource};
pub use rule::{Policy, Priority, Rule};
pub use trigger::{DmxComparison, LocalTime, MidiMessageKind, ScheduleSpec, Trigger, TriggerSpec, Weekday};
pub use validate::{cross, steps, validate_pack};

pub use tpt_app_av_automation_core::{Diagnostic, Error, Result, Severity};

/// Whether a diagnostic list contains at least one error.
pub fn has_errors(diagnostics: &[Diagnostic]) -> bool {
    diagnostics.iter().any(|d| d.severity == Severity::Error)
}