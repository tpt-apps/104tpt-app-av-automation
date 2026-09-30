//! Rule identity, priority and chain policy (spec §6.1).

use std::fmt;

use serde::{Deserialize, Serialize};
use tpt_app_av_automation_core::{ActionId, RuleId, slugify_rule_id};

use crate::action::{ActionStep, FailurePolicy};
use crate::condition::Condition;
use crate::trigger::Trigger;

/// Coarse execution priority used for deterministic conflict resolution (spec §10.1).
///
/// Higher priority wins. Conflicts between equal-priority rules are resolved by rule id so the
/// ordering is total and reproducible across runs and platforms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Priority {
    /// Runs before normal rules.
    Critical,
    /// The default priority.
    Normal,
    /// Runs after normal rules; never overrides a critical/normal rule for the same endpoint.
    Low,
    /// Informational only.
    Informational,
}

impl Default for Priority {
    fn default() -> Self {
        Priority::Normal
    }
}

impl fmt::Display for Priority {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Priority::Critical => "critical",
            Priority::Normal => "normal",
            Priority::Low => "low",
            Priority::Informational => "informational",
        };
        f.write_str(name)
    }
}

/// Chain-level behaviour (spec §6.4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Policy {
    /// What to do when an action step fails.
    #[serde(default)]
    pub on_failure: FailurePolicy,
    /// Optional fallback chain, used by [`FailurePolicy::RunFallback`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fallback: Vec<ActionStep>,
    /// Overall wall-clock budget for the chain, in milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            on_failure: FailurePolicy::default(),
            fallback: Vec::new(),
            timeout_ms: None,
        }
    }
}

impl Policy {
    /// Every step in the chain: main first, then fallback.
    pub fn all_steps<'a>(&'a self, main: &'a [ActionStep]) -> impl Iterator<Item = &'a ActionStep> {
        main.iter().chain(self.fallback.iter())
    }
}
/// A single automation rule (spec §6.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Rule {
    /// Stable identifier. Defaults to a slug of [`Rule::name`] when omitted.
    #[serde(default)]
    pub id: RuleId,
    /// Human-readable name.
    pub name: String,
    /// Optional free-form description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Rule version; recorded in every execution event (spec §3.3).
    #[serde(default = "default_rule_version")]
    pub version: u32,
    /// What causes the rule to be considered.
    pub trigger: Trigger,
    /// Extra gates. All must be satisfied.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conditions: Vec<Condition>,
    /// Ordered action chain.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<ActionStep>,
    /// Chain-level failure and timeout behaviour.
    #[serde(default)]
    pub policy: Policy,
    /// Conflict-resolution priority.
    #[serde(default)]
    pub priority: Priority,
    /// Whether the rule may send real actions.
    ///
    /// Defaults to `false`: new or edited rules are simulation-only until an operator explicitly
    /// arms them (spec §3.5).
    #[serde(default)]
    pub armed: bool,
    /// Free-form tags for filtering in the UI and CLI.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// Whether a rule that already fired for a given event may fire again.
    #[serde(default)]
    pub reentrant: bool,
}

fn default_rule_version() -> u32 {
    1
}

impl Rule {
    /// Builds a rule with defaults.
    pub fn new(id: impl Into<RuleId>, name: impl Into<String>, trigger: Trigger) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            description: None,
            version: 1,
            trigger,
            conditions: Vec::new(),
            actions: Vec::new(),
            policy: Policy::default(),
            priority: Priority::Normal,
            armed: false,
            tags: Vec::new(),
            reentrant: false,
        }
    }

    /// Derives an id from the rule name, for callers that omit one.
    pub fn with_derived_id(mut self) -> Self {
        self.id = RuleId::new(slugify_rule_id(&self.name));
        self
    }

    /// Whether this rule is allowed to perform real side effects.
    pub fn is_live(&self) -> bool {
        self.armed
    }

    /// Short label for logs and reports.
    pub fn label(&self) -> String {
        format!("{} (v{})", self.name, self.version)
    }

    /// Finds a condition by id.
    pub fn condition(&self, id: &str) -> Option<&Condition> {
        self.conditions.iter().find(|c| c.id.as_str() == id)
    }

    /// Finds an action step by id in the main chain.
    pub fn action(&self, id: &str) -> Option<&ActionStep> {
        self.actions.iter().find(|s| s.id.as_str() == id)
    }

    /// Whether this rule writes to the given device in its main or fallback chain.
    pub fn targets_device(&self, device: &str) -> bool {
        self.policy
            .all_steps(&self.actions)
            .any(|s| s.target() == Some(device))
    }

    /// Every device this rule writes to, in chain order and deduplicated.
    pub fn targeted_devices(&self) -> Vec<String> {
        let mut seen: Vec<String> = Vec::new();
        for step in self.policy.all_steps(&self.actions) {
            if let Some(device) = step.target() {
                if !seen.iter().any(|d| d == device) {
                    seen.push(device.to_owned());
                }
            }
        }
        seen
    }

    /// Total steps in the main chain.
    pub fn action_count(&self) -> usize {
        self.actions.len()
    }

    /// Ids of every action step, used by explainability output.
    pub fn action_ids(&self) -> Vec<ActionId> {
        self.actions.iter().map(|s| s.id.clone()).collect()
    }

    /// Whether the rule carries the given tag.
    pub fn has_tag(&self, tag: &str) -> bool {
        self.tags.iter().any(|t| t == tag)
    }
}

impl fmt::Display for Rule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} [{}] v{} ({}, {})",
            self.name,
            self.id,
            self.version,
            self.priority,
            if self.armed { "armed" } else { "disarmed" }
        )
    }
}