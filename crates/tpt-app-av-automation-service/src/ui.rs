//! The desktop UI's view of a running service (spec §12).
//!
//! Every screen of the desktop app is a thin renderer over this bridge: the same engine, the same
//! pack operations and the same status document as the local API (spec §14), because the CLI, the
//! API and the GUI must behave identically (spec §13, §24). The bridge is plain Rust — no webview
//! types — so every screen's backing logic is covered by ordinary integration tests.

use serde::Serialize;
use serde_json::{json, Value};
use tpt_app_av_automation_model::ExecutionStatus;
use tpt_app_av_automation_report::Filter;

use crate::api::{executions_json, health_json};
use crate::runtime::{ControlRequest, ServiceHandle};

/// One action the builder can insert, with a canonical valid example.
#[derive(Debug, Clone, Serialize)]
pub struct CatalogueEntry {
    /// Stable catalogue type name, e.g. `control.osc`.
    pub kind: &'static str,
    /// Human-readable label.
    pub label: &'static str,
    /// A valid example payload (a full `trigger`/`condition`/`action` object).
    pub example: Value,
}

/// A group of catalogue entries (all triggers, all conditions, all actions).
#[derive(Debug, Clone, Serialize)]
pub struct CatalogueGroup {
    /// Group key: `trigger`, `condition` or `action`.
    pub kind: &'static str,
    /// Human-readable group label.
    pub label: &'static str,
    /// The entries, in catalogue order.
    pub entries: Vec<CatalogueEntry>,
}

/// The command surface of the desktop UI.
///
/// Cloning is cheap and shares the same service connection; the webview layer keeps one bridge in
/// Tauri-managed state and calls it from every command.
#[derive(Clone)]
pub struct UiBridge {
    handle: ServiceHandle,
}

impl UiBridge {
    /// Wraps a running (or about-to-run) service.
    pub fn new(handle: ServiceHandle) -> Self {
        Self { handle }
    }

    /// The underlying handle, for the event stream and shutdown.
    pub fn handle(&self) -> &ServiceHandle {
        &self.handle
    }

    fn control(&self, request: ControlRequest) -> Result<Value, String> {
        self.handle.control(request)
    }

    /// Everything the dashboard screen needs in one document: the status summary, the rules with
    /// their armed state, device health, active chains and the most recent executions (spec §12.1).
    pub fn dashboard(&self) -> Value {
        let snapshot = self.handle.snapshot();
        let recent: Vec<Value> = snapshot
            .recent
            .iter()
            .rev()
            .take(20)
            .filter_map(|r| serde_json::to_value(r).ok())
            .collect();
        json!({
            "status": health_json(&self.handle),
            "rules": snapshot.rules,
            "devices": snapshot.devices,
            "active_chains": snapshot.active_chains
                .iter()
                .map(|(execution_id, rule_id)| json!({
                    "execution_id": execution_id,
                    "rule_id": rule_id,
                }))
                .collect::<Vec<_>>(),
            "recent_executions": recent,
        })
    }

    /// The device registry as the dashboard and device manager show it (spec §12.3).
    pub fn devices(&self) -> Value {
        json!({ "devices": self.handle.snapshot().devices })
    }

    /// Filtered execution history for the timeline screen (spec §12.4).
    ///
    /// Reads through the database when one is configured, so history survives restarts; otherwise
    /// the in-memory ring of recent executions.
    #[allow(clippy::too_many_arguments)]
    pub fn executions(
        &self,
        rule: Option<String>,
        status: Option<String>,
        device: Option<String>,
        since_ms: Option<u64>,
        until_ms: Option<u64>,
        simulated: Option<bool>,
        limit: Option<usize>,
    ) -> Result<Value, String> {
        let mut filter = Filter {
            rule,
            device,
            ..Filter::default()
        };
        if let Some(status) = status {
            filter.status = Some(
                serde_json::from_value::<ExecutionStatus>(Value::String(status.clone()))
                    .map_err(|_| format!("unknown status `{status}`"))?,
            );
        }
        filter.since_ms = since_ms;
        filter.until_ms = until_ms;
        filter.simulated = simulated;
        Ok(executions_json(
            &self.handle,
            &filter,
            limit.unwrap_or(100).clamp(1, 1000),
        ))
    }

    /// The incident log (`log.incident` output), newest first.
    pub fn incidents(&self, limit: Option<usize>) -> Result<Value, String> {
        let limit = limit.unwrap_or(100).clamp(1, 1000);
        match &self.handle.store {
            Some(store) => {
                let store = store.lock().unwrap_or_else(|e| e.into_inner());
                let incidents = store.incidents(limit).map_err(|e| e.to_string())?;
                Ok(serde_json::to_value(incidents).unwrap_or_else(|_| json!([])))
            }
            None => Ok(json!([])),
        }
    }

    /// Saved pack revisions, newest first (spec §15 version history).
    pub fn pack_history(&self) -> Result<Value, String> {
        match &self.handle.store {
            Some(store) => {
                let store = store.lock().unwrap_or_else(|e| e.into_inner());
                let history = store.pack_history().map_err(|e| e.to_string())?;
                Ok(json!(history
                    .into_iter()
                    .map(|v| json!({
                        "id": v.id,
                        "name": v.name,
                        "revision": v.revision,
                        "saved_at_ms": v.saved_at_ms,
                    }))
                    .collect::<Vec<_>>()))
            }
            None => Ok(json!([])),
        }
    }

    /// Restores a saved pack revision.
    pub fn restore_pack_version(&self, id: i64) -> Result<Value, String> {
        let yaml = match &self.handle.store {
            Some(store) => {
                let store = store.lock().unwrap_or_else(|e| e.into_inner());
                match store.load_pack_version(id).map_err(|e| e.to_string())? {
                    Some(pack) => pack.to_yaml_string().map_err(|e| e.to_string())?,
                    None => return Err(format!("no saved pack version {id}")),
                }
            }
            None => return Err("persistence is disabled".to_string()),
        };
        self.control(ControlRequest::LoadPack(yaml))
    }

    /// The current pack as YAML — the single representation the builder edits (spec §12.2).
    pub fn pack_yaml(&self) -> Result<Value, String> {
        self.control(ControlRequest::PackYaml)
    }

    /// The current pack as JSON — what the builder canvas renders from.
    pub fn pack_json(&self) -> Result<Value, String> {
        self.control(ControlRequest::PackJson)
    }

    /// Validates a candidate pack without applying it.
    pub fn validate_pack(&self, yaml: &str) -> Result<Value, String> {
        self.control(ControlRequest::ValidatePack(yaml.to_owned()))
    }

    /// Validates and hot-loads a candidate pack (spec §12.2 round-trip).
    pub fn apply_pack(&self, yaml: &str) -> Result<Value, String> {
        self.control(ControlRequest::LoadPack(yaml.to_owned()))
    }

    /// Inserts or replaces one rule, given as a JSON object in the same shape as the YAML rule.
    pub fn upsert_rule(&self, rule: Value) -> Result<Value, String> {
        self.control(ControlRequest::UpsertRule(rule))
    }

    /// Removes one rule from the current pack.
    pub fn remove_rule(&self, id: &str) -> Result<Value, String> {
        self.control(ControlRequest::RemoveRule(id.to_owned()))
    }

    /// Arms a rule.
    pub fn arm(&self, id: &str) -> Result<Value, String> {
        self.control(ControlRequest::Arm(id.to_owned()))
    }

    /// Disarms a rule.
    pub fn disarm(&self, id: &str) -> Result<Value, String> {
        self.control(ControlRequest::Disarm(id.to_owned()))
    }

    /// Runs a rule now (manual trigger semantics).
    pub fn run_rule(&self, id: &str) -> Result<Value, String> {
        self.control(ControlRequest::Run(id.to_owned()))
    }

    /// Cancels a running action chain (spec §10.2).
    pub fn cancel_execution(&self, execution_id: &str) -> Result<Value, String> {
        if self.handle.cancel_handle().cancel(execution_id) {
            Ok(json!({ "execution": execution_id, "cancelled": true }))
        } else {
            Err(format!("no running execution `{execution_id}`"))
        }
    }

    /// Switches the engine between live and simulation. The mode is always visible in the UI
    /// (spec §12.5); the switch takes effect at runtime without a restart.
    pub fn set_simulation(&self, on: bool) -> Result<Value, String> {
        self.control(ControlRequest::Simulation(on))
    }

    /// Probes a device from the device manager (spec §12.3).
    pub fn ping_device(&self, id: &str) -> Result<Value, String> {
        self.control(ControlRequest::PingDevice(id.to_owned()))
    }

    /// The catalogue of triggers, conditions and actions the builder can insert, each with a
    /// canonical valid example. A test pins every example to the real model types, so the
    /// catalogue cannot drift out of the rule format (spec §12.2).
    pub fn catalogue() -> Vec<CatalogueGroup> {
        vec![
            CatalogueGroup {
                kind: "trigger",
                label: "Triggers",
                entries: vec![
                    CatalogueEntry {
                        kind: "schedule",
                        label: "Schedule (fixed time, cron, interval or solar)",
                        example: json!({
                            "type": "schedule",
                            "at": "18:55",
                            "days": ["mon", "tue", "wed", "thu", "fri"],
                        }),
                    },
                    CatalogueEntry {
                        kind: "osc",
                        label: "OSC message",
                        example: json!({
                            "type": "osc",
                            "address": "/cue/go",
                            "arg_equals": 1.0,
                        }),
                    },
                    CatalogueEntry {
                        kind: "midi",
                        label: "MIDI message (1.0 or 2.0/UMP)",
                        example: json!({
                            "type": "midi",
                            "message": "note_on",
                            "channel": 0,
                            "number": 60,
                        }),
                    },
                    CatalogueEntry {
                        kind: "dmx",
                        label: "DMX channel change",
                        example: json!({
                            "type": "dmx",
                            "universe": 1,
                            "channel": 0,
                            "comparison": "crossed_above",
                            "value": 128,
                        }),
                    },
                    CatalogueEntry {
                        kind: "device_state",
                        label: "Device health transition",
                        example: json!({
                            "type": "device_state",
                            "device": "projector-1",
                            "state": "offline",
                        }),
                    },
                    CatalogueEntry {
                        kind: "heartbeat_missed",
                        label: "Heartbeat missed",
                        example: json!({
                            "type": "heartbeat_missed",
                            "device": "switcher-1",
                            "after_ms": 5000,
                        }),
                    },
                    CatalogueEntry {
                        kind: "device_parameter",
                        label: "Device parameter change",
                        example: json!({
                            "type": "device_parameter",
                            "device": "amp-1",
                            "parameter": "temperature",
                            "comparison": "gt",
                            "value": 60.0,
                        }),
                    },
                    CatalogueEntry {
                        kind: "manual",
                        label: "Operator \"run now\"",
                        example: json!({ "type": "manual" }),
                    },
                    CatalogueEntry {
                        kind: "api",
                        label: "API / CLI invocation",
                        example: json!({ "type": "api", "name": "panic-run" }),
                    },
                ],
            },
            CatalogueGroup {
                kind: "condition",
                label: "Conditions",
                entries: vec![
                    CatalogueEntry {
                        kind: "device_health",
                        label: "Device health is",
                        example: json!({
                            "type": "device_health",
                            "device": "projector-1",
                            "equals": "online",
                        }),
                    },
                    CatalogueEntry {
                        kind: "time_window",
                        label: "Time of day inside window",
                        example: json!({ "type": "time_window", "from": "09:00", "to": "17:00" }),
                    },
                    CatalogueEntry {
                        kind: "dmx_channel",
                        label: "DMX channel comparison",
                        example: json!({
                            "type": "dmx_channel",
                            "universe": 1,
                            "channel": 10,
                            "comparison": "gt",
                            "value": 128.0,
                        }),
                    },
                    CatalogueEntry {
                        kind: "device_parameter",
                        label: "Device parameter comparison",
                        example: json!({
                            "type": "device_parameter",
                            "device": "amp-1",
                            "parameter": "temperature",
                            "comparison": "lt",
                            "value": 80.0,
                        }),
                    },
                    CatalogueEntry {
                        kind: "rule_armed",
                        label: "Another rule is armed",
                        example: json!({ "type": "rule_armed", "rule": "other-rule" }),
                    },
                    CatalogueEntry {
                        kind: "rule_has_tag",
                        label: "Rule has tag",
                        example: json!({ "type": "rule_has_tag", "tag": "opening" }),
                    },
                ],
            },
            CatalogueGroup {
                kind: "action",
                label: "Actions",
                entries: vec![
                    CatalogueEntry {
                        kind: "control.osc",
                        label: "Send OSC",
                        example: json!({
                            "type": "control.osc",
                            "address": "/projector/1/power",
                            "args": [1.0],
                        }),
                    },
                    CatalogueEntry {
                        kind: "control.midi",
                        label: "Send MIDI (1.0 or 2.0/UMP)",
                        example: json!({
                            "type": "control.midi",
                            "channel": 0,
                            "kind": "cc",
                            "number": 1,
                            "value": 64,
                        }),
                    },
                    CatalogueEntry {
                        kind: "control.dmx_channels",
                        label: "Set DMX channels",
                        example: json!({
                            "type": "control.dmx_channels",
                            "universe": 1,
                            "start_channel": 0,
                            "values": [255, 255, 0],
                        }),
                    },
                    CatalogueEntry {
                        kind: "control.dmx_scene",
                        label: "Recall DMX scene",
                        example: json!({
                            "type": "control.dmx_scene",
                            "scene": "house-to-half",
                            "fade_ms": 2000,
                        }),
                    },
                    CatalogueEntry {
                        kind: "control.dmx_universe",
                        label: "Send DMX universe snapshot",
                        example: json!({
                            "type": "control.dmx_universe",
                            "universe": 1,
                            "values": Value::Array(vec![Value::from(0u8); 512]),
                        }),
                    },
                    CatalogueEntry {
                        kind: "media.video_source",
                        label: "Video source start/stop/switch",
                        example: json!({
                            "type": "media.video_source",
                            "source": "display-1-input",
                            "operation": "switch",
                        }),
                    },
                    CatalogueEntry {
                        kind: "media.audio_route",
                        label: "Audio route start/stop",
                        example: json!({
                            "type": "media.audio_route",
                            "from": "main-pa",
                            "to": "backup-pa",
                            "operation": "start",
                        }),
                    },
                    CatalogueEntry {
                        kind: "notify.operator",
                        label: "Notify operator",
                        example: json!({
                            "type": "notify.operator",
                            "message": "Main display lost signal",
                            "severity": "high",
                        }),
                    },
                    CatalogueEntry {
                        kind: "log.incident",
                        label: "Log incident",
                        example: json!({
                            "type": "log.incident",
                            "severity": "high",
                            "message": "Failover ran",
                        }),
                    },
                    CatalogueEntry {
                        kind: "workflow.wait",
                        label: "Wait",
                        example: json!({ "type": "workflow.wait", "ms": 1000 }),
                    },
                    CatalogueEntry {
                        kind: "workflow.invoke_rule",
                        label: "Invoke another rule",
                        example: json!({ "type": "workflow.invoke_rule", "rule": "other-rule" }),
                    },
                    CatalogueEntry {
                        kind: "workflow.exec",
                        label: "Run external program (sandboxed)",
                        example: json!({
                            "type": "workflow.exec",
                            "program": "C:/tools/report.exe",
                            "args": [],
                            "timeout_ms": 5000,
                        }),
                    },
                    CatalogueEntry {
                        kind: "workflow.use_action",
                        label: "Use action-library fragment",
                        example: json!({ "type": "workflow.use_action", "library": "key" }),
                    },
                ],
            },
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tpt_app_av_automation_model::{ActionStep, Condition, Trigger};

    /// Every catalogue example must deserialize into the real model type. If the rule format and
    /// the catalogue ever drift, a rule built from a catalogue entry would be rejected — this test
    /// fails first, at build time.
    #[test]
    fn every_catalogue_example_parses_as_its_model_type() {
        let catalogue = UiBridge::catalogue();
        assert_eq!(catalogue.len(), 3);
        for group in &catalogue {
            assert!(!group.entries.is_empty(), "{} group is empty", group.kind);
            for entry in &group.entries {
                let text = serde_json::to_string(&entry.example).unwrap();
                match group.kind {
                    "trigger" => {
                        serde_json::from_str::<Trigger>(&text).unwrap_or_else(|e| {
                            panic!("trigger `{}` does not parse: {e}", entry.kind)
                        });
                    }
                    "condition" => {
                        serde_json::from_str::<Condition>(&text).unwrap_or_else(|e| {
                            panic!("condition `{}` does not parse: {e}", entry.kind)
                        });
                    }
                    "action" => {
                        serde_json::from_str::<ActionStep>(&text).unwrap_or_else(|e| {
                            panic!("action `{}` does not parse: {e}", entry.kind)
                        });
                    }
                    other => panic!("unknown catalogue group `{other}`"),
                }
            }
        }
    }

    /// The catalogue must cover the whole MVP matrix: 9 triggers, 6 conditions, 13 actions
    /// (spec §7, §8).
    #[test]
    fn catalogue_covers_every_mvp_type() {
        let catalogue = UiBridge::catalogue();
        let count = |kind: &str| {
            catalogue
                .iter()
                .find(|g| g.kind == kind)
                .map(|g| g.entries.len())
                .unwrap_or(0)
        };
        assert_eq!(count("trigger"), 9, "trigger catalogue is incomplete");
        assert_eq!(count("condition"), 6, "condition catalogue is incomplete");
        assert_eq!(count("action"), 13, "action catalogue is incomplete");
    }
}
