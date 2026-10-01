//! SQLite persistence (spec §15).
//!
//! Stores rule packs with version history, the device registry, execution history, user
//! preferences and the engine's restart state. It stores *summaries and references*, never raw
//! protocol payload dumps.

use std::path::Path;

use rusqlite::{params, Connection, OptionalExtension};
use tpt_app_av_automation_core::{Error, Result};
use tpt_app_av_automation_devices::DeviceFile;
use tpt_app_av_automation_engine::EngineState;
use tpt_app_av_automation_model::{ExecutionRecord, RulePack};
use tpt_app_av_automation_report::Filter;

const SCHEMA_VERSION: i64 = 1;

fn storage(e: rusqlite::Error) -> Error {
    Error::Storage(e.to_string())
}

fn json_err(e: serde_json::Error) -> Error {
    Error::Storage(format!("json: {e}"))
}

/// One saved revision of a rule pack.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackVersion {
    /// Database id, increasing with each saved revision.
    pub id: i64,
    /// Pack name.
    pub name: String,
    /// Pack revision number from the YAML.
    pub revision: u32,
    /// When it was saved, epoch milliseconds.
    pub saved_at_ms: u64,
}

/// One entry of the incident log (`log.incident`, spec §8.3).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct StoredIncident {
    /// When it was logged, epoch milliseconds.
    pub at_ms: u64,
    /// Rule that logged it.
    pub rule_id: String,
    /// Execution it belongs to.
    pub execution_id: String,
    /// `info`, `normal`, `high` or `critical`.
    pub severity: String,
    /// What happened.
    pub message: String,
}

/// The application database.
#[derive(Debug)]
pub struct Store {
    conn: Connection,
}

impl Store {
    /// Opens (creating if needed) a database file.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        if let Some(parent) = path.as_ref().parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| Error::Io(format!("{}: {e}", parent.display())))?;
            }
        }
        let conn = Connection::open(path.as_ref()).map_err(storage)?;
        Self::init(conn)
    }

    /// An in-memory database, for tests.
    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory().map_err(storage)?)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.busy_timeout(std::time::Duration::from_secs(5)).map_err(storage)?;
        // WAL survives a killed process without corruption and keeps readers unblocked.
        let _: String = conn
            .query_row("PRAGMA journal_mode = WAL", [], |r| r.get(0))
            .map_err(storage)?;
        conn.execute_batch(
            "PRAGMA synchronous = NORMAL;
             CREATE TABLE IF NOT EXISTS schema_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS rule_packs (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 name TEXT NOT NULL,
                 revision INTEGER NOT NULL,
                 yaml TEXT NOT NULL,
                 saved_at_ms INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS devices (
                 id TEXT PRIMARY KEY,
                 config_json TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS executions (
                 seq INTEGER PRIMARY KEY AUTOINCREMENT,
                 execution_id TEXT NOT NULL,
                 rule_id TEXT NOT NULL,
                 status TEXT NOT NULL,
                 simulated INTEGER NOT NULL,
                 triggered_at_ms INTEGER NOT NULL,
                 record_json TEXT NOT NULL
             );
             CREATE INDEX IF NOT EXISTS executions_rule ON executions(rule_id, seq);
             CREATE INDEX IF NOT EXISTS executions_time ON executions(triggered_at_ms);
             CREATE TABLE IF NOT EXISTS incidents (
                 seq INTEGER PRIMARY KEY AUTOINCREMENT,
                 at_ms INTEGER NOT NULL,
                 rule_id TEXT NOT NULL,
                 execution_id TEXT NOT NULL,
                 severity TEXT NOT NULL,
                 message TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS preferences (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS engine_state (
                 id INTEGER PRIMARY KEY CHECK (id = 1),
                 json TEXT NOT NULL
             );",
        )
        .map_err(storage)?;
        let version: Option<String> = conn
            .query_row("SELECT value FROM schema_meta WHERE key = 'version'", [], |r| r.get(0))
            .optional()
            .map_err(storage)?;
        match version {
            None => {
                conn.execute(
                    "INSERT INTO schema_meta (key, value) VALUES ('version', ?1)",
                    params![SCHEMA_VERSION.to_string()],
                )
                .map_err(storage)?;
            }
            Some(v) if v == SCHEMA_VERSION.to_string() => {}
            Some(other) => {
                return Err(Error::Storage(format!(
                    "database schema version {other} is not supported (expected {SCHEMA_VERSION})"
                )))
            }
        }
        Ok(Self { conn })
    }

    /// Saves a pack as a new version unless it is identical to the latest one.
    ///
    /// Returns the id of the current (new or unchanged) version.
    pub fn save_pack(&self, pack: &RulePack, now_ms: u64) -> Result<i64> {
        let yaml = pack.to_yaml_string()?;
        if let Some((id, latest)) = self.latest_pack_yaml()? {
            if latest == yaml {
                return Ok(id);
            }
        }
        self.conn
            .execute(
                "INSERT INTO rule_packs (name, revision, yaml, saved_at_ms) VALUES (?1, ?2, ?3, ?4)",
                params![pack.name, pack.revision, yaml, now_ms as i64],
            )
            .map_err(storage)?;
        Ok(self.conn.last_insert_rowid())
    }

    /// The latest saved pack version as `(id, yaml)`.
    pub fn latest_pack_yaml(&self) -> Result<Option<(i64, String)>> {
        self.conn
            .query_row(
                "SELECT id, yaml FROM rule_packs ORDER BY id DESC LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(storage)
    }

    /// Every saved version, newest first.
    pub fn pack_history(&self) -> Result<Vec<PackVersion>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, name, revision, saved_at_ms FROM rule_packs ORDER BY id DESC")
            .map_err(storage)?;
        let rows = stmt
            .query_map([], |r| {
                Ok(PackVersion {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    revision: r.get::<_, i64>(2)? as u32,
                    saved_at_ms: r.get::<_, i64>(3)? as u64,
                })
            })
            .map_err(storage)?;
        rows.collect::<std::result::Result<_, _>>().map_err(storage)
    }

    /// Loads a saved version's pack.
    pub fn load_pack_version(&self, id: i64) -> Result<Option<RulePack>> {
        let yaml: Option<String> = self
            .conn
            .query_row("SELECT yaml FROM rule_packs WHERE id = ?1", params![id], |r| r.get(0))
            .optional()
            .map_err(storage)?;
        yaml.map(|y| RulePack::from_yaml_str(&y)).transpose()
    }

    /// Replaces the stored device registry.
    pub fn save_devices(&mut self, devices: &DeviceFile) -> Result<()> {
        let tx = self.conn.transaction().map_err(storage)?;
        tx.execute("DELETE FROM devices", []).map_err(storage)?;
        for device in &devices.devices {
            tx.execute(
                "INSERT INTO devices (id, config_json) VALUES (?1, ?2)",
                params![device.id, serde_json::to_string(device).map_err(json_err)?],
            )
            .map_err(storage)?;
        }
        tx.commit().map_err(storage)
    }

    /// Loads the stored device registry.
    pub fn load_devices(&self) -> Result<DeviceFile> {
        let mut stmt = self
            .conn
            .prepare("SELECT config_json FROM devices ORDER BY id")
            .map_err(storage)?;
        let rows = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(storage)?;
        let mut devices = Vec::new();
        for row in rows {
            devices.push(serde_json::from_str(&row.map_err(storage)?).map_err(json_err)?);
        }
        Ok(DeviceFile { devices })
    }

    /// Appends an execution to the history.
    pub fn append_execution(&self, record: &ExecutionRecord) -> Result<()> {
        self.conn
            .execute(
                "INSERT INTO executions
                 (execution_id, rule_id, status, simulated, triggered_at_ms, record_json)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    record.execution_id,
                    record.rule_id.as_str(),
                    record.overall_status.as_str(),
                    record.simulated,
                    record.triggered_at_ms as i64,
                    serde_json::to_string(record).map_err(json_err)?,
                ],
            )
            .map_err(storage)?;
        Ok(())
    }

    /// Recent executions matching `filter`, newest first, at most `limit`.
    pub fn executions(&self, filter: &Filter, limit: usize) -> Result<Vec<ExecutionRecord>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT record_json FROM executions
                 WHERE (?1 IS NULL OR rule_id = ?1)
                   AND (?2 IS NULL OR status = ?2)
                   AND (?3 IS NULL OR triggered_at_ms >= ?3)
                   AND (?4 IS NULL OR triggered_at_ms < ?4)
                   AND (?5 IS NULL OR simulated = ?5)
                 ORDER BY seq DESC",
            )
            .map_err(storage)?;
        let rows = stmt
            .query_map(
                params![
                    filter.rule,
                    filter.status.map(|s| s.as_str()),
                    filter.since_ms.map(|v| v as i64),
                    filter.until_ms.map(|v| v as i64),
                    filter.simulated,
                ],
                |r| r.get::<_, String>(0),
            )
            .map_err(storage)?;
        let mut out = Vec::new();
        for row in rows {
            let record: ExecutionRecord =
                serde_json::from_str(&row.map_err(storage)?).map_err(json_err)?;
            // The device filter needs the per-action detail, so it is applied on the decoded row.
            if filter.matches(&record) {
                out.push(record);
                if out.len() >= limit {
                    break;
                }
            }
        }
        Ok(out)
    }

    /// Total stored executions.
    pub fn execution_count(&self) -> Result<u64> {
        self.conn
            .query_row("SELECT COUNT(*) FROM executions", [], |r| r.get::<_, i64>(0))
            .map(|n| n as u64)
            .map_err(storage)
    }

    /// Deletes the oldest executions beyond `keep`; returns how many were removed.
    pub fn prune_executions(&self, keep: u64) -> Result<u64> {
        self.conn
            .execute(
                "DELETE FROM executions WHERE seq <= (SELECT MAX(seq) FROM executions) - ?1",
                params![keep as i64],
            )
            .map(|n| n as u64)
            .map_err(storage)
    }

    /// Appends an incident to the incident log.
    pub fn append_incident(&self, at_ms: u64, incident: &tpt_app_av_automation_actions::Incident) -> Result<()> {
        self.conn
            .execute(
                "INSERT INTO incidents (at_ms, rule_id, execution_id, severity, message)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    at_ms as i64,
                    incident.rule_id,
                    incident.execution_id,
                    incident.severity.as_str(),
                    incident.message
                ],
            )
            .map_err(storage)?;
        Ok(())
    }

    /// The most recent incidents, newest first.
    pub fn incidents(&self, limit: usize) -> Result<Vec<StoredIncident>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT at_ms, rule_id, execution_id, severity, message
                 FROM incidents ORDER BY seq DESC LIMIT ?1",
            )
            .map_err(storage)?;
        let rows = stmt
            .query_map(params![limit as i64], |r| {
                Ok(StoredIncident {
                    at_ms: r.get::<_, i64>(0)? as u64,
                    rule_id: r.get(1)?,
                    execution_id: r.get(2)?,
                    severity: r.get(3)?,
                    message: r.get(4)?,
                })
            })
            .map_err(storage)?;
        rows.collect::<std::result::Result<_, _>>().map_err(storage)
    }

    /// Reads a user preference.
    pub fn preference(&self, key: &str) -> Result<Option<String>> {
        self.conn
            .query_row("SELECT value FROM preferences WHERE key = ?1", params![key], |r| r.get(0))
            .optional()
            .map_err(storage)
    }

    /// Writes a user preference.
    pub fn set_preference(&self, key: &str, value: &str) -> Result<()> {
        self.conn
            .execute(
                "INSERT INTO preferences (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![key, value],
            )
            .map_err(storage)?;
        Ok(())
    }

    /// Persists the engine's restart state.
    pub fn save_state(&self, state: &EngineState) -> Result<()> {
        self.conn
            .execute(
                "INSERT INTO engine_state (id, json) VALUES (1, ?1)
                 ON CONFLICT(id) DO UPDATE SET json = excluded.json",
                params![serde_json::to_string(state).map_err(json_err)?],
            )
            .map_err(storage)?;
        Ok(())
    }

    /// Loads the engine's restart state, if any was saved.
    pub fn load_state(&self) -> Result<Option<EngineState>> {
        let json: Option<String> = self
            .conn
            .query_row("SELECT json FROM engine_state WHERE id = 1", [], |r| r.get(0))
            .optional()
            .map_err(storage)?;
        json.map(|j| serde_json::from_str(&j).map_err(json_err)).transpose()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tpt_app_av_automation_core::{Event, RuleId};
    use tpt_app_av_automation_model::ExecutionStatus;

    const PACK: &str = "format_version: 1\nname: P\nrevision: 1\nrules:\n  - id: r\n    name: R\n    trigger: { type: manual }\n    actions:\n      - { id: a, type: notify.operator, message: hi }\n";

    fn record(id: &str, rule: &str, status: ExecutionStatus, at: u64, simulated: bool) -> ExecutionRecord {
        ExecutionRecord {
            execution_id: id.into(),
            rule_id: RuleId::new(rule),
            rule_name: rule.into(),
            rule_version: 1,
            triggered_at_ms: at,
            trigger_detail: Event::Manual { rule: None },
            trigger_type: "manual".into(),
            condition_results: vec![],
            action_results: vec![],
            overall_status: status,
            simulated,
            duration_ms: 0,
        }
    }

    #[test]
    fn pack_versions_are_kept_and_identical_saves_are_deduplicated() {
        let store = Store::open_in_memory().unwrap();
        let pack = RulePack::from_yaml_str(PACK).unwrap();
        let first = store.save_pack(&pack, 1).unwrap();
        assert_eq!(store.save_pack(&pack, 2).unwrap(), first, "unchanged pack is not a new version");
        let mut edited = pack.clone();
        edited.revision = 2;
        let second = store.save_pack(&edited, 3).unwrap();
        assert_ne!(first, second);
        let history = store.pack_history().unwrap();
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].revision, 2, "newest first");
        assert_eq!(store.load_pack_version(first).unwrap().unwrap().revision, 1);
        assert!(store.load_pack_version(999).unwrap().is_none());
    }

    #[test]
    fn devices_round_trip() {
        let mut store = Store::open_in_memory().unwrap();
        let file = DeviceFile::from_yaml_str(
            "devices:\n  - {id: a, protocol: virtual}\n  - {id: b, protocol: osc, address: '127.0.0.1:9', heartbeat_ms: 500}\n",
        )
        .unwrap();
        store.save_devices(&file).unwrap();
        assert_eq!(store.load_devices().unwrap(), file);
        store.save_devices(&DeviceFile::default()).unwrap();
        assert!(store.load_devices().unwrap().devices.is_empty());
    }

    #[test]
    fn executions_are_filterable_and_newest_first() {
        let store = Store::open_in_memory().unwrap();
        store.append_execution(&record("e1", "a", ExecutionStatus::Success, 100, false)).unwrap();
        store.append_execution(&record("e2", "b", ExecutionStatus::Failed, 200, true)).unwrap();
        store.append_execution(&record("e3", "a", ExecutionStatus::PartialFailure, 300, false)).unwrap();
        let all = store.executions(&Filter::default(), 10).unwrap();
        let ids: Vec<_> = all.iter().map(|r| r.execution_id.as_str()).collect();
        assert_eq!(ids, ["e3", "e2", "e1"]);
        let only_a = store.executions(&Filter { rule: Some("a".into()), ..Filter::default() }, 10).unwrap();
        assert_eq!(only_a.len(), 2);
        let failed = store.executions(&Filter { status: Some(ExecutionStatus::Failed), ..Filter::default() }, 10).unwrap();
        assert_eq!(failed[0].execution_id, "e2");
        let window = store.executions(&Filter { since_ms: Some(150), until_ms: Some(250), ..Filter::default() }, 10).unwrap();
        assert_eq!(window.len(), 1);
        let sim = store.executions(&Filter { simulated: Some(true), ..Filter::default() }, 10).unwrap();
        assert_eq!(sim.len(), 1);
        assert_eq!(store.executions(&Filter::default(), 2).unwrap().len(), 2, "limit honoured");
    }

    #[test]
    fn pruning_keeps_the_newest() {
        let store = Store::open_in_memory().unwrap();
        for i in 0..10 {
            store.append_execution(&record(&format!("e{i}"), "a", ExecutionStatus::Success, i, false)).unwrap();
        }
        assert_eq!(store.prune_executions(3).unwrap(), 7);
        assert_eq!(store.execution_count().unwrap(), 3);
        let left = store.executions(&Filter::default(), 10).unwrap();
        assert_eq!(left[0].execution_id, "e9");
        assert_eq!(left[2].execution_id, "e7");
    }

    #[test]
    fn incidents_are_logged_newest_first() {
        use tpt_app_av_automation_actions::Incident;
        use tpt_app_av_automation_model::AlertSeverity;
        let store = Store::open_in_memory().unwrap();
        for (i, severity) in [AlertSeverity::Info, AlertSeverity::Critical].into_iter().enumerate() {
            store
                .append_incident(
                    100 + i as u64,
                    &Incident {
                        severity,
                        message: format!("m{i}"),
                        rule_id: "r".into(),
                        execution_id: format!("e{i}"),
                    },
                )
                .unwrap();
        }
        let all = store.incidents(10).unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!((all[0].message.as_str(), all[0].severity.as_str(), all[0].at_ms), ("m1", "critical", 101));
        assert_eq!(store.incidents(1).unwrap().len(), 1);
    }

    #[test]
    fn preferences_and_engine_state_round_trip() {
        let store = Store::open_in_memory().unwrap();
        assert_eq!(store.preference("theme").unwrap(), None);
        store.set_preference("theme", "dark").unwrap();
        store.set_preference("theme", "light").unwrap();
        assert_eq!(store.preference("theme").unwrap().as_deref(), Some("light"));

        assert!(store.load_state().unwrap().is_none());
        let mut state = EngineState::default();
        state.next_execution = 42;
        state.armed.insert("r".into(), true);
        store.save_state(&state).unwrap();
        store.save_state(&state).unwrap();
        assert_eq!(store.load_state().unwrap().unwrap(), state);
    }

    #[test]
    fn state_survives_reopening_a_file_database() {
        let dir = std::env::temp_dir().join(format!("tpt-av-store-{}", std::process::id()));
        let path = dir.join("nested").join("state.db");
        {
            let store = Store::open(&path).unwrap();
            store.set_preference("k", "v").unwrap();
        }
        let store = Store::open(&path).unwrap();
        assert_eq!(store.preference("k").unwrap().as_deref(), Some("v"));
        drop(store);
        let _ = std::fs::remove_dir_all(dir);
    }
}
