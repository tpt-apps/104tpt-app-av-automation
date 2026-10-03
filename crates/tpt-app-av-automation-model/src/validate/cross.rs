//! Cross-rule validation: library fragments and `invoke_rule` references.

use std::collections::{HashMap, HashSet};

use tpt_app_av_automation_core::Diagnostic;

use crate::action::ActionSpec;
use crate::condition::ConditionSpec;
use crate::pack::RulePack;
use crate::rule::Rule;

use super::steps::validate_action_spec;

/// Validates references that span more than one rule.
pub fn validate_cross_references(pack: &RulePack, diagnostics: &mut Vec<Diagnostic>) {
    validate_library(pack, diagnostics);
    validate_invocations(pack, diagnostics);
    validate_rule_condition_references(pack, diagnostics);
    diagnostics.extend(detect_invocation_cycles(pack));
}

/// Validates `rule_armed` conditions against the pack's rule ids.
///
/// A `rule_armed` condition that names a rule which does not exist can never be met, so the rule
/// holding it would sit armed and never fire. That is the same class of mistake as an
/// `invoke_rule` naming a missing rule, and it is caught here rather than at runtime.
fn validate_rule_condition_references(pack: &RulePack, diagnostics: &mut Vec<Diagnostic>) {
    let known: HashSet<&str> = pack.rules.iter().map(|r| r.id.as_str()).collect();
    for (index, rule) in pack.rules.iter().enumerate() {
        for (position, condition) in rule.conditions.iter().enumerate() {
            if let ConditionSpec::RuleArmed { rule: target } = &condition.spec {
                if !known.contains(target.as_str()) {
                    diagnostics.push(Diagnostic::error(
                        format!("rules[{index}].conditions[{position}].rule"),
                        format!(
                            "rule_armed targets unknown rule `{target}`, so this condition can never be met"
                        ),
                    ));
                }
            }
        }
    }
}

fn validate_library(pack: &RulePack, diagnostics: &mut Vec<Diagnostic>) {
    for (key, spec) in &pack.action_library {
        let loc = format!("action_library.{key}");
        if matches!(spec, ActionSpec::UseAction { .. }) {
            diagnostics.push(Diagnostic::error(
                &loc,
                "library fragments must not reference other fragments",
            ));
        }
        validate_action_spec(spec, &loc, diagnostics);
    }
}

fn validate_invocations(pack: &RulePack, diagnostics: &mut Vec<Diagnostic>) {
    let known: HashSet<&str> = pack.rules.iter().map(|r| r.id.as_str()).collect();

    for (index, rule) in pack.rules.iter().enumerate() {
        for step in rule.policy.all_steps(&rule.actions) {
            match &step.spec {
                ActionSpec::UseAction { library } => {
                    if !pack.action_library.contains_key(library) {
                        diagnostics.push(Diagnostic::error(
                            format!("rules[{index}].actions"),
                            format!("unknown action library fragment `{library}`"),
                        ));
                    }
                }
                ActionSpec::InvokeRule { rule: target } if !known.contains(target.as_str()) => {
                    diagnostics.push(Diagnostic::error(
                        format!("rules[{index}].actions"),
                        format!("invoke_rule targets unknown rule `{target}`"),
                    ));
                }
                _ => {}
            }
        }
    }
}

/// Detects `workflow.invoke_rule` cycles.
///
/// Cycles are rejected rather than bounded at runtime: an invocation cycle in a show pack is a
/// configuration bug, and refusing to load it is safer than silently cutting the chain short at
/// an arbitrary depth.
fn detect_invocation_cycles(pack: &RulePack) -> Vec<Diagnostic> {
    let rules: HashMap<&str, &Rule> = pack.rules.iter().map(|r| (r.id.as_str(), r)).collect();
    let mut diagnostics = Vec::new();
    let mut reported: HashSet<String> = HashSet::new();

    for rule in &pack.rules {
        let mut path = vec![rule.id.as_str().to_string()];
        let mut visited: HashSet<String> = HashSet::new();
        if let Some(cycle) = walk_invocations(&rules, rule.id.as_str(), &mut path, &mut visited) {
            if reported.insert(cycle.join(">")) {
                diagnostics.push(Diagnostic::error(
                    "rules",
                    format!(
                        "workflow.invoke_rule forms a cycle: {}; rule invocation must terminate",
                        cycle.join(" -> ")
                    ),
                ));
            }
        }
    }
    diagnostics
}

fn walk_invocations(
    rules: &HashMap<&str, &Rule>,
    current: &str,
    path: &mut Vec<String>,
    visited: &mut HashSet<String>,
) -> Option<Vec<String>> {
    if !visited.insert(current.to_string()) {
        return None;
    }
    let rule = rules.get(current)?;
    for step in rule.policy.all_steps(&rule.actions) {
        if let ActionSpec::InvokeRule { rule: target } = &step.spec {
            if let Some(start) = path.iter().position(|p| p == target) {
                let mut cycle = path[start..].to_vec();
                cycle.push(target.clone());
                return Some(cycle);
            }
            path.push(target.clone());
            if let Some(found) = walk_invocations(rules, target, path, visited) {
                return Some(found);
            }
            path.pop();
        }
    }
    None
}
