//! Condition definitions and evaluation (spec §6.3, §7).

use serde::{Deserialize, Serialize};
use tpt_app_av_automation_core::{ConditionId, DeviceHealth};

use crate::trigger::approx_eq;

/// Numeric comparison operators used by conditions and parameter triggers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Comparison {
    /// Equal to.
    Eq,
    /// Not equal to.
    Ne,
    /// Strictly greater than.
    Gt,
    /// Greater than or equal.
    Gte,
    /// Strictly less than.
    Lt,
    /// Less than or equal.
    Lte,
}

impl Comparison {
    /// Applies the comparison to two values.
    pub fn evaluate_f64(self, left: f64, right: f64) -> bool {
        match self {
            Comparison::Eq => approx_eq(left, right),
            Comparison::Ne => !approx_eq(left, right),
            Comparison::Gt => left > right,
            Comparison::Gte => left >= right,
            Comparison::Lt => left < right,
            Comparison::Lte => left <= right,
        }
    }

    /// The canonical label.
    pub fn as_str(self) -> &'static str {
        match self {
            Comparison::Eq => "eq",
            Comparison::Ne => "ne",
            Comparison::Gt => "gt",
            Comparison::Gte => "gte",
            Comparison::Lt => "lt",
            Comparison::Lte => "lte",
        }
    }
}

/// What a condition inspects.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[serde(deny_unknown_fields)]
pub enum ConditionSpec {
    /// Device health equals a given state (spec §7.3).
    #[serde(rename = "device_health")]
    DeviceHealth {
        /// Device identifier.
        device: String,
        /// Required health state.
        equals: DeviceHealth,
    },
    /// Current local wall-clock time falls inside a window. Used for venue-hours gating.
    #[serde(rename = "time_window")]
    TimeWindow {
        /// Inclusive start.
        from: crate::trigger::LocalTime,
        /// Exclusive end.
        to: crate::trigger::LocalTime,
    },
    /// A monitored DMX channel satisfies a numeric comparison.
    #[serde(rename = "dmx_channel")]
    DmxChannel {
        /// Universe number.
        universe: u16,
        /// Zero-based channel index.
        channel: u16,
        /// Comparison operator.
        comparison: Comparison,
        /// Right-hand side of the comparison.
        value: f64,
    },
    /// A device-reported parameter satisfies a numeric comparison.
    #[serde(rename = "device_parameter")]
    DeviceParameter {
        /// Device identifier.
        device: String,
        /// Parameter name.
        parameter: String,
        /// Comparison operator.
        comparison: Comparison,
        /// Right-hand side of the comparison.
        value: f64,
    },
    /// Another rule must currently be armed. Used to sequence automations.
    #[serde(rename = "rule_armed")]
    RuleArmed {
        /// Rule identifier.
        rule: String,
    },
    /// A tag must be present on the triggering rule.
    #[serde(rename = "rule_has_tag")]
    RuleHasTag {
        /// Tag to look for.
        tag: String,
    },
}

impl ConditionSpec {
    /// Stable type label used in reports.
    pub fn type_label(&self) -> &'static str {
        match self {
            ConditionSpec::DeviceHealth { .. } => "device_health",
            ConditionSpec::TimeWindow { .. } => "time_window",
            ConditionSpec::DmxChannel { .. } => "dmx_channel",
            ConditionSpec::DeviceParameter { .. } => "device_parameter",
            ConditionSpec::RuleArmed { .. } => "rule_armed",
            ConditionSpec::RuleHasTag { .. } => "rule_has_tag",
        }
    }

    /// Whether this condition needs a rule-id list to evaluate.
    ///
    /// Rule-aware conditions must not be evaluated by the engine's condition crate in isolation;
    /// they are resolved by the engine, which owns the loaded rule set.
    pub fn needs_rule_set(&self) -> bool {
        matches!(
            self,
            ConditionSpec::RuleArmed { .. } | ConditionSpec::RuleHasTag { .. }
        )
    }
}

/// A named gate on a rule.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Condition {
    /// Stable identifier, referenced by execution records.
    #[serde(default)]
    pub id: ConditionId,
    /// Human-readable label for the UI.
    #[serde(default)]
    pub description: String,
    /// What to inspect.
    #[serde(flatten)]
    pub spec: ConditionSpec,
    /// Whether an `Unknown` result permits the chain to proceed.
    ///
    /// Defaults to `false`: a rule must not fire on unknown state unless it opts in explicitly
    /// (spec §6.3).
    #[serde(default)]
    pub fire_on_unknown: bool,
}

impl Condition {
    /// Builds a condition with the default id and strict `Unknown` handling.
    pub fn new(id: impl Into<ConditionId>, spec: ConditionSpec) -> Self {
        Self {
            id: id.into(),
            description: String::new(),
            spec,
            fire_on_unknown: false,
        }
    }

    /// Builds a condition that permits firing on unknown state.
    pub fn lenient(id: impl Into<ConditionId>, spec: ConditionSpec) -> Self {
        Self {
            id: id.into(),
            description: String::new(),
            spec,
            fire_on_unknown: true,
        }
    }

    /// Stable type label.
    pub fn type_label(&self) -> &'static str {
        self.spec.type_label()
    }
}
