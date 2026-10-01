//! Turning execution records into something people and scripts can use (spec §12.4, §12.5, §13).
//!
//! * [`render_record`] — the human text. A simulation and a live execution are impossible to
//!   confuse: the first line of each says which it is (spec §16).
//! * [`machine_result`] — the stable JSON contract for scripts (spec §13).
//! * [`exit_code`] / [`ExitCode`] — the stable process exit-code contract (spec §13).
//! * [`Filter`] — the execution log filters (spec §12.4).

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use std::fmt::Write as _;

use serde_json::{json, Value};
use tpt_app_av_automation_core::{Error, Event};
use tpt_app_av_automation_model::{
    ActionOutcome, ConditionResult, ExecutionRecord, ExecutionStatus,
};

/// Process exit codes. **Stable across versions** (spec §13).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ExitCode {
    /// Every attempted action succeeded.
    Success = 0,
    /// Some actions succeeded and some failed.
    PartialFailure = 1,
    /// The execution failed.
    Failed = 2,
    /// Nothing ran: no rule matched or a condition was not met.
    Skipped = 3,
    /// The rule pack, device file or arguments were invalid.
    ConfigurationError = 4,
    /// A device or endpoint could not be reached or opened.
    DeviceError = 5,
    /// An internal error or storage failure.
    InternalError = 6,
}

impl ExitCode {
    /// The numeric process exit status.
    pub fn code(self) -> i32 {
        i32::from(self as u8)
    }

    /// Maps an error to its exit code.
    pub fn from_error(error: &Error) -> ExitCode {
        match error {
            Error::Parse(_)
            | Error::Validation(_)
            | Error::NotFound { .. }
            | Error::DuplicateRuleId(_)
            | Error::InvalidOperation(_)
            | Error::Io(_) => ExitCode::ConfigurationError,
            Error::Control(_) | Error::MalformedMessage(_) | Error::Timeout(_) => {
                ExitCode::DeviceError
            }
            Error::Subprocess(_) | Error::Storage(_) | Error::Cancelled | Error::Internal(_) => {
                ExitCode::InternalError
            }
        }
    }
}

/// The exit code for a set of executions: the worst outcome wins, and "nothing ran" is `Skipped`.
pub fn exit_code(records: &[ExecutionRecord]) -> ExitCode {
    let any = |status| records.iter().any(|r| r.overall_status == status);
    if any(ExecutionStatus::Failed) {
        ExitCode::Failed
    } else if any(ExecutionStatus::PartialFailure) {
        ExitCode::PartialFailure
    } else if any(ExecutionStatus::Success) {
        ExitCode::Success
    } else {
        ExitCode::Skipped
    }
}

/// The machine-readable result of one execution (spec §13).
///
/// `rule`, `status`, `actions_executed` and `actions_failed` are the contract; the remaining fields
/// are additive. `actions_executed` counts attempted steps (succeeded + failed), not skipped ones.
pub fn machine_result(record: &ExecutionRecord) -> Value {
    json!({
        "rule": record.rule_name,
        "rule_id": record.rule_id,
        "rule_version": record.rule_version,
        "execution_id": record.execution_id,
        "status": record.overall_status.as_str(),
        "simulated": record.simulated,
        "actions_executed": record.actions_succeeded() + record.actions_failed(),
        "actions_failed": record.actions_failed(),
        "actions_skipped": record.actions_skipped(),
    })
}

/// A short description of an event, e.g. `osc /cue/1 [1, 0.5]`.
pub fn describe_event(event: &Event) -> String {
    match event {
        Event::Schedule { spec } => format!("schedule {spec}"),
        Event::Osc { address, args } if args.is_empty() => format!("osc {address}"),
        Event::Osc { address, args } => {
            let args: Vec<String> = args.iter().map(f64::to_string).collect();
            format!("osc {address} [{}]", args.join(", "))
        }
        Event::Midi(m) => format!("midi {} ch{} #{} = {}", m.kind, m.channel, m.number, m.value),
        Event::Dmx(d) => format!("dmx universe {} channel {} = {}", d.universe, d.channel, d.value),
        Event::DeviceState {
            device,
            previous,
            current,
        } => format!("device {device}: {previous} -> {current}"),
        Event::HeartbeatMissed {
            device,
            missed_millis,
        } => format!("heartbeat missed on {device} for {missed_millis}ms"),
        Event::DeviceParameter {
            device,
            parameter,
            value,
        } => format!("device {device} {parameter} = {value}"),
        Event::Manual { rule: Some(rule) } => format!("manual run of `{rule}`"),
        Event::Manual { rule: None } => "manual".to_string(),
        Event::Api { name, .. } => format!("api {name}"),
    }
}

fn outcome_tag(outcome: &ActionOutcome) -> &'static str {
    match outcome {
        ActionOutcome::Success => "ok",
        ActionOutcome::Failed { .. } => "FAILED",
        ActionOutcome::TimedOut { .. } => "TIMED OUT",
        ActionOutcome::Skipped { .. } => "skipped",
    }
}

/// Renders one execution for a person.
pub fn render_record(record: &ExecutionRecord) -> String {
    let mut out = String::new();
    let _ = if record.simulated {
        writeln!(out, "SIMULATION — no live actions were sent")
    } else {
        writeln!(out, "LIVE — actions were sent to devices")
    };
    let _ = writeln!(out);
    let _ = writeln!(out, "Rule: \"{}\" (v{}, {})", record.rule_name, record.rule_version, record.execution_id);
    let _ = writeln!(
        out,
        "Trigger: {} ({})",
        record.trigger_type,
        describe_event(&record.trigger_detail)
    );

    if record.condition_results.is_empty() {
        let verdict = if record.simulated { "would have fired" } else { "fired" };
        let _ = writeln!(out, "Conditions: none configured — {verdict}");
    } else {
        let _ = writeln!(out, "Conditions:");
        for c in &record.condition_results {
            let result = match c.result {
                ConditionResult::Met => "met",
                ConditionResult::NotMet => "NOT MET",
                ConditionResult::Unknown => "UNKNOWN",
            };
            let observed = c.observed.as_deref().unwrap_or("");
            let blocked = if c.permits { "" } else { "  <- blocked the rule" };
            let _ = writeln!(out, "  [{result}] {}: {observed}{blocked}", c.condition_id);
        }
    }

    if record.overall_status == ExecutionStatus::Skipped && record.action_results.is_empty() {
        let _ = writeln!(out);
        let _ = writeln!(out, "Result: skipped — the rule did not run");
        return out;
    }

    let _ = writeln!(out);
    let _ = writeln!(out, "{}:", if record.simulated { "Would execute" } else { "Executed" });
    for (i, step) in record.action_results.iter().enumerate() {
        let what = step.detail.clone().unwrap_or_else(|| step.action_type.clone());
        if record.simulated && step.outcome.is_success() {
            let _ = writeln!(out, "  {}. {what}", i + 1);
        } else {
            let extra = match &step.outcome {
                ActionOutcome::Failed { reason } | ActionOutcome::Skipped { reason } => format!(": {reason}"),
                ActionOutcome::TimedOut { after_ms } => format!(" after {after_ms}ms"),
                ActionOutcome::Success => String::new(),
            };
            let _ = writeln!(out, "  {}. {what} [{}{extra}]", i + 1, outcome_tag(&step.outcome));
        }
    }
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "Result: {} ({} ok, {} failed, {} skipped)",
        record.overall_status,
        record.actions_succeeded(),
        record.actions_failed(),
        record.actions_skipped()
    );
    out
}

/// Execution log filters (spec §12.4). Every set field must match.
#[derive(Debug, Clone, Default)]
pub struct Filter {
    /// Only this rule id.
    pub rule: Option<String>,
    /// Only this overall status.
    pub status: Option<ExecutionStatus>,
    /// Only executions that targeted this device.
    pub device: Option<String>,
    /// Only executions at or after this time (epoch ms).
    pub since_ms: Option<u64>,
    /// Only executions before this time (epoch ms).
    pub until_ms: Option<u64>,
    /// Only simulated (`Some(true)`) or only live (`Some(false)`) executions.
    pub simulated: Option<bool>,
}

impl Filter {
    /// Whether a record passes every filter.
    pub fn matches(&self, record: &ExecutionRecord) -> bool {
        self.rule.as_deref().is_none_or(|r| record.rule_id.as_str() == r)
            && self.status.is_none_or(|s| record.overall_status == s)
            && self.device.as_deref().is_none_or(|d| {
                record
                    .action_results
                    .iter()
                    .any(|a| a.device.as_deref() == Some(d))
            })
            && self.since_ms.is_none_or(|t| record.triggered_at_ms >= t)
            && self.until_ms.is_none_or(|t| record.triggered_at_ms < t)
            && self.simulated.is_none_or(|s| record.simulated == s)
    }

    /// Applies the filter to a sequence of records.
    pub fn apply<'a>(&self, records: impl IntoIterator<Item = &'a ExecutionRecord>) -> Vec<&'a ExecutionRecord> {
        records.into_iter().filter(|r| self.matches(r)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tpt_app_av_automation_core::RuleId;
    use tpt_app_av_automation_model::{ActionResultRecord, ConditionResultRecord, FailurePolicy};

    fn step(id: &str, device: Option<&str>, outcome: ActionOutcome, simulated: bool) -> ActionResultRecord {
        ActionResultRecord {
            action_id: id.into(),
            action_type: "control.osc".into(),
            device: device.map(str::to_owned),
            outcome,
            duration_ms: 1,
            simulated,
            failure_policy: FailurePolicy::StopChain,
            detail: Some(format!("osc /{id} = 1")),
        }
    }

    fn record(status: ExecutionStatus, simulated: bool, steps: Vec<ActionResultRecord>) -> ExecutionRecord {
        ExecutionRecord {
            execution_id: "exec-000001".into(),
            rule_id: RuleId::new("main"),
            rule_name: "Main Display Failover".into(),
            rule_version: 2,
            triggered_at_ms: 1_000,
            trigger_detail: Event::Osc {
                address: "/go".into(),
                args: vec![1.0],
            },
            trigger_type: "osc".into(),
            condition_results: vec![],
            action_results: steps,
            overall_status: status,
            simulated,
            duration_ms: 5,
        }
    }

    #[test]
    fn exit_codes_are_the_contract() {
        assert_eq!(ExitCode::Success.code(), 0);
        assert_eq!(ExitCode::PartialFailure.code(), 1);
        assert_eq!(ExitCode::Failed.code(), 2);
        assert_eq!(ExitCode::Skipped.code(), 3);
        assert_eq!(ExitCode::ConfigurationError.code(), 4);
        assert_eq!(ExitCode::DeviceError.code(), 5);
        assert_eq!(ExitCode::InternalError.code(), 6);
    }

    #[test]
    fn worst_outcome_wins_and_nothing_ran_is_skipped() {
        let ok = record(ExecutionStatus::Success, false, vec![]);
        let partial = record(ExecutionStatus::PartialFailure, false, vec![]);
        let failed = record(ExecutionStatus::Failed, false, vec![]);
        let skipped = record(ExecutionStatus::Skipped, false, vec![]);
        assert_eq!(exit_code(&[]), ExitCode::Skipped);
        assert_eq!(exit_code(std::slice::from_ref(&skipped)), ExitCode::Skipped);
        assert_eq!(exit_code(&[ok.clone(), skipped]), ExitCode::Success);
        assert_eq!(exit_code(&[ok.clone(), partial.clone()]), ExitCode::PartialFailure);
        assert_eq!(exit_code(&[ok, partial, failed]), ExitCode::Failed);
    }

    #[test]
    fn errors_map_to_configuration_device_or_internal() {
        assert_eq!(ExitCode::from_error(&Error::Parse("x".into())), ExitCode::ConfigurationError);
        assert_eq!(ExitCode::from_error(&Error::Io("x".into())), ExitCode::ConfigurationError);
        assert_eq!(ExitCode::from_error(&Error::Control("x".into())), ExitCode::DeviceError);
        assert_eq!(ExitCode::from_error(&Error::Storage("x".into())), ExitCode::InternalError);
        assert_eq!(ExitCode::from_error(&Error::Internal("x".into())), ExitCode::InternalError);
    }

    #[test]
    fn machine_result_carries_the_contract_fields() {
        let r = record(
            ExecutionStatus::PartialFailure,
            false,
            vec![
                step("a", Some("d"), ActionOutcome::Success, false),
                step("b", Some("d"), ActionOutcome::Failed { reason: "x".into() }, false),
                step("c", None, ActionOutcome::Skipped { reason: "y".into() }, false),
            ],
        );
        let v = machine_result(&r);
        assert_eq!(v["rule"], "Main Display Failover");
        assert_eq!(v["status"], "partial_failure");
        assert_eq!(v["actions_executed"], 2);
        assert_eq!(v["actions_failed"], 1);
        assert_eq!(v["actions_skipped"], 1);
        assert_eq!(v["simulated"], false);
    }

    #[test]
    fn simulation_output_follows_the_spec_layout() {
        let r = record(
            ExecutionStatus::Success,
            true,
            vec![
                step("switch", Some("switcher"), ActionOutcome::Success, true),
                step("alert", None, ActionOutcome::Success, true),
            ],
        );
        let text = render_record(&r);
        assert!(text.starts_with("SIMULATION — no live actions were sent"));
        assert!(text.contains("Rule: \"Main Display Failover\""));
        assert!(text.contains("Trigger: osc (osc /go [1])"));
        assert!(text.contains("Conditions: none configured — would have fired"));
        assert!(text.contains("Would execute:\n  1. osc /switch = 1\n  2. osc /alert = 1"));
    }

    #[test]
    fn live_output_is_unmistakable_and_shows_failures() {
        let r = record(
            ExecutionStatus::PartialFailure,
            false,
            vec![
                step("a", Some("d"), ActionOutcome::Success, false),
                step("b", Some("d"), ActionOutcome::TimedOut { after_ms: 50 }, false),
            ],
        );
        let text = render_record(&r);
        assert!(text.starts_with("LIVE"));
        assert!(text.contains("[TIMED OUT after 50ms]"));
        assert!(text.contains("Result: partial_failure (1 ok, 1 failed, 0 skipped)"));
    }

    #[test]
    fn blocked_rules_explain_which_condition_stopped_them() {
        let mut r = record(ExecutionStatus::Skipped, true, vec![]);
        r.condition_results.push(ConditionResultRecord {
            condition_id: "hours".into(),
            result: ConditionResult::NotMet,
            permits: false,
            observed: Some("local time 10:00".into()),
        });
        let text = render_record(&r);
        assert!(text.contains("[NOT MET] hours: local time 10:00  <- blocked the rule"));
        assert!(text.contains("Result: skipped"));
    }

    #[test]
    fn filters_combine_with_and_semantics() {
        let a = record(
            ExecutionStatus::Success,
            false,
            vec![step("a", Some("proj"), ActionOutcome::Success, false)],
        );
        let mut b = record(ExecutionStatus::Failed, true, vec![]);
        b.rule_id = RuleId::new("other");
        b.triggered_at_ms = 5_000;
        let all = [a.clone(), b.clone()];
        assert_eq!(Filter::default().apply(&all).len(), 2);
        assert_eq!(Filter { rule: Some("main".into()), ..Filter::default() }.apply(&all).len(), 1);
        assert_eq!(Filter { status: Some(ExecutionStatus::Failed), ..Filter::default() }.apply(&all).len(), 1);
        assert_eq!(Filter { device: Some("proj".into()), ..Filter::default() }.apply(&all).len(), 1);
        assert_eq!(Filter { since_ms: Some(2_000), ..Filter::default() }.apply(&all).len(), 1);
        assert_eq!(Filter { until_ms: Some(2_000), ..Filter::default() }.apply(&all).len(), 1);
        assert_eq!(Filter { simulated: Some(true), ..Filter::default() }.apply(&all).len(), 1);
        assert_eq!(
            Filter {
                rule: Some("main".into()),
                status: Some(ExecutionStatus::Failed),
                ..Filter::default()
            }
            .apply(&all)
            .len(),
            0
        );
    }
}
