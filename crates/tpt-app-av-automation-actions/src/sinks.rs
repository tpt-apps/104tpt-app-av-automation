//! Notification, incident-log and sleep abstractions used by actions.

use std::sync::Mutex;
use std::time::Duration;

use tpt_app_av_automation_model::AlertSeverity;

use crate::CancelToken;

/// An operator alert.
#[derive(Debug, Clone, PartialEq)]
pub struct Notice {
    /// Severity.
    pub severity: AlertSeverity,
    /// Message text.
    pub message: String,
    /// Rule that raised it.
    pub rule_id: String,
}

/// An incident log entry.
#[derive(Debug, Clone, PartialEq)]
pub struct Incident {
    /// Severity.
    pub severity: AlertSeverity,
    /// Message text.
    pub message: String,
    /// Rule that raised it.
    pub rule_id: String,
    /// Execution it belongs to.
    pub execution_id: String,
}

/// Delivers operator alerts (desktop toast, console, sound, ...).
pub trait Notifier: Send + Sync {
    /// Delivers an alert.
    fn notify(&self, notice: Notice) -> Result<(), String>;
}

/// Persists incident log entries.
pub trait IncidentSink: Send + Sync {
    /// Records an incident.
    fn record(&self, incident: Incident) -> Result<(), String>;
}

/// Pauses a chain. Abstracted so tests and simulations never really sleep.
pub trait Sleeper: Send + Sync {
    /// Sleeps for `ms`, returning early when `cancel` fires.
    fn sleep(&self, ms: u64, cancel: &CancelToken);
}

/// Real sleeping in short slices, so cancellation is honoured within ~20 ms.
#[derive(Debug, Clone, Copy, Default)]
pub struct ThreadSleeper;

impl Sleeper for ThreadSleeper {
    fn sleep(&self, ms: u64, cancel: &CancelToken) {
        let mut remaining = ms;
        while remaining > 0 && !cancel.is_cancelled() {
            let slice = remaining.min(20);
            std::thread::sleep(Duration::from_millis(slice));
            remaining -= slice;
        }
    }
}

/// A sleeper that returns immediately.
#[derive(Debug, Clone, Copy, Default)]
pub struct NullSleeper;

impl Sleeper for NullSleeper {
    fn sleep(&self, _ms: u64, _cancel: &CancelToken) {}
}

/// Writes alerts and incidents to the `tracing` log.
#[derive(Debug, Clone, Copy, Default)]
pub struct TracingSink;

impl Notifier for TracingSink {
    fn notify(&self, notice: Notice) -> Result<(), String> {
        tracing::warn!(rule = %notice.rule_id, severity = notice.severity.as_str(), "operator alert: {}", notice.message);
        Ok(())
    }
}

impl IncidentSink for TracingSink {
    fn record(&self, incident: Incident) -> Result<(), String> {
        tracing::error!(rule = %incident.rule_id, execution = %incident.execution_id, severity = incident.severity.as_str(), "incident: {}", incident.message);
        Ok(())
    }
}

/// Keeps alerts and incidents in memory. Used by tests, and by shells that render them afterwards.
#[derive(Debug, Default)]
pub struct MemorySink {
    notices: Mutex<Vec<Notice>>,
    incidents: Mutex<Vec<Incident>>,
}

impl MemorySink {
    /// Alerts raised so far.
    pub fn notices(&self) -> Vec<Notice> {
        self.notices.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Incidents logged so far.
    pub fn incidents(&self) -> Vec<Incident> {
        self.incidents.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

impl Notifier for MemorySink {
    fn notify(&self, notice: Notice) -> Result<(), String> {
        self.notices.lock().unwrap_or_else(|e| e.into_inner()).push(notice);
        Ok(())
    }
}

impl IncidentSink for MemorySink {
    fn record(&self, incident: Incident) -> Result<(), String> {
        self.incidents.lock().unwrap_or_else(|e| e.into_inner()).push(incident);
        Ok(())
    }
}
