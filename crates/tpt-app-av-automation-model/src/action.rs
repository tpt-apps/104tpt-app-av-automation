//! Action specifications and chain steps (spec §6.4, §8).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use tpt_app_av_automation_core::{ActionId, Error, Result};

/// DMX transport used for an outbound universe action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DmxTransport {
    /// DMX512 over Art-Net (UDP 6454).
    ArtNet,
    /// DMX512 over sACN / E1.31 (UDP 5568).
    Sacn,
    /// DMX512 over a serial port.
    Dmx512,
}
/// Defaults to [`MediaOperation::Switch`], the common case for venue automation.
/// Operation applied to a media source or route.
///
/// Defaults to [MediaOperation::Switch], the most common case for venue automation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaOperation {
    /// Start playback/routing.
    Start,
    /// Stop playback/routing.
    Stop,
    /// Switch to this source/route.
    #[default]
    Switch,
}

/// Severity attached to operator notifications and incident log entries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum AlertSeverity {
    /// Informational only.
    Info,
    /// Something the operator should be aware of.
    #[default]
    Normal,
    /// Something went wrong but the show continues.
    High,
    /// The show is at risk; immediate intervention needed.
    Critical,
}

impl AlertSeverity {
    /// The canonical label.
    pub fn as_str(self) -> &'static str {
        match self {
            AlertSeverity::Info => "info",
            AlertSeverity::Normal => "normal",
            AlertSeverity::High => "high",
            AlertSeverity::Critical => "critical",
        }
    }
}

/// What an action does (spec §8).
///
/// YAML uses the dotted catalogue names from spec §9 — `control.osc`, `control.dmx_scene`,
/// `media.audio_route`, `notify.operator`, `log.incident`, `workflow.wait` — rather than
/// underscored variant names, so a rule pack reads the same as the specification and the docs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
#[serde(deny_unknown_fields)]
pub enum ActionSpec {
    /// Send an OSC message (spec §8.1).
    #[serde(rename = "control.osc")]
    Osc {
        /// OSC address, e.g. `/projector/1/power`.
        address: String,
        /// Positional arguments.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        args: Vec<f64>,
    },
    /// Send a MIDI message (spec §8.1).
    #[serde(rename = "control.midi")]
    Midi {
        /// Zero-based channel.
        channel: u8,
        /// Message kind.
        ///
        /// `note_on`, `note_off`, `cc` and `program_change` are MIDI 1.0 and are accepted by both
        /// `midi` and `ump` devices. The remaining kinds are MIDI 2.0-only and require a `ump`
        /// device: `per_note_rcc`, `per_note_acc`, `rpn`, `nrpn`, `relative_rpn`, `relative_nrpn`,
        /// `per_note_pitch_bend`, `polyphonic_pressure`, `channel_pressure` and `pitch_bend`.
        kind: String,
        /// Note/CC/program number, 0-127. For `rpn`/`nrpn` this is the parameter index; for the
        /// per-note kinds it is the note number.
        number: u8,
        /// Value: velocity or CC value, 7-bit for MIDI 1.0 kinds and up to 14-bit for `pitch_bend`
        /// on a MIDI 1.0 port.
        #[serde(default)]
        value: u16,
        /// UMP port group (0-15). Required for a `ump` device, rejected for a `midi` device.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        group: Option<u8>,
        /// The full 32-bit MIDI 2.0 value. Requires a `ump` device; a MIDI 1.0 port rejects it
        /// rather than silently truncating to 16 bits.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        value32: Option<u32>,
        /// Enumeration index for the kinds that carry one: `per_note_rcc`, `per_note_acc`, `rpn`,
        /// `nrpn`, `relative_rpn` and `relative_nrpn`. Ignored by every other kind.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        index: Option<u8>,
    },
    /// Set consecutive DMX channels (spec §8.1).
    #[serde(rename = "control.dmx_channels")]
    DmxChannels {
        /// Universe number.
        universe: u16,
        /// Zero-based start channel.
        start_channel: u16,
        /// Values to write, in consecutive channels.
        values: Vec<u8>,
        /// Transport to send on.
        #[serde(default = "default_transport")]
        transport: DmxTransport,
    },
    /// Recall a named DMX scene defined on the target device.
    #[serde(rename = "control.dmx_scene")]
    DmxScene {
        /// Scene name from the device's scene table.
        scene: String,
        /// Fade duration in milliseconds.
        #[serde(default)]
        fade_ms: u64,
    },
    /// Send a complete universe snapshot.
    #[serde(rename = "control.dmx_universe")]
    DmxUniverse {
        /// Universe number.
        universe: u16,
        /// 512 channel values.
        #[serde(default)]
        values: Vec<u8>,
        /// Transport to send on.
        #[serde(default = "default_transport")]
        transport: DmxTransport,
    },
    /// Start/stop/switch a video source via `tpt-kinetix` (spec §8.2).
    #[serde(rename = "media.video_source")]
    MediaVideoSource {
        /// Named video source, e.g. `display-1-input`.
        source: String,
        /// Operation to perform.
        operation: MediaOperation,
    },
    /// Start/stop audio routing via `tpt-cadence` (spec §8.2).
    #[serde(rename = "media.audio_route")]
    MediaAudioRoute {
        /// Source endpoint name.
        from: String,
        /// Destination endpoint name.
        to: String,
        /// Operation to perform.
        #[serde(default)]
        operation: MediaOperation,
    },
    /// Alert the operator (spec §8.3).
    #[serde(rename = "notify.operator")]
    NotifyOperator {
        /// Message shown to the operator.
        message: String,
        /// Severity of the alert.
        #[serde(default = "default_severity")]
        severity: AlertSeverity,
    },
    /// Write to the incident log (spec §8.3).
    #[serde(rename = "log.incident")]
    LogIncident {
        /// Severity recorded against the incident.
        #[serde(default = "default_severity")]
        severity: AlertSeverity,
        /// Optional message; defaults to the rule's own description.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<String>,
    },
    /// Pause the chain (spec §8.4).
    #[serde(rename = "workflow.wait")]
    Wait {
        /// Delay in milliseconds.
        ms: u64,
    },
    /// Invoke another rule (spec §8.4).
    #[serde(rename = "workflow.invoke_rule")]
    InvokeRule {
        /// Rule to invoke.
        rule: String,
    },
    /// Run a sandboxed external executable with a bounded timeout (spec §8.4, §16).
    #[serde(rename = "workflow.exec")]
    Exec {
        /// Program to run.
        program: String,
        /// Arguments.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        args: Vec<String>,
        /// Hard wall-clock bound in milliseconds.
        #[serde(default = "default_exec_timeout")]
        timeout_ms: u64,
    },
    /// Reference a fragment from the pack's `action_library`.
    #[serde(rename = "workflow.use_action")]
    UseAction {
        /// Library key.
        library: String,
    },
}

fn default_transport() -> DmxTransport {
    DmxTransport::ArtNet
}

fn default_severity() -> AlertSeverity {
    AlertSeverity::Normal
}

fn default_exec_timeout() -> u64 {
    5_000
}

impl ActionSpec {
    /// Parses a dotted catalogue type name, e.g. `control.osc`.
    ///
    /// The error lists every valid name, because an unknown action type is the most common
    /// authoring mistake and serde's bare "unknown variant" message is unhelpful at 2am.
    pub fn parse_type_name(name: &str) -> Result<ActionSpec, Error> {
        let known = [
            "control.osc",
            "control.midi",
            "control.dmx_channels",
            "control.dmx_scene",
            "control.dmx_universe",
            "media.video_source",
            "media.audio_route",
            "notify.operator",
            "log.incident",
            "workflow.wait",
            "workflow.invoke_rule",
            "workflow.exec",
            "workflow.use_action",
        ];
        Err(Error::Parse(format!(
            "unknown action type `{name}`; expected one of: {}",
            known.join(", ")
        )))
    }

    /// Stable type label, e.g. `control.osc`, `media.video_source`.
    pub fn type_label(&self) -> &'static str {
        match self {
            ActionSpec::Osc { .. } => "control.osc",
            ActionSpec::Midi { .. } => "control.midi",
            ActionSpec::DmxChannels { .. } => "control.dmx_channels",
            ActionSpec::DmxScene { .. } => "control.dmx_scene",
            ActionSpec::DmxUniverse { .. } => "control.dmx_universe",
            ActionSpec::MediaVideoSource { .. } => "media.video_source",
            ActionSpec::MediaAudioRoute { .. } => "media.audio_route",
            ActionSpec::NotifyOperator { .. } => "notify.operator",
            ActionSpec::LogIncident { .. } => "log.incident",
            ActionSpec::Wait { .. } => "workflow.wait",
            ActionSpec::InvokeRule { .. } => "workflow.invoke_rule",
            ActionSpec::Exec { .. } => "workflow.exec",
            ActionSpec::UseAction { .. } => "workflow.use_action",
        }
    }

    /// Whether this action writes to a physical endpoint, and therefore needs a device target and
    /// participates in conflict resolution.
    pub fn is_endpoint_write(&self) -> bool {
        matches!(
            self,
            ActionSpec::Osc { .. }
                | ActionSpec::Midi { .. }
                | ActionSpec::DmxChannels { .. }
                | ActionSpec::DmxScene { .. }
                | ActionSpec::DmxUniverse { .. }
                | ActionSpec::MediaVideoSource { .. }
                | ActionSpec::MediaAudioRoute { .. }
        )
    }

    /// Resolves a `UseAction` reference against a library, following one level of indirection.
    ///
    /// A fragment that itself references a fragment is rejected, so library resolution cannot
    /// recurse and cannot build an unbounded chain.
    pub fn resolve_library<'a>(
        &'a self,
        library: &'a BTreeMap<String, ActionSpec>,
    ) -> Result<&'a ActionSpec, &'static str> {
        match self {
            ActionSpec::UseAction { library: key } => {
                let resolved = library.get(key).ok_or("library fragment does not exist")?;
                if matches!(resolved, ActionSpec::UseAction { .. }) {
                    return Err("library fragments must not reference other fragments");
                }
                Ok(resolved)
            }
            other => Ok(other),
        }
    }
}

/// What happens when an action step fails (spec §6.4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum FailurePolicy {
    /// Abort the chain; remaining steps are recorded as skipped.
    #[default]
    StopChain,
    /// Continue with the next step.
    ContinueChain,
    /// Run the rule's fallback chain instead of the remaining steps.
    RunFallback,
}

impl FailurePolicy {
    /// The canonical label.
    pub fn as_str(&self) -> &'static str {
        match self {
            FailurePolicy::StopChain => "stop_chain",
            FailurePolicy::ContinueChain => "continue_chain",
            FailurePolicy::RunFallback => "run_fallback",
        }
    }
}

/// One step of an action chain.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActionStep {
    /// Stable identifier, referenced by execution records.
    #[serde(default)]
    pub id: ActionId,
    /// What to do.
    #[serde(flatten)]
    pub spec: ActionSpec,
    /// Target device/endpoint for actions that write to hardware.
    ///
    /// Notification, log, wait and workflow steps have no target. Every endpoint write must name a
    /// device that exists in the registry, so an unresolvable target is a validation error rather
    /// than a runtime surprise mid-show.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    /// Per-step timeout in milliseconds. `None` means the chain budget applies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    /// Whether this step may run concurrently with other steps targeting the same device.
    #[serde(default)]
    pub concurrent_safe: bool,
    /// Estimated cost, recorded so operators can spot pathological chains in the UI.
    #[serde(default = "default_cost")]
    pub estimated_cost_ms: u64,
}

fn default_cost() -> u64 {
    10
}

impl ActionStep {
    /// Builds a step with defaults.
    pub fn new(id: impl Into<ActionId>, spec: ActionSpec) -> Self {
        Self {
            id: id.into(),
            spec,
            device: None,
            timeout_ms: None,
            concurrent_safe: false,
            estimated_cost_ms: default_cost(),
        }
    }

    /// Assigns a target device.
    pub fn targeting(mut self, device: impl Into<String>) -> Self {
        self.device = Some(device.into());
        self
    }

    /// The effective timeout: the step bound if set, otherwise the chain budget.
    pub fn effective_timeout_ms(&self, chain_budget_ms: Option<u64>) -> Option<u64> {
        self.timeout_ms.or(chain_budget_ms)
    }

    /// The resolved device target.
    pub fn target(&self) -> Option<&str> {
        self.device.as_deref()
    }
}
