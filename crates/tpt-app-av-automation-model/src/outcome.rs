//! Execution outcomes and the auditable execution record (spec §3.3, §3.4, §6.6).

use std::fmt;

use serde::{Deserialize, Serialize};
use tpt_app_av_automation_core::{ActionId, ConditionId, Event, RuleId};

use crate::action::FailurePolicy;

/// Result of evaluating a condition (spec §6.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConditionResult {
    /// The condition was evaluated and holds.
    Met,
    /// The condition was evaluated and does not hold.
    NotMet,
    /// The condition could not be evaluated (e.g. the device has not reported state).
    ///
    /// A rule must not fire on `Unknown` unless it explicitly opts in (spec §6.3).
    Unknown,
}

impl ConditionResult {
    /// Whether the gate allows the chain to proceed.
    pub fn permits(self, on_unknown: bool) -> bool {
        match self {
            ConditionResult::Met => true,
            ConditionResult::NotMet => false,
            ConditionResult::Unknown => on_unknown,
        }
    }
}

impl fmt::Display for ConditionResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            ConditionResult::Met => "met",
            ConditionResult::NotMet => "not_met",
            ConditionResult::Unknown => "unknown",
        };
        f.write_str(name)
    }
}

/// Outcome of a single action step (spec §6.4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ActionOutcome {
    /// The action completed.
    Success,
    /// The action failed; the message explains why.
    Failed {
        /// Failure detail.
        reason: String,
    },
    /// The action exceeded its bound and was abandoned.
    TimedOut {
        /// The bound that was exceeded, in milliseconds.
        after_ms: u64,
    },
    /// The action was not attempted because a preceding gate or step prevented it.
    Skipped {
        /// Why it was skipped.
        reason: String,
    },
}

impl ActionOutcome {
    /// Whether the step performed real work.
    pub fn is_success(&self) -> bool {
        matches!(self, ActionOutcome::Success)
    }

    /// Whether the step failed outright, as opposed to being skipped.
    pub fn is_failure(&self) -> bool {
        matches!(
            self,
            ActionOutcome::Failed { .. } | ActionOutcome::TimedOut { .. }
        )
    }

    /// Whether the step was skipped without being attempted.
    pub fn is_skipped(&self) -> bool {
        matches!(self, ActionOutcome::Skipped { .. })
    }
}

impl fmt::Display for ActionOutcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ActionOutcome::Success => write!(f, "success"),
            ActionOutcome::Failed { reason } => write!(f, "failed: {reason}"),
            ActionOutcome::TimedOut { after_ms } => write!(f, "timed_out({after_ms}ms)"),
            ActionOutcome::Skipped { reason } => write!(f, "skipped: {reason}"),
        }
    }
}

/// Overall status of one rule execution (spec §6.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionStatus {
    /// Every attempted action succeeded.
    Success,
    /// Some actions succeeded and some failed. Never reported as success (spec §3.4).
    PartialFailure,
    /// Every attempted action failed.
    Failed,
    /// The rule matched but a gate prevented execution.
    Skipped,
}

impl ExecutionStatus {
    /// Folds one more step outcome into the running status.
    ///
    /// This fold is what guarantees a partially failed chain is never reported as a full success
    /// (spec §3.4). Skipped steps are not failures: a chain stopped by policy is a partial
    /// failure only if some earlier step actually failed.
    pub fn fold(self, outcome: &ActionOutcome) -> ExecutionStatus {
        let (succeeded, failed) = match self {
            ExecutionStatus::Success => (1usize, 0usize),
            ExecutionStatus::PartialFailure => (1, 1),
            ExecutionStatus::Failed => (0, 1),
            ExecutionStatus::Skipped => (0, 0),
        };
        let (succeeded, failed) = match outcome {
            ActionOutcome::Success => (succeeded + 1, failed),
            ActionOutcome::Skipped { .. } => (succeeded, failed),
            _ => (succeeded, failed + 1),
        };
        match (succeeded, failed) {
            (0, 0) => ExecutionStatus::Skipped,
            (_, 0) => ExecutionStatus::Success,
            (0, _) => ExecutionStatus::Failed,
            _ => ExecutionStatus::PartialFailure,
        }
    }

    /// Machine-readable lowercase label.
    pub fn as_str(self) -> &'static str {
        match self {
            ExecutionStatus::Success => "success",
            ExecutionStatus::PartialFailure => "partial_failure",
            ExecutionStatus::Failed => "failed",
            ExecutionStatus::Skipped => "skipped",
        }
    }
}

impl fmt::Display for ExecutionStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}
/// Per-action record inside an [`ExecutionRecord`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActionResultRecord {
    /// Which step this was.
    pub action_id: ActionId,
    /// Human-readable action type, e.g. `control.osc`.
    pub action_type: String,
    /// Target device/endpoint, when the action has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    /// What happened.
    pub outcome: ActionOutcome,
    /// Duration in milliseconds.
    pub duration_ms: u64,
    /// Whether the action ran in simulation rather than live.
    pub simulated: bool,
    /// Failure policy in force when this step ran.
    pub failure_policy: FailurePolicy,
    /// What the step did or would do, e.g. `osc /power = 1 -> projector-1`. Always set for
    /// simulated steps, so a simulation explains itself (spec §12.5).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// Per-condition record inside an [`ExecutionRecord`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConditionResultRecord {
    /// Which condition this was.
    pub condition_id: ConditionId,
    /// Evaluated result.
    pub result: ConditionResult,
    /// Whether this result permits the chain to proceed.
    pub permits: bool,
    /// Optional observed value, for explainability.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed: Option<String>,
}

/// A complete, auditable record of one rule execution (spec §3.3, §6.6).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExecutionRecord {
    /// Execution identifier.
    pub execution_id: String,
    /// Rule that ran.
    pub rule_id: RuleId,
    /// Rule name at execution time.
    pub rule_name: String,
    /// Rule version that executed.
    pub rule_version: u32,
    /// When the trigger fired, in milliseconds since the Unix epoch.
    pub triggered_at_ms: u64,
    /// The normalized event that matched the trigger.
    pub trigger_detail: Event,
    /// Which trigger variant matched, e.g. `schedule`.
    pub trigger_type: String,
    /// Per-condition results.
    pub condition_results: Vec<ConditionResultRecord>,
    /// Per-action results, in execution order.
    pub action_results: Vec<ActionResultRecord>,
    /// Overall status.
    pub overall_status: ExecutionStatus,
    /// Whether the whole record was produced without sending real messages.
    pub simulated: bool,
    /// Total wall-clock duration of the chain.
    pub duration_ms: u64,
}

impl ExecutionRecord {
    /// Counts successful steps.
    pub fn actions_succeeded(&self) -> usize {
        self.action_results
            .iter()
            .filter(|r| r.outcome.is_success())
            .count()
    }

    /// Counts failed or timed-out steps. Skips are not failures.
    pub fn actions_failed(&self) -> usize {
        self.action_results
            .iter()
            .filter(|r| r.outcome.is_failure())
            .count()
    }

    /// Counts steps that were never attempted.
    pub fn actions_skipped(&self) -> usize {
        self.action_results
            .iter()
            .filter(|r| r.outcome.is_skipped())
            .count()
    }

    /// Whether any condition returned `Unknown` without permitting the chain.
    pub fn blocked_by_unknown_condition(&self) -> bool {
        self.condition_results
            .iter()
            .any(|c| c.result == ConditionResult::Unknown && !c.permits)
    }
}

impl fmt::Display for ExecutionRecord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} v{} [{}] {} ({} ok / {} failed / {} skipped){}",
            self.rule_name,
            self.rule_version,
            self.overall_status,
            self.trigger_type,
            self.actions_succeeded(),
            self.actions_failed(),
            self.actions_skipped(),
            if self.simulated { " [SIMULATION]" } else { "" }
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fold_all(outcomes: &[ActionOutcome]) -> ExecutionStatus {
        outcomes
            .iter()
            .fold(ExecutionStatus::Skipped, |acc, o| acc.fold(o))
    }

    #[test]
    fn all_success_is_success() {
        assert_eq!(
            fold_all(&[ActionOutcome::Success, ActionOutcome::Success]),
            ExecutionStatus::Success
        );
    }

    #[test]
    fn mixed_outcomes_are_partial_not_success() {
        let status = fold_all(&[
            ActionOutcome::Success,
            ActionOutcome::Failed {
                reason: "no route".into(),
            },
        ]);
        assert_eq!(status, ExecutionStatus::PartialFailure);
    }

    #[test]
    fn all_failed_is_failed() {
        let status = fold_all(&[
            ActionOutcome::Failed { reason: "a".into() },
            ActionOutcome::TimedOut { after_ms: 10 },
        ]);
        assert_eq!(status, ExecutionStatus::Failed);
    }

    #[test]
    fn skipped_steps_do_not_make_a_chain_failed() {
        let status = fold_all(&[
            ActionOutcome::Success,
            ActionOutcome::Skipped {
                reason: "policy".into(),
            },
        ]);
        assert_eq!(status, ExecutionStatus::Success);
    }

    #[test]
    fn condition_result_permission_matrix() {
        assert!(ConditionResult::Met.permits(false));
        assert!(!ConditionResult::NotMet.permits(true));
        assert!(!ConditionResult::Unknown.permits(false));
        assert!(ConditionResult::Unknown.permits(true));
    }
}
