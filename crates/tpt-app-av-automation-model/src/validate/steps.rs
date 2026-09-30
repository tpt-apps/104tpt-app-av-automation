//! Per-action-step validation.

use std::collections::HashSet;

use tpt_app_av_automation_core::Diagnostic;

use crate::action::ActionSpec;
use crate::pack::MAX_CHAIN_LENGTH;

/// Whether a diagnostic list contains at least one error.
pub fn has_errors(diagnostics: &[Diagnostic]) -> bool {
    diagnostics.iter().any(|d| d.severity == tpt_app_av_automation_core::Severity::Error)
}

/// Validates one action chain.
///
/// Checks step id uniqueness and ordering, whether hardware writes declare a device target, and
/// concurrency flags. Per-action field ranges are delegated to [`validate_action_spec`].
pub fn validate_chain(steps: &[crate::ActionStep], loc: &str, diagnostics: &mut Vec<Diagnostic>) {
    if steps.len() > MAX_CHAIN_LENGTH {
        diagnostics.push(Diagnostic::error(
            loc,
            format!(
                "chain has {} steps, exceeding the limit of {MAX_CHAIN_LENGTH}",
                steps.len()
            ),
        ));
    }

    let mut step_ids: HashSet<String> = HashSet::new();
    let mut first_sequential: Option<String> = None;

    for (index, step) in steps.iter().enumerate() {
        let step_loc = format!("{loc}[{index}]");

        if step.id.as_str().trim().is_empty() {
            diagnostics.push(Diagnostic::error(
                format!("{step_loc}.id"),
                "action id must not be empty",
            ));
        } else if !step_ids.insert(step.id.as_str().to_owned()) {
            diagnostics.push(Diagnostic::error(
                format!("{step_loc}.id"),
                format!("duplicate action id `{}`", step.id),
            ));
        }

        if step.spec.is_endpoint_write() && step.device.is_none() {
            diagnostics.push(Diagnostic::error(
                format!("{step_loc}.device"),
                format!(
                    "`{}` writes to hardware and must name a target device",
                    step.spec.type_label()
                ),
            ));
        }

        if !step.spec.is_endpoint_write() && step.device.is_some() {
            diagnostics.push(Diagnostic::warning(
                format!("{step_loc}.device"),
                format!(
                    "`{}` has no device target; the `device` field will be ignored",
                    step.spec.type_label()
                ),
            ));
        }

        // Chains execute in order, so once a step opts out of concurrency no later step may opt
        // back in; accepting it would silently change execution semantics.
        if step.concurrent_safe {
            if let Some(first) = &first_sequential {
                diagnostics.push(Diagnostic::error(
                    format!("{step_loc}.concurrent_safe"),
                    format!(
                        "action `{first}` is not concurrent_safe, so later actions cannot opt into concurrency; chains execute in order"
                    ),
                ));
            }
        } else if first_sequential.is_none() {
            first_sequential = Some(step.id.as_str().to_owned());
        }

        validate_action_spec(&step.spec, &step_loc, diagnostics);
    }
}

/// Validates the fields of a single action, whatever chain it appears in.
pub fn validate_action_spec(spec: &ActionSpec, loc: &str, diagnostics: &mut Vec<Diagnostic>) {
    match spec {
        ActionSpec::Osc { address, .. } => {
            if !address.starts_with('/') {
                diagnostics.push(Diagnostic::error(
                    format!("{loc}.address"),
                    format!("OSC address `{address}` must start with `/`"),
                ));
            }
        }
        ActionSpec::Midi {
            channel,
            kind,
            number,
            value,
        } => {
            if *channel > 15 {
                diagnostics.push(Diagnostic::error(
                    format!("{loc}.channel"),
                    "MIDI channel must be 0-15",
                ));
            }
            if *number > 127 {
                diagnostics.push(Diagnostic::error(
                    format!("{loc}.number"),
                    "MIDI number must be 0-127",
                ));
            }
            if *value > 16383 {
                diagnostics.push(Diagnostic::error(
                    format!("{loc}.value"),
                    "MIDI value out of range (7-bit 0-127, 14-bit 0-16383)",
                ));
            }
            if !matches!(
                kind.as_str(),
                "note_on" | "note_off" | "cc" | "program_change"
            ) {
                diagnostics.push(Diagnostic::error(
                    format!("{loc}.kind"),
                    format!(
                        "unknown MIDI message kind `{kind}`; expected note_on, note_off, cc or program_change"
                    ),
                ));
            }
            if kind == "program_change" && *value > 127 {
                diagnostics.push(Diagnostic::error(
                    format!("{loc}.value"),
                    "program_change value must be 0-127",
                ));
            }
        }
        ActionSpec::DmxChannels {
            universe,
            start_channel,
            values,
            ..
        } => {
            if *start_channel > 511 {
                diagnostics.push(Diagnostic::error(
                    format!("{loc}.start_channel"),
                    format!("DMX start channel {start_channel} is outside 0-511"),
                ));
            }
            if *start_channel as usize + values.len() > 512 {
                diagnostics.push(Diagnostic::error(
                    format!("{loc}.values"),
                    format!(
                        "{} values from channel {start_channel} run past the end of the universe",
                        values.len()
                    ),
                ));
            }
            if *universe > 32767 {
                diagnostics.push(Diagnostic::error(
                    format!("{loc}.universe"),
                    "universe number is out of range",
                ));
            }
        }
        ActionSpec::DmxUniverse { universe, values, .. } => {
            if values.len() > 512 {
                diagnostics.push(Diagnostic::error(
                    format!("{loc}.values"),
                    format!("universe has at most 512 channels, got {}", values.len()),
                ));
            }
            if *universe > 32767 {
                diagnostics.push(Diagnostic::error(
                    format!("{loc}.universe"),
                    "universe number is out of range",
                ));
            }
        }
        ActionSpec::DmxScene { scene, .. } => {
            if scene.trim().is_empty() {
                diagnostics.push(Diagnostic::error(
                    format!("{loc}.scene"),
                    "scene name must not be empty",
                ));
            }
        }
        ActionSpec::MediaVideoSource { source, .. } => {
            if source.trim().is_empty() {
                diagnostics.push(Diagnostic::error(
                    format!("{loc}.source"),
                    "video source must not be empty",
                ));
            }
        }
        ActionSpec::MediaAudioRoute { from, to, .. } => {
            if from.trim().is_empty() || to.trim().is_empty() {
                diagnostics.push(Diagnostic::error(
                    loc,
                    "audio route needs non-empty `from` and `to` endpoints",
                ));
            }
            if from == to {
                diagnostics.push(Diagnostic::warning(
                    loc,
                    "audio route source and destination are identical",
                ));
            }
        }
        ActionSpec::NotifyOperator { message, .. } => {
            if message.trim().is_empty() {
                diagnostics.push(Diagnostic::error(
                    format!("{loc}.message"),
                    "operator notification must say something",
                ));
            }
        }
        ActionSpec::Wait { ms } => {
            if *ms > 30_000 {
                diagnostics.push(Diagnostic::warning(
                    format!("{loc}.ms"),
                    format!("a {ms} ms inline wait will hold the action chain open"),
                ));
            }
        }
        ActionSpec::InvokeRule { rule } => {
            if rule.trim().is_empty() {
                diagnostics.push(Diagnostic::error(
                    format!("{loc}.rule"),
                    "invoke_rule must name a rule",
                ));
            }
        }
        ActionSpec::Exec {
            program,
            timeout_ms,
            ..
        } => {
            if program.trim().is_empty() {
                diagnostics.push(Diagnostic::error(
                    format!("{loc}.program"),
                    "exec must name a program",
                ));
            }
            if *timeout_ms == 0 {
                diagnostics.push(Diagnostic::error(
                    format!("{loc}.timeout_ms"),
                    "exec timeout must be greater than zero",
                ));
            } else if *timeout_ms > 60_000 {
                diagnostics.push(Diagnostic::warning(
                    format!("{loc}.timeout_ms"),
                    "exec timeouts above 60 s will stall the chain",
                ));
            }
        }
        ActionSpec::LogIncident { .. } => {}
        ActionSpec::UseAction { library } => {
            if library.trim().is_empty() {
                diagnostics.push(Diagnostic::error(
                    format!("{loc}.library"),
                    "library key must not be empty",
                ));
            }
        }
    }
}
