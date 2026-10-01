//! State that must survive an engine restart (spec §11).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use tpt_app_av_automation_triggers::SchedulerState;

/// Everything persisted across restarts.
///
/// Device health is deliberately *not* persisted: after a restart each device must prove itself
/// again with a heartbeat, rather than being trusted on stale data.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EngineState {
    /// Next execution id to hand out, so ids stay unique across restarts.
    pub next_execution: u64,
    /// Scheduler memory, which is what prevents one-shot schedules re-firing.
    pub scheduler: SchedulerState,
    /// Armed state per rule id.
    pub armed: BTreeMap<String, bool>,
}
