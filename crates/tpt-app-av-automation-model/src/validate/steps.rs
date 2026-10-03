//! Per-action-step validation.

use std::collections::HashSet;

use tpt_app_av_automation_core::Diagnostic;

use crate::action::ActionSpec;
use crate::pack::MAX_CHAIN_LENGTH;
use crate::trigger::MidiMessageKind;

/// Whether a diagnostic list contains at least one error.
pub fn has_errors(diagnostics: &[Diagnostic]) -> bool {
    diagnostics
        .iter()
        .any(|d| d.severity == tpt_app_av_automation_core::Severity::Error)
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
            group,
            value32,
            index,
        } => validate_midi(
            loc,
            channel,
            kind,
            number,
            value,
            group,
            value32,
            index,
            diagnostics,
        ),
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
        ActionSpec::DmxUniverse {
            universe, values, ..
        } => {
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

/// Validates one `control.midi` action (spec §8.1).
///
/// The checks split by protocol version. On a MIDI 1.0 port the value is 7-bit. A MIDI 2.0 (UMP)
/// endpoint is strictly wider — velocity became 16-bit and every other data field became 32-bit —
/// so `value32` widens the message rather than replacing `value`, and a MIDI 2.0-only message kind
/// is an error without one. The target device's protocol is not known here (the model crate must
/// stay free of the device registry), so the version is inferred from what the action asks for and
/// the device check is left to engine load time.
#[allow(clippy::too_many_arguments)]
fn validate_midi(
    loc: &str,
    channel: &u8,
    kind: &str,
    number: &u8,
    value: &u16,
    group: &Option<u8>,
    value32: &Option<u32>,
    index: &Option<u8>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let message = MidiMessageKind::parse(kind);
    if message.is_none() {
        diagnostics.push(Diagnostic::error(
            format!("{loc}.kind"),
            format!(
                "unknown MIDI message kind `{kind}`; expected one of {}",
                MidiMessageKind::names().collect::<Vec<_>>().join(", ")
            ),
        ));
    }
    if *channel > 15 {
        diagnostics.push(Diagnostic::error(
            format!("{loc}.channel"),
            format!("MIDI channel must be 0-15, got {channel}"),
        ));
    }
    if *number > 127 {
        diagnostics.push(Diagnostic::error(
            format!("{loc}.number"),
            format!("MIDI number must be 0-127, got {number}"),
        ));
    }
    if let Some(group) = group {
        if *group > 15 {
            diagnostics.push(Diagnostic::error(
                format!("{loc}.group"),
                format!("UMP group must be 0-15, got {group}"),
            ));
        }
    }
    if let Some(index) = index {
        if *index > 127 {
            diagnostics.push(Diagnostic::error(
                format!("{loc}.index"),
                format!("MIDI index must be 0-127, got {index}"),
            ));
        } else if let Some(kind) = message.filter(|m| !m.carries_index()) {
            diagnostics.push(Diagnostic::warning(
                format!("{loc}.index"),
                format!("`{kind}` carries no enumeration index; `index` is ignored"),
            ));
        }
    }

    let Some(kind) = message else { return };

    if kind.is_midi2_only() {
        match value32 {
            None => diagnostics.push(Diagnostic::error(
                format!("{loc}.value32"),
                format!(
                    "`{kind}` is a MIDI 2.0 message and needs a `value32`; it also needs a `ump` target device"
                ),
            )),
            Some(v32) if *v32 > kind.max_value32() => diagnostics.push(Diagnostic::error(
                format!("{loc}.value32"),
                if kind.velocity() {
                    format!("MIDI 2.0 velocity must fit in 16 bits, got {v32}")
                } else {
                    format!("MIDI 2.0 value must fit in 32 bits, got {v32}")
                },
            )),
            Some(_) => {
                if *value != 0 {
                    diagnostics.push(Diagnostic::warning(
                        format!("{loc}.value"),
                        format!("`{kind}` sends `value32`; the 16-bit `value` is ignored"),
                    ));
                }
            }
        }
        return;
    }

    // A MIDI 1.0 kind. `value32` promotes the message to MIDI 2.0 so the full range is reachable;
    // without it the value has to fit the 1.0 field for this kind.
    let ceiling = if value32.is_some() {
        u32::from(u16::MAX)
    } else {
        127
    };
    if u32::from(*value) > ceiling {
        diagnostics.push(Diagnostic::error(
            format!("{loc}.value"),
            format!("MIDI value must be 0-{ceiling} for `{kind}`, got {value}"),
        ));
    }
    if let Some(v32) = value32 {
        if *v32 > kind.max_value32() {
            diagnostics.push(Diagnostic::error(
                format!("{loc}.value32"),
                if kind.velocity() {
                    format!("MIDI 2.0 velocity must fit in 16 bits, got {v32}")
                } else {
                    format!("MIDI 2.0 value must fit in 32 bits, got {v32}")
                },
            ));
        } else if group.is_none() {
            diagnostics.push(Diagnostic::warning(
                format!("{loc}.group"),
                "`value32` needs a UMP port, so the target must be a `ump` device and `group` selects its port",
            ));
        }
    }
}
