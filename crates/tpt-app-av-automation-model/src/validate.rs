//! Rule pack validation (spec §9).
//!
//! Validation is total: every check runs and every problem is reported with a location, so an
//! operator fixing a live-show pack sees all errors at once instead of one per edit cycle.
//!
//! The checks are split across three files purely for readability:
//!
//! - [`validate_pack`] — pack-level and per-rule structure.
//! - [`steps`] — per-action-step checks.
//! - [`cross`] — checks that need to see more than one rule.
//!
//! Checks that need the device registry are performed by the engine at load time, not here: the
//! model crate must stay free of I/O so the CLI, service and desktop UI share it exactly.

pub mod cross;
pub mod steps;

use std::collections::HashSet;

use tpt_app_av_automation_core::{Diagnostic, Error, Result};

use crate::condition::ConditionSpec;
use crate::cron::CronSchedule;
use crate::pack::{RulePack, FORMAT_VERSION};
use crate::rule::Rule;
use crate::trigger::{DmxComparison, TriggerSpec};

use steps::has_errors;

/// Validates a rule pack, returning every diagnostic found.
///
/// Returns [`Error::Validation`] only when at least one diagnostic is an error; warnings alone are
/// returned as `Ok`, because a pack with warnings is still loadable.
pub fn validate_pack(pack: &RulePack) -> Result<Vec<Diagnostic>> {
    let mut diagnostics = Vec::new();

    if pack.format_version > FORMAT_VERSION {
        diagnostics.push(Diagnostic::error(
            "format_version",
            format!(
                "pack requires format version {} but this build understands {}",
                pack.format_version, FORMAT_VERSION
            ),
        ));
    } else if pack.format_version < FORMAT_VERSION {
        diagnostics.push(Diagnostic::warning(
            "format_version",
            format!(
                "pack written for older format version {}; upgrade with `migrate`",
                pack.format_version
            ),
        ));
    }

    if pack.name.trim().is_empty() {
        diagnostics.push(Diagnostic::error("name", "pack name must not be empty"));
    }

    if pack.rules.is_empty() {
        diagnostics.push(Diagnostic::warning(
            "rules",
            "pack contains no rules; it will load but never fire",
        ));
    }

    let mut seen_ids: HashSet<String> = HashSet::new();
    for (index, rule) in pack.rules.iter().enumerate() {
        let base = format!("rules[{index}]");
        validate_rule(rule, &base, pack.site.as_ref(), &mut diagnostics);

        if rule.id.as_str().trim().is_empty() {
            diagnostics.push(Diagnostic::error(
                format!("{base}.id"),
                "rule id must not be empty",
            ));
        } else if !seen_ids.insert(rule.id.as_str().to_owned()) {
            diagnostics.push(Diagnostic::error(
                format!("{base}.id"),
                format!("duplicate rule id `{}`", rule.id),
            ));
        }
    }

    cross::validate_cross_references(pack, &mut diagnostics);

    if has_errors(&diagnostics) {
        return Err(Error::Validation(diagnostics));
    }
    Ok(diagnostics)
}

fn validate_rule(
    rule: &Rule,
    base: &str,
    site: Option<&crate::solar::SolarSite>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if rule.name.trim().is_empty() {
        diagnostics.push(Diagnostic::error(
            format!("{base}.name"),
            "rule name must not be empty",
        ));
    }

    validate_trigger(
        &rule.trigger,
        &format!("{base}.trigger"),
        base,
        site,
        diagnostics,
    );

    let mut condition_ids: HashSet<String> = HashSet::new();
    for (index, condition) in rule.conditions.iter().enumerate() {
        let loc = format!("{base}.conditions[{index}]");
        if condition.id.as_str().trim().is_empty() {
            diagnostics.push(Diagnostic::error(
                format!("{loc}.id"),
                "condition id must not be empty",
            ));
        } else if !condition_ids.insert(condition.id.as_str().to_owned()) {
            diagnostics.push(Diagnostic::error(
                format!("{loc}.id"),
                format!("duplicate condition id `{}`", condition.id),
            ));
        }
        if let ConditionSpec::TimeWindow { from, to } = &condition.spec {
            if from >= to {
                diagnostics.push(Diagnostic::error(
                    format!("{loc}.time_window"),
                    format!("window start {from} must be before end {to}"),
                ));
            }
        }
        // A DMX condition is bounded by the universe exactly as a DMX trigger is; without this a
        // rule can gate on a channel that can never carry a value and silently never pass.
        if let ConditionSpec::DmxChannel {
            universe, channel, ..
        } = &condition.spec
        {
            if *channel > 511 {
                diagnostics.push(Diagnostic::error(
                    format!("{loc}.channel"),
                    format!("DMX channel {channel} is outside 0-511"),
                ));
            }
            if *universe > 32767 {
                diagnostics.push(Diagnostic::error(
                    format!("{loc}.universe"),
                    "universe number is out of range",
                ));
            }
        }
        if let ConditionSpec::DeviceHealth { device, .. } = &condition.spec {
            if device.trim().is_empty() {
                diagnostics.push(Diagnostic::error(
                    format!("{loc}.device"),
                    "device must not be empty",
                ));
            }
        }
        if let ConditionSpec::DeviceParameter {
            device, parameter, ..
        } = &condition.spec
        {
            if device.trim().is_empty() {
                diagnostics.push(Diagnostic::error(
                    format!("{loc}.device"),
                    "device must not be empty",
                ));
            }
            if parameter.trim().is_empty() {
                diagnostics.push(Diagnostic::error(
                    format!("{loc}.parameter"),
                    "parameter must not be empty",
                ));
            }
        }
    }

    steps::validate_chain(&rule.actions, &format!("{base}.actions"), diagnostics);
    steps::validate_chain(
        &rule.policy.fallback,
        &format!("{base}.policy.fallback"),
        diagnostics,
    );

    if rule.actions.is_empty() {
        diagnostics.push(Diagnostic::error(
            format!("{base}.actions"),
            "rule must define at least one action",
        ));
    }

    if rule.policy.on_failure == crate::FailurePolicy::RunFallback
        && rule.policy.fallback.is_empty()
    {
        diagnostics.push(Diagnostic::error(
            format!("{base}.policy.on_failure"),
            "on_failure is run_fallback but no fallback chain is defined",
        ));
    }

    if rule.armed && rule.reentrant {
        diagnostics.push(Diagnostic::warning(
            format!("{base}.reentrant"),
            "armed reentrant rule may fire repeatedly on a sustained event",
        ));
    }

    let estimated: u64 = rule
        .actions
        .iter()
        .map(|s| s.timeout_ms.unwrap_or(s.estimated_cost_ms))
        .sum();
    if rule
        .policy
        .timeout_ms
        .is_some_and(|budget| estimated > budget)
    {
        diagnostics.push(Diagnostic::warning(
            format!("{base}.policy.timeout_ms"),
            format!(
                "sum of step timeouts ({estimated} ms) exceeds the chain budget; the chain will be cut short"
            ),
        ));
    }
}

fn validate_trigger(
    trigger: &crate::Trigger,
    loc: &str,
    base: &str,
    site: Option<&crate::solar::SolarSite>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    match &trigger.spec {
        TriggerSpec::Schedule(spec) => {
            let forms = usize::from(spec.at.is_some())
                + usize::from(spec.cron.is_some())
                + usize::from(spec.solar.is_some())
                + usize::from(spec.interval_ms.is_some());
            if forms == 0 {
                diagnostics.push(Diagnostic::error(
                    loc,
                    "schedule trigger needs one of `at`, `cron`, `solar` or `interval_ms`",
                ));
            } else if forms > 1 {
                diagnostics.push(Diagnostic::error(
                    loc,
                    "schedule trigger must use exactly one of `at`, `cron`, `solar` or `interval_ms`",
                ));
            }
            if let Some(solar) = &spec.solar {
                // A solar crossing needs the site, and the site has to be a real place on Earth.
                if !site.is_some_and(|s| s.is_valid()) {
                    diagnostics.push(Diagnostic::error(
                        format!("{base}.site"),
                        "a `solar` schedule needs a pack-level `site` with a valid latitude (-90 to 90) and longitude (-180 to 180)".to_string(),
                    ));
                }
                // An offset of more than a day cannot be meant: it would schedule a time on the
                // wrong day and only fire if the sun happened to cross at that minute.
                if solar.minutes.abs() > 720 {
                    diagnostics.push(Diagnostic::error(
                        format!("{loc}.solar.minutes"),
                        "a solar offset must be within +/- 12 hours of the crossing",
                    ));
                }
            }
            if let Some(expression) = &spec.cron {
                if let Err(e) = CronSchedule::parse(expression) {
                    diagnostics.push(Diagnostic::error(format!("{loc}.cron"), e.to_string()));
                }
                if !spec.days.is_empty() {
                    diagnostics.push(Diagnostic::error(
                        format!("{loc}.days"),
                        "`days` cannot be combined with `cron`; use the expression's day-of-week field",
                    ));
                }
            }
            if let Some(interval) = spec.interval_ms {
                if interval == 0 {
                    diagnostics.push(Diagnostic::error(
                        format!("{loc}.interval_ms"),
                        "interval must be greater than zero",
                    ));
                } else if interval < 250 {
                    diagnostics.push(Diagnostic::warning(
                        format!("{loc}.interval_ms"),
                        "intervals below 250 ms will load the engine and are rarely intended",
                    ));
                }
            }
            if spec.once && spec.at.is_none() && spec.cron.is_none() {
                diagnostics.push(Diagnostic::error(
                    format!("{loc}.once"),
                    "`once` requires a fixed `at` time or a `cron` expression",
                ));
            }
        }
        TriggerSpec::Osc {
            address,
            min_args,
            arg_equals,
        } => {
            if !address.starts_with('/') {
                diagnostics.push(Diagnostic::error(
                    format!("{loc}.address"),
                    format!("OSC address `{address}` must start with `/`"),
                ));
            }
            if min_args == &Some(0) && arg_equals.is_some() {
                diagnostics.push(Diagnostic::error(
                    loc,
                    "`arg_equals` requires at least one argument",
                ));
            }
        }
        TriggerSpec::Midi {
            channel,
            number,
            group,
            ..
        } => {
            if channel.is_some_and(|c| c > 15) {
                diagnostics.push(Diagnostic::error(
                    format!("{loc}.channel"),
                    "MIDI channel must be 0-15",
                ));
            }
            if number.is_some_and(|n| n > 127) {
                diagnostics.push(Diagnostic::error(
                    format!("{loc}.number"),
                    "MIDI number must be 0-127",
                ));
            }
            if group.is_some_and(|g| g > 15) {
                diagnostics.push(Diagnostic::error(
                    format!("{loc}.group"),
                    "UMP group must be 0-15",
                ));
            }
        }
        TriggerSpec::Dmx {
            universe,
            channel,
            comparison,
            ..
        } => {
            if *channel > 511 {
                diagnostics.push(Diagnostic::error(
                    format!("{loc}.channel"),
                    format!("DMX channel {channel} is outside 0-511"),
                ));
            }
            if *universe > 32767 {
                diagnostics.push(Diagnostic::error(
                    format!("{loc}.universe"),
                    "universe number is out of range",
                ));
            }
            let is_edge = matches!(
                comparison,
                DmxComparison::Changed | DmxComparison::CrossedAbove | DmxComparison::CrossedBelow
            );
            if !is_edge && trigger.dmx_previous.is_some() {
                diagnostics.push(Diagnostic::warning(
                    format!("{loc}.dmx_previous"),
                    "dmx_previous is only meaningful for edge comparisons",
                ));
            }
        }
        TriggerSpec::HeartbeatMissed { device, after_ms } => {
            if *after_ms == 0 {
                diagnostics.push(Diagnostic::error(
                    format!("{loc}.after_ms"),
                    "heartbeat threshold must be greater than zero",
                ));
            }
            if device.trim().is_empty() {
                diagnostics.push(Diagnostic::error(
                    format!("{loc}.device"),
                    "device must not be empty",
                ));
            }
        }
        TriggerSpec::DeviceState { device, .. } | TriggerSpec::DeviceParameter { device, .. } => {
            if device.trim().is_empty() {
                diagnostics.push(Diagnostic::error(
                    format!("{loc}.device"),
                    "device must not be empty",
                ));
            }
        }
        TriggerSpec::Api { name } => {
            if let Some(name) = name {
                if !name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
                {
                    diagnostics.push(Diagnostic::error(
                        format!("{loc}.name"),
                        "API name may contain only letters, digits, `_`, `-` and `.`",
                    ));
                }
            }
        }
        TriggerSpec::Manual => {}
    }
}
