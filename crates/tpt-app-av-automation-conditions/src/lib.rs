//! Condition evaluation (spec §6.3).
//!
//! Evaluation is a pure function of a condition and an [`EvaluationContext`] snapshot. The context
//! carries *everything* a condition may read — local time, device health, DMX levels, parameters,
//! the armed set — so the same inputs always give the same result (spec §3.2), and "we have never
//! heard from that device" is reported as [`ConditionResult::Unknown`], never as `NotMet`.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use std::collections::{BTreeMap, BTreeSet};

use tpt_app_av_automation_core::DeviceHealth;
use tpt_app_av_automation_model::{Condition, ConditionResult, ConditionSpec};

/// Snapshot of the world a condition is evaluated against.
#[derive(Debug, Clone, Default)]
pub struct EvaluationContext {
    /// Local minutes since midnight; `None` when the shell supplied no wall-clock time.
    pub local_minutes: Option<u32>,
    /// Health per device id. Missing entries mean the device is unregistered.
    pub device_health: BTreeMap<String, DeviceHealth>,
    /// Last observed DMX value per `(universe, channel)`.
    pub dmx: BTreeMap<(u16, u16), u8>,
    /// Last reported parameter per `(device, parameter)`.
    pub parameters: BTreeMap<(String, String), f64>,
    /// Ids of rules currently armed.
    pub armed_rules: BTreeSet<String>,
    /// Every known rule id (so `rule_armed` can tell "disarmed" from "no such rule").
    pub known_rules: BTreeSet<String>,
    /// Tags of the rule being evaluated.
    pub rule_tags: Vec<String>,
}

/// Result plus a human-readable observation for the execution record (spec §3.3).
#[derive(Debug, Clone, PartialEq)]
pub struct Evaluation {
    /// The three-valued result.
    pub result: ConditionResult,
    /// What was observed, e.g. `device lighting-rack is offline`.
    pub observed: String,
}

impl Evaluation {
    fn new(result: ConditionResult, observed: impl Into<String>) -> Self {
        Self {
            result,
            observed: observed.into(),
        }
    }

    fn from_bool(holds: bool, observed: impl Into<String>) -> Self {
        Self::new(
            if holds {
                ConditionResult::Met
            } else {
                ConditionResult::NotMet
            },
            observed,
        )
    }
}

/// Evaluates one condition.
pub fn evaluate(condition: &Condition, ctx: &EvaluationContext) -> Evaluation {
    match &condition.spec {
        ConditionSpec::DeviceHealth { device, equals } => match ctx.device_health.get(device) {
            None => Evaluation::new(
                ConditionResult::Unknown,
                format!("device `{device}` is not registered"),
            ),
            Some(DeviceHealth::Unknown) => Evaluation::new(
                ConditionResult::Unknown,
                format!("device `{device}` has not reported state yet"),
            ),
            Some(actual) => Evaluation::from_bool(
                actual == equals,
                format!("device `{device}` is {actual}, required {equals}"),
            ),
        },
        ConditionSpec::TimeWindow { from, to } => match ctx.local_minutes {
            None => Evaluation::new(ConditionResult::Unknown, "no wall-clock time available"),
            Some(now) => {
                let (from, to) = (from.minutes_since_midnight(), to.minutes_since_midnight());
                // `from` inclusive, `to` exclusive; a window with from > to wraps past midnight and
                // from == to is the empty window.
                let inside = if from <= to {
                    now >= from && now < to
                } else {
                    now >= from || now < to
                };
                Evaluation::from_bool(
                    inside,
                    format!("local time {:02}:{:02}", now / 60, now % 60),
                )
            }
        },
        ConditionSpec::DmxChannel {
            universe,
            channel,
            comparison,
            value,
        } => match ctx.dmx.get(&(*universe, *channel)) {
            None => Evaluation::new(
                ConditionResult::Unknown,
                format!("no DMX observation for universe {universe} channel {channel}"),
            ),
            Some(actual) => Evaluation::from_bool(
                comparison.evaluate_f64(f64::from(*actual), *value),
                format!("dmx u{universe} ch{channel} = {actual}"),
            ),
        },
        ConditionSpec::DeviceParameter {
            device,
            parameter,
            comparison,
            value,
        } => match ctx.parameters.get(&(device.clone(), parameter.clone())) {
            None => Evaluation::new(
                ConditionResult::Unknown,
                format!("`{device}` has not reported `{parameter}`"),
            ),
            Some(actual) => Evaluation::from_bool(
                comparison.evaluate_f64(*actual, *value),
                format!("`{device}`.{parameter} = {actual}"),
            ),
        },
        ConditionSpec::RuleArmed { rule } => {
            if !ctx.known_rules.contains(rule) {
                Evaluation::new(ConditionResult::Unknown, format!("rule `{rule}` does not exist"))
            } else {
                let armed = ctx.armed_rules.contains(rule);
                Evaluation::from_bool(
                    armed,
                    format!("rule `{rule}` is {}", if armed { "armed" } else { "disarmed" }),
                )
            }
        }
        ConditionSpec::RuleHasTag { tag } => {
            let has = ctx.rule_tags.iter().any(|t| t == tag);
            Evaluation::from_bool(has, format!("tag `{tag}` {}", if has { "present" } else { "absent" }))
        }
    }
}

/// Outcome of gating a rule on all of its conditions.
#[derive(Debug, Clone, PartialEq)]
pub struct Gate {
    /// Per-condition results, in declaration order.
    pub results: Vec<(String, Evaluation, bool)>,
    /// Whether the chain may proceed.
    pub permits: bool,
}

/// Evaluates every condition (no short-circuiting, so the record explains all of them) and applies
/// the `fire_on_unknown` opt-in.
pub fn gate(conditions: &[Condition], ctx: &EvaluationContext) -> Gate {
    let mut permits = true;
    let results = conditions
        .iter()
        .map(|c| {
            let eval = evaluate(c, ctx);
            let ok = eval.result.permits(c.fire_on_unknown);
            permits &= ok;
            (c.id.as_str().to_owned(), eval, ok)
        })
        .collect();
    Gate { results, permits }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tpt_app_av_automation_model::{Comparison, LocalTime};

    fn cond(spec: ConditionSpec) -> Condition {
        Condition::new("c", spec)
    }

    fn at(h: u8, m: u8) -> LocalTime {
        LocalTime::new(h, m).unwrap()
    }

    fn time_ctx(minutes: u32) -> EvaluationContext {
        EvaluationContext {
            local_minutes: Some(minutes),
            ..Default::default()
        }
    }

    #[test]
    fn time_window_is_inclusive_start_exclusive_end() {
        let c = cond(ConditionSpec::TimeWindow {
            from: at(18, 0),
            to: at(23, 0),
        });
        assert_eq!(evaluate(&c, &time_ctx(17 * 60 + 59)).result, ConditionResult::NotMet);
        assert_eq!(evaluate(&c, &time_ctx(18 * 60)).result, ConditionResult::Met);
        assert_eq!(evaluate(&c, &time_ctx(22 * 60 + 59)).result, ConditionResult::Met);
        assert_eq!(evaluate(&c, &time_ctx(23 * 60)).result, ConditionResult::NotMet);
    }

    #[test]
    fn time_window_wraps_midnight_and_empty_window_never_matches() {
        let wrap = cond(ConditionSpec::TimeWindow {
            from: at(22, 0),
            to: at(2, 0),
        });
        assert_eq!(evaluate(&wrap, &time_ctx(23 * 60)).result, ConditionResult::Met);
        assert_eq!(evaluate(&wrap, &time_ctx(60)).result, ConditionResult::Met);
        assert_eq!(evaluate(&wrap, &time_ctx(12 * 60)).result, ConditionResult::NotMet);
        let empty = cond(ConditionSpec::TimeWindow {
            from: at(5, 0),
            to: at(5, 0),
        });
        assert_eq!(evaluate(&empty, &time_ctx(5 * 60)).result, ConditionResult::NotMet);
    }

    #[test]
    fn time_window_without_clock_is_unknown() {
        let c = cond(ConditionSpec::TimeWindow {
            from: at(1, 0),
            to: at(2, 0),
        });
        assert_eq!(evaluate(&c, &EvaluationContext::default()).result, ConditionResult::Unknown);
    }

    #[test]
    fn device_health_distinguishes_unknown_from_not_met() {
        let c = cond(ConditionSpec::DeviceHealth {
            device: "d".into(),
            equals: DeviceHealth::Online,
        });
        let mut ctx = EvaluationContext::default();
        assert_eq!(evaluate(&c, &ctx).result, ConditionResult::Unknown, "unregistered");
        ctx.device_health.insert("d".into(), DeviceHealth::Unknown);
        assert_eq!(evaluate(&c, &ctx).result, ConditionResult::Unknown, "never reported");
        ctx.device_health.insert("d".into(), DeviceHealth::Offline);
        assert_eq!(evaluate(&c, &ctx).result, ConditionResult::NotMet);
        ctx.device_health.insert("d".into(), DeviceHealth::Online);
        assert_eq!(evaluate(&c, &ctx).result, ConditionResult::Met);
    }

    #[test]
    fn dmx_and_parameter_comparisons() {
        let dmx = cond(ConditionSpec::DmxChannel {
            universe: 1,
            channel: 3,
            comparison: Comparison::Gte,
            value: 128.0,
        });
        let mut ctx = EvaluationContext::default();
        assert_eq!(evaluate(&dmx, &ctx).result, ConditionResult::Unknown);
        ctx.dmx.insert((1, 3), 127);
        assert_eq!(evaluate(&dmx, &ctx).result, ConditionResult::NotMet);
        ctx.dmx.insert((1, 3), 128);
        assert_eq!(evaluate(&dmx, &ctx).result, ConditionResult::Met);

        let param = cond(ConditionSpec::DeviceParameter {
            device: "p".into(),
            parameter: "temp".into(),
            comparison: Comparison::Lt,
            value: 50.0,
        });
        assert_eq!(evaluate(&param, &ctx).result, ConditionResult::Unknown);
        ctx.parameters.insert(("p".into(), "temp".into()), 49.9);
        assert_eq!(evaluate(&param, &ctx).result, ConditionResult::Met);
        ctx.parameters.insert(("p".into(), "temp".into()), 50.0);
        assert_eq!(evaluate(&param, &ctx).result, ConditionResult::NotMet);
    }

    #[test]
    fn rule_armed_and_tag_conditions() {
        let armed = cond(ConditionSpec::RuleArmed { rule: "r".into() });
        let mut ctx = EvaluationContext::default();
        assert_eq!(evaluate(&armed, &ctx).result, ConditionResult::Unknown);
        ctx.known_rules.insert("r".into());
        assert_eq!(evaluate(&armed, &ctx).result, ConditionResult::NotMet);
        ctx.armed_rules.insert("r".into());
        assert_eq!(evaluate(&armed, &ctx).result, ConditionResult::Met);

        let tag = cond(ConditionSpec::RuleHasTag { tag: "show".into() });
        assert_eq!(evaluate(&tag, &ctx).result, ConditionResult::NotMet);
        ctx.rule_tags.push("show".into());
        assert_eq!(evaluate(&tag, &ctx).result, ConditionResult::Met);
    }

    #[test]
    fn gate_blocks_on_unknown_unless_opted_in() {
        let spec = ConditionSpec::DeviceHealth {
            device: "d".into(),
            equals: DeviceHealth::Online,
        };
        let ctx = EvaluationContext::default();
        let strict = gate(&[Condition::new("a", spec.clone())], &ctx);
        assert!(!strict.permits);
        let lenient = gate(&[Condition::lenient("a", spec)], &ctx);
        assert!(lenient.permits);
        assert!(gate(&[], &ctx).permits, "no conditions always permits");
    }

    #[test]
    fn gate_evaluates_every_condition_for_explainability() {
        let ctx = time_ctx(0);
        let not_met = ConditionSpec::TimeWindow {
            from: at(10, 0),
            to: at(11, 0),
        };
        let g = gate(
            &[
                Condition::new("a", not_met.clone()),
                Condition::new("b", not_met),
            ],
            &ctx,
        );
        assert!(!g.permits);
        assert_eq!(g.results.len(), 2);
    }
}
