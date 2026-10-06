//! The desktop UI bridge against a running service: every screen's backing logic (spec §12).

use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde_json::json;
use tpt_app_av_automation_core::SystemClock;
use tpt_app_av_automation_devices::DeviceFile;
use tpt_app_av_automation_model::RulePack;
use tpt_app_av_automation_service::{Service, ServiceConfig, Store, UiBridge};

const PACK: &str = r#"
format_version: 1
name: Bridge Test
revision: 1
rules:
  - id: show
    name: Show
    armed: false
    trigger: { type: osc, address: /go }
    actions:
      - { id: power, type: control.osc, device: proj, address: /power, args: [1] }
      - { id: note, type: notify.operator, message: hello }
"#;

const DEVICES: &str = "devices:\n  - { id: proj, protocol: virtual }\n";

struct Running {
    bridge: UiBridge,
    thread: Option<JoinHandle<()>>,
}

impl Running {
    fn start(pack: &str, devices: &str, store: Option<Store>) -> Self {
        Self::start_with_devices_path(pack, devices, store, None)
    }

    fn start_with_devices_path(
        pack: &str,
        devices: &str,
        store: Option<Store>,
        devices_path: Option<std::path::PathBuf>,
    ) -> Self {
        let config = ServiceConfig {
            tick_ms: 10,
            ..ServiceConfig::default()
        };
        let mut service = Service::build(
            RulePack::from_yaml_str(pack).unwrap(),
            DeviceFile::from_yaml_str(devices).unwrap(),
            config,
            store,
            Arc::new(SystemClock),
        )
        .expect("service should build");
        if let Some(path) = devices_path {
            service.set_devices_path(path);
        }
        let bridge = UiBridge::new(service.handle());
        let thread = std::thread::spawn(move || service.run().expect("service run"));
        assert!(
            bridge.handle().wait_ready(Duration::from_secs(10)),
            "service did not become ready"
        );
        Self {
            bridge,
            thread: Some(thread),
        }
    }

    fn stop(&mut self) {
        self.bridge.handle().shutdown();
        if let Some(t) = self.thread.take() {
            t.join().unwrap();
        }
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        self.stop();
    }
}

fn wait_until(what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if condition() {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("timed out waiting for {what}");
}

fn dashboard_rule_count(running: &Running) -> usize {
    running.bridge.dashboard()["status"]["rules"]
        .as_u64()
        .unwrap_or(0) as usize
}

#[test]
fn the_dashboard_reports_status_rules_devices_and_recent_executions() {
    let running = Running::start(PACK, DEVICES, None);
    let board = running.bridge.dashboard();
    assert_eq!(board["status"]["status"], "nominal");
    assert_eq!(board["status"]["mode"], "live");
    assert_eq!(board["status"]["rules"], 1);
    assert_eq!(board["status"]["devices"]["total"], 1);

    running.bridge.run_rule("show").unwrap();
    wait_until("the execution to reach the dashboard", || {
        !running.bridge.dashboard()["recent_executions"]
            .as_array()
            .unwrap()
            .is_empty()
    });
    let board = running.bridge.dashboard();
    let record = &board["recent_executions"][0];
    assert_eq!(record["rule_id"], "show");
    assert_eq!(record["simulated"], true, "disarmed rules simulate");
    assert_eq!(record["overall_status"], "success");
    assert_eq!(record["action_results"].as_array().unwrap().len(), 2);
}

#[test]
fn pack_yaml_round_trips_through_the_bridge() {
    let mut running = Running::start(PACK, DEVICES, None);
    let yaml = running
        .bridge
        .pack_yaml()
        .unwrap()
        .as_str()
        .unwrap()
        .to_owned();
    let reparsed = RulePack::from_yaml_str(&yaml).unwrap();
    assert_eq!(reparsed.name, "Bridge Test");
    assert_eq!(reparsed.rules.len(), 1);
    assert_eq!(reparsed.rules[0].id.as_str(), "show");
    running.stop();
}

#[test]
fn pack_json_is_the_same_pack_the_yaml_shows() {
    let mut running = Running::start(PACK, DEVICES, None);
    let pack = running.bridge.pack_json().unwrap();
    assert_eq!(pack["name"], "Bridge Test");
    assert_eq!(pack["rules"][0]["id"], "show");
    // The canvas needs conditions and actions, which the dashboard summary leaves out.
    assert_eq!(pack["rules"][0]["actions"][0]["type"], "control.osc");
    assert_eq!(pack["rules"][0]["actions"][0]["device"], "proj");
    running.stop();
}

#[test]
fn validate_pack_reports_errors_without_applying() {
    let mut running = Running::start(PACK, DEVICES, None);

    let broken = running
        .bridge
        .validate_pack("format_version: 1\nname: broken\nrules:\n  - name: no trigger\n")
        .unwrap();
    assert_eq!(broken["valid"], false);
    assert!(
        !broken["diagnostics"].as_array().unwrap().is_empty(),
        "diagnostics are returned: {broken}"
    );

    let answer = running
        .bridge
        .validate_pack("format_version: 99\nname: future\n")
        .unwrap();
    assert_eq!(answer["valid"], false, "a future format is rejected");

    let good = running.bridge.validate_pack(PACK).unwrap();
    assert_eq!(good["valid"], true);
    assert_eq!(good["rules"], 1);

    // Nothing was applied.
    assert_eq!(dashboard_rule_count(&running), 1);
    running.stop();
}

#[test]
fn apply_pack_swaps_rules_and_preserves_armed_state() {
    let mut running = Running::start(PACK, DEVICES, None);
    running.bridge.arm("show").unwrap();
    wait_until("arm state to be visible", || {
        running.bridge.dashboard()["status"]["armed_rules"] == 1
    });

    // The unchanged rule (same id *and* version) stays armed; the new rule comes up disarmed.
    let next = format!(
        "{PACK}  - id: afterparty\n    name: After party\n    trigger: {{ type: manual }}\n    actions:\n      - {{ id: note, type: notify.operator, message: done }}\n"
    );
    let answer = running.bridge.apply_pack(&next).unwrap();
    assert_eq!(answer["valid"], true, "{answer}");
    assert_eq!(answer["rules"], 2);

    wait_until("the new rule to appear", || {
        dashboard_rule_count(&running) == 2
    });
    // Armed state per rule is read from the rules list the dashboard carries.
    assert_eq!(
        running.bridge.dashboard()["status"]["armed_rules"],
        1,
        "show is still armed"
    );

    // An empty pack parses and is applied; the swap lands asynchronously like the one above.
    let emptied = running
        .bridge
        .apply_pack("format_version: 1\nname: empty\nrules: []\n")
        .unwrap();
    assert_eq!(emptied["valid"], true, "an empty pack parses");
    wait_until("the empty pack to be applied", || {
        dashboard_rule_count(&running) == 0
    });
    // Restore for the rest of the test.
    running.bridge.apply_pack(PACK).unwrap();
    wait_until("rules to be back", || dashboard_rule_count(&running) == 1);
    running.stop();
}

#[test]
fn rules_can_be_added_and_removed_from_the_builder() {
    let mut running = Running::start(PACK, DEVICES, None);

    let rule = json!({
        "id": "new-rule",
        "name": "New Rule",
        "trigger": { "type": "manual" },
        "actions": [
            { "type": "control.osc", "device": "proj", "address": "/power", "args": [1] }
        ],
    });
    let answer = running.bridge.upsert_rule(rule).unwrap();
    assert_eq!(answer["valid"], true, "{answer}");
    wait_until("the rule to appear", || dashboard_rule_count(&running) == 2);
    // Replacing the same rule does not duplicate it.
    let updated = json!({
        "id": "new-rule",
        "name": "Renamed Rule",
        "trigger": { "type": "manual" },
        "actions": [
            { "type": "notify.operator", "message": "hi" }
        ],
    });
    running.bridge.upsert_rule(updated).unwrap();
    wait_until("the replacement to land", || {
        let yaml = running
            .bridge
            .pack_yaml()
            .unwrap()
            .as_str()
            .unwrap()
            .to_owned();
        yaml.contains("Renamed Rule")
    });
    assert_eq!(
        dashboard_rule_count(&running),
        2,
        "no duplicate was created"
    );

    // A rule writing to an unknown device is refused with diagnostics.
    let bad = json!({
        "id": "bad-rule",
        "name": "Bad Rule",
        "trigger": { "type": "manual" },
        "actions": [
            { "type": "control.osc", "device": "ghost", "address": "/x" }
        ],
    });
    let answer = running.bridge.upsert_rule(bad).unwrap();
    assert_eq!(answer["valid"], false);
    assert!(!answer["diagnostics"].as_array().unwrap().is_empty());

    // Not a rule object at all is an error, not a panic.
    assert!(running.bridge.upsert_rule(json!({ "nope": 1 })).is_err());

    running.bridge.remove_rule("new-rule").unwrap();
    wait_until("the rule to disappear", || {
        dashboard_rule_count(&running) == 1
    });
    assert!(running.bridge.remove_rule("new-rule").is_err());
    running.stop();
}

#[test]
fn a_working_rule_can_be_built_from_catalogue_examples_without_yaml() {
    // First run: an empty pack — the desktop's "new show" state.
    let pack = RulePack::new("New Show");
    let service = Service::build(
        pack,
        DeviceFile::from_yaml_str(DEVICES).unwrap(),
        ServiceConfig {
            tick_ms: 10,
            ..ServiceConfig::default()
        },
        None,
        Arc::new(SystemClock),
    )
    .expect("an empty pack builds");
    let bridge = UiBridge::new(service.handle());
    let thread = std::thread::spawn(move || service.run().expect("service run"));
    assert!(bridge.handle().wait_ready(Duration::from_secs(10)));

    // Pick a trigger and an action straight from the catalogue and assemble a rule.
    let catalogue = UiBridge::catalogue();
    let trigger = catalogue
        .iter()
        .find(|g| g.kind == "trigger")
        .unwrap()
        .entries
        .iter()
        .find(|e| e.kind == "manual")
        .unwrap()
        .example
        .clone();
    let action = catalogue
        .iter()
        .find(|g| g.kind == "action")
        .unwrap()
        .entries
        .iter()
        .find(|e| e.kind == "control.osc")
        .unwrap()
        .example
        .clone();
    let mut action = action;
    action["device"] = json!("proj");

    let rule = json!({
        "name": "Panic Run",
        "trigger": trigger,
        "actions": [action],
    });
    let answer = bridge.upsert_rule(rule).unwrap();
    assert_eq!(answer["valid"], true, "{answer}");

    let yaml = bridge.pack_yaml().unwrap().as_str().unwrap().to_owned();
    assert!(yaml.contains("Panic Run"), "{yaml}");
    // The YAML the canvas shows is exactly what the engine loaded.
    let reparsed = RulePack::from_yaml_str(&yaml).unwrap();
    assert_eq!(reparsed.rules.len(), 1);
    assert_eq!(reparsed.rules[0].name, "Panic Run");

    bridge.handle().shutdown();
    thread.join().unwrap();
}

#[test]
fn simulation_toggles_at_runtime_and_every_record_carries_the_flag() {
    let mut running = Running::start(PACK, DEVICES, None);
    assert_eq!(running.bridge.dashboard()["status"]["mode"], "live");

    running.bridge.set_simulation(true).unwrap();
    wait_until("simulation to be visible", || {
        running.bridge.dashboard()["status"]["mode"] == "simulation"
    });
    running.bridge.run_rule("show").unwrap();
    wait_until("the simulated execution to land", || {
        !running.bridge.dashboard()["recent_executions"]
            .as_array()
            .unwrap()
            .is_empty()
    });
    assert_eq!(
        running.bridge.dashboard()["recent_executions"][0]["simulated"],
        true
    );

    running.bridge.set_simulation(false).unwrap();
    wait_until("live to be visible", || {
        running.bridge.dashboard()["status"]["mode"] == "live"
    });
    let before = running.bridge.dashboard()["recent_executions"]
        .as_array()
        .unwrap()
        .len();
    running.bridge.arm("show").unwrap();
    running.bridge.run_rule("show").unwrap();
    wait_until("the live execution to land", || {
        running.bridge.dashboard()["recent_executions"]
            .as_array()
            .unwrap()
            .len()
            > before
    });
    let latest = running.bridge.dashboard()["recent_executions"][0].clone();
    assert_eq!(latest["simulated"], false);

    // The timeline can separate the two.
    let live = running
        .bridge
        .executions(None, None, None, None, None, Some(false), None)
        .unwrap();
    assert_eq!(live["executions"].as_array().unwrap().len(), 1);
    let simulated = running
        .bridge
        .executions(None, None, None, None, None, Some(true), None)
        .unwrap();
    assert_eq!(simulated["executions"].as_array().unwrap().len(), 1);
    let by_rule = running
        .bridge
        .executions(Some("show".into()), None, None, None, None, None, None)
        .unwrap();
    assert_eq!(by_rule["executions"].as_array().unwrap().len(), 2);
    assert!(running
        .bridge
        .executions(None, Some("bogus".into()), None, None, None, None, None)
        .is_err());
    running.stop();
}

#[test]
fn the_device_manager_can_ping_devices() {
    let mut running = Running::start(PACK, DEVICES, None);
    let answer = running.bridge.ping_device("proj").unwrap();
    assert_eq!(answer["device"], "proj");
    assert_eq!(answer["reachable"], true);
    assert!(running.bridge.ping_device("ghost").is_err());
    running.stop();
}

#[test]
fn pack_versions_are_saved_and_can_be_restored() {
    let dir = std::env::temp_dir().join(format!("tpt-av-ui-bridge-{}", std::process::id()));
    let db = dir.join("state.db");
    let mut running = Running::start(PACK, DEVICES, Some(Store::open(&db).unwrap()));

    assert_eq!(
        running
            .bridge
            .pack_history()
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let next = format!("{PACK}  - id: encore\n    name: Encore\n    trigger: {{ type: manual }}\n    actions:\n      - {{ id: note, type: notify.operator, message: again }}\n");
    running.bridge.apply_pack(&next).unwrap();
    wait_until("two versions saved", || {
        running
            .bridge
            .pack_history()
            .unwrap()
            .as_array()
            .unwrap()
            .len()
            == 2
    });

    // Restore the first version and the second rule goes away.
    let history = running.bridge.pack_history().unwrap();
    let first_id = history.as_array().unwrap().last().unwrap()["id"]
        .as_i64()
        .unwrap();
    let answer = running.bridge.restore_pack_version(first_id).unwrap();
    assert_eq!(answer["valid"], true, "{answer}");
    wait_until("the restored pack to run", || {
        dashboard_rule_count(&running) == 1
    });
    running.stop();
    drop(running);

    // Versions survive the process.
    let store = Store::open(&db).unwrap();
    assert_eq!(store.pack_history().unwrap().len(), 3, "restore saved too");
    drop(store);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn incidents_flow_through_the_bridge() {
    let dir = std::env::temp_dir().join(format!("tpt-av-ui-incidents-{}", std::process::id()));
    let db = dir.join("state.db");
    let pack = r#"
format_version: 1
name: Incidents
revision: 1
rules:
  - id: alarm
    name: Alarm
    armed: true
    trigger: { type: manual }
    actions:
      - { id: log, type: log.incident, severity: critical, message: "lamp failure" }
"#;
    let mut running = Running::start(pack, DEVICES, Some(Store::open(&db).unwrap()));
    running.bridge.run_rule("alarm").unwrap();
    wait_until("the incident to be persisted", || {
        running
            .bridge
            .incidents(None)
            .unwrap()
            .as_array()
            .unwrap()
            .len()
            == 1
    });
    let incidents = running.bridge.incidents(None).unwrap();
    assert_eq!(incidents[0]["severity"], "critical");
    assert_eq!(incidents[0]["rule_id"], "alarm");
    running.stop();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn cancelling_an_unknown_execution_is_an_error_not_a_crash() {
    let mut running = Running::start(PACK, DEVICES, None);
    assert!(running.bridge.cancel_execution("nope").is_err());
    running.stop();
}

#[test]
fn devices_can_be_added_edited_and_removed_and_the_file_follows() {
    let dir = std::env::temp_dir().join(format!("tpt-av-ui-devices-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("devices.yaml");
    std::fs::write(
        &path,
        "# my hand-written comment
devices:
  - { id: proj, protocol: virtual }
",
    )
    .unwrap();
    let mut running = Running::start_with_devices_path(PACK, DEVICES, None, Some(path.clone()));

    // Add.
    let added = running
        .bridge
        .upsert_device(json!({"id": "cam", "kind": "display", "protocol": "virtual"}))
        .unwrap();
    assert_eq!(added["valid"], true);
    assert!(running.bridge.ping_device("cam").unwrap()["reachable"] == true);
    let on_disk = DeviceFile::from_path(&path).unwrap();
    assert_eq!(on_disk.devices.len(), 2);
    assert!(
        std::fs::read_to_string(dir.join("devices.yaml.bak"))
            .unwrap()
            .contains("hand-written"),
        "the original is kept once"
    );

    // Edit keeps one entry per id.
    running
        .bridge
        .upsert_device(json!({"id": "cam", "name": "Camera", "protocol": "virtual"}))
        .unwrap();
    let configs = running.bridge.device_configs().unwrap();
    assert_eq!(configs["devices"].as_array().unwrap().len(), 2);
    wait_until("the snapshot shows the new name", || {
        running.bridge.devices()["devices"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["name"] == "Camera")
    });

    // Mistakes are structured answers and change nothing.
    let bad = running
        .bridge
        .upsert_device(json!({"id": "x", "protocol": "osc", "address": "nope"}))
        .unwrap();
    assert_eq!(bad["valid"], false);
    assert_eq!(DeviceFile::from_path(&path).unwrap().devices.len(), 2);
    let unknown_key = running
        .bridge
        .upsert_device(json!({"id": "y", "protocol": "virtual", "bogus": 1}))
        .unwrap();
    assert_eq!(unknown_key["valid"], false);

    // A device a rule uses cannot be removed; an unused one can.
    let refused = running.bridge.remove_device("proj").unwrap();
    assert_eq!(refused["valid"], false);
    assert_eq!(running.bridge.remove_device("cam").unwrap()["valid"], true);
    assert!(running.bridge.ping_device("cam").is_err());
    assert_eq!(DeviceFile::from_path(&path).unwrap().devices.len(), 1);
    assert!(running.bridge.remove_device("ghost").is_err());
    running.stop();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_template_adds_its_devices_and_rules_without_clobbering_existing_ones() {
    let mut running = Running::start(PACK, DEVICES, None);
    assert!(UiBridge::templates().as_array().unwrap().len() >= 5);

    let first = running.bridge.apply_template("starter").unwrap();
    assert_eq!(first["valid"], true, "{first}");
    assert_eq!(first["added_rules"].as_array().unwrap().len(), 2);
    assert_eq!(first["added_devices"].as_array().unwrap().len(), 2);
    let pack = running.bridge.pack_json().unwrap();
    assert_eq!(
        pack["rules"].as_array().unwrap().len(),
        3,
        "the existing rule is kept"
    );
    assert!(pack["rules"]
        .as_array()
        .unwrap()
        .iter()
        .all(|r| r["armed"] != true || r["id"] == "show"));

    // Applying it again adds renamed copies of the rules but no duplicate devices.
    let again = running.bridge.apply_template("starter").unwrap();
    assert_eq!(again["added_devices"].as_array().unwrap().len(), 0);
    let ids: Vec<String> = running.bridge.pack_json().unwrap()["rules"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap().to_owned())
        .collect();
    let unique: std::collections::HashSet<_> = ids.iter().collect();
    assert_eq!(ids.len(), unique.len(), "ids stay unique: {ids:?}");

    assert!(running.bridge.apply_template("nope").is_err());
    running.stop();
}
