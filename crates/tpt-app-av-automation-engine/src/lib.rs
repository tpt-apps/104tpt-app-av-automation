//! The deterministic rule evaluation engine (spec §3.2, §10).
//!
//! ```text
//! event ──▶ ingest ──▶ match (priority order) ──▶ conditions ──▶ conflict check ──▶ action chain
//!                                                     │                                 │
//!                                                     └── not met / unknown: Skipped    ├─ live: Actions::execute
//!                                                                                       └─ disarmed / simulation: Actions::simulate
//! ```
//!
//! Given the same pack, registry state, clock and event, [`Engine::handle_event`] produces the same
//! decision and the same ordered [`ExecutionRecord`]s. Everything non-deterministic (time, device
//! I/O, sleeping) is injected: the [`Clock`], the [`Actions`] dispatcher and its endpoints.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod conflict;
mod run;
mod state;

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex};

use tpt_app_av_automation_actions::{Actions, CancelToken};
use tpt_app_av_automation_conditions::EvaluationContext;
use tpt_app_av_automation_core::{Clock, DeviceHealth, Diagnostic, Error, Event, Result, Timestamp};
use tpt_app_av_automation_devices::DeviceRegistry;
use tpt_app_av_automation_model::{ExecutionRecord, Rule, RulePack};
use tpt_app_av_automation_triggers::{LocalClock, Scheduler};

pub use state::EngineState;

/// Whether the engine may touch the outside world.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// Armed rules send real actions; disarmed rules are simulated.
    #[default]
    Live,
    /// Nothing is ever sent, regardless of rule arming (spec §3.5).
    Simulation,
}

/// Engine tunables.
#[derive(Debug, Clone, Copy)]
pub struct EngineConfig {
    /// Live or simulation.
    pub mode: Mode,
    /// Site UTC offset used for schedules and time-window conditions.
    pub local_clock: LocalClock,
    /// Executions kept in memory.
    pub history_limit: usize,
    /// Most events one external event may cascade into (device degradations, ...).
    pub max_cascade: usize,
    /// Deepest `workflow.invoke_rule` nesting.
    pub max_invoke_depth: usize,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            mode: Mode::Live,
            local_clock: LocalClock::default(),
            history_limit: 10_000,
            max_cascade: 64,
            max_invoke_depth: 8,
        }
    }
}

/// Shared table of running chains, so another thread can cancel them (spec §10.2).
#[derive(Debug, Clone, Default)]
pub struct CancelHandle {
    chains: Arc<Mutex<BTreeMap<String, (String, CancelToken)>>>,
}

impl CancelHandle {
    fn register(&self, execution_id: &str, rule_id: &str, token: CancelToken) {
        self.lock()
            .insert(execution_id.to_owned(), (rule_id.to_owned(), token));
    }

    fn unregister(&self, execution_id: &str) {
        self.lock().remove(execution_id);
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, (String, CancelToken)>> {
        self.chains.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// `(execution id, rule id)` of every chain currently running.
    pub fn active(&self) -> Vec<(String, String)> {
        self.lock()
            .iter()
            .map(|(e, (r, _))| (e.clone(), r.clone()))
            .collect()
    }

    /// Cancels one running chain. Returns whether it existed.
    pub fn cancel(&self, execution_id: &str) -> bool {
        match self.lock().get(execution_id) {
            Some((_, token)) => {
                token.cancel();
                true
            }
            None => false,
        }
    }

    /// Cancels every running chain of a rule; returns how many were signalled.
    pub fn cancel_rule(&self, rule_id: &str) -> usize {
        let chains = self.lock();
        let mut n = 0;
        for (rule, token) in chains.values() {
            if rule == rule_id {
                token.cancel();
                n += 1;
            }
        }
        n
    }

    /// Cancels everything that is running.
    pub fn cancel_all(&self) -> usize {
        let chains = self.lock();
        for (_, token) in chains.values() {
            token.cancel();
        }
        chains.len()
    }
}

/// The rule engine.
pub struct Engine {
    pack: RulePack,
    armed: BTreeMap<String, bool>,
    registry: DeviceRegistry,
    actions: Actions,
    clock: Arc<dyn Clock>,
    scheduler: Scheduler,
    config: EngineConfig,
    history: VecDeque<ExecutionRecord>,
    next_execution: u64,
    cancel: CancelHandle,
    state_hook: Option<Box<dyn FnMut(&EngineState) + Send>>,
}

impl Engine {
    /// Builds an engine. The pack is validated; a pack with errors is refused.
    pub fn new(
        pack: RulePack,
        registry: DeviceRegistry,
        actions: Actions,
        clock: Arc<dyn Clock>,
        config: EngineConfig,
    ) -> Result<Self> {
        Self::ensure_valid(&pack)?;
        let armed = pack
            .rules
            .iter()
            .map(|r| (r.id.as_str().to_owned(), r.armed))
            .collect();
        let scheduler = Scheduler::from_rules(
            pack.rules.iter(),
            pack.site,
            config.local_clock.utc_offset_minutes,
        );
        Ok(Self {
            pack,
            armed,
            registry,
            actions,
            clock,
            scheduler,
            config,
            history: VecDeque::new(),
            next_execution: 1,
            cancel: CancelHandle::default(),
            state_hook: None,
        })
    }

    fn ensure_valid(pack: &RulePack) -> Result<()> {
        let diagnostics = pack.validate()?;
        if tpt_app_av_automation_model::has_errors(&diagnostics) {
            return Err(Error::Validation(diagnostics));
        }
        Ok(())
    }

    /// The loaded pack.
    pub fn pack(&self) -> &RulePack {
        &self.pack
    }

    /// Replaces the loaded pack (hot reload).
    ///
    /// Rules whose id and version are unchanged keep their armed state; new or edited rules come
    /// up disarmed (spec §3.5). Scheduler memory is kept, so a reload cannot re-fire a one-shot.
    pub fn load_pack(&mut self, pack: RulePack) -> Result<()> {
        Self::ensure_valid(&pack)?;
        let mut armed = BTreeMap::new();
        for rule in &pack.rules {
            let unchanged = self
                .pack
                .rule(rule.id.as_str())
                .is_some_and(|old| old.version == rule.version);
            let was_armed = unchanged
                && self
                    .armed
                    .get(rule.id.as_str())
                    .copied()
                    .unwrap_or(false);
            armed.insert(rule.id.as_str().to_owned(), was_armed);
        }
        let memory = self.scheduler.state().clone();
        self.scheduler = Scheduler::from_rules(
            pack.rules.iter(),
            pack.site,
            self.config.local_clock.utc_offset_minutes,
        );
        self.scheduler.restore(memory);
        self.armed = armed;
        self.pack = pack;
        Ok(())
    }

    /// The device registry.
    pub fn registry(&self) -> &DeviceRegistry {
        &self.registry
    }

    /// Mutable access to the registry.
    pub fn registry_mut(&mut self) -> &mut DeviceRegistry {
        &mut self.registry
    }

    /// Mutable access to the action dispatcher (binding endpoints, exec policy).
    pub fn actions_mut(&mut self) -> &mut Actions {
        &mut self.actions
    }

    /// Current mode.
    pub fn mode(&self) -> Mode {
        self.config.mode
    }

    /// Switches between live and simulation.
    pub fn set_mode(&mut self, mode: Mode) {
        self.config.mode = mode;
    }

    /// Whether a rule is armed. Unknown rules are not.
    pub fn is_armed(&self, rule_id: &str) -> bool {
        self.armed.get(rule_id).copied().unwrap_or(false)
    }

    /// Arms a rule.
    pub fn arm(&mut self, rule_id: &str) -> Result<()> {
        self.set_armed(rule_id, true)
    }

    /// Disarms a rule.
    pub fn disarm(&mut self, rule_id: &str) -> Result<()> {
        self.set_armed(rule_id, false)
    }

    fn set_armed(&mut self, rule_id: &str, armed: bool) -> Result<()> {
        match self.armed.get_mut(rule_id) {
            Some(slot) => {
                *slot = armed;
                Ok(())
            }
            None => Err(Error::NotFound {
                kind: "rule",
                id: rule_id.to_owned(),
            }),
        }
    }

    /// A handle for cancelling running chains from another thread.
    pub fn cancel_handle(&self) -> CancelHandle {
        self.cancel.clone()
    }

    /// Recent executions, oldest first.
    pub fn history(&self) -> impl Iterator<Item = &ExecutionRecord> {
        self.history.iter()
    }

    /// Current time from the injected clock.
    pub fn now(&self) -> Timestamp {
        self.clock.now()
    }

    /// Checks that every device a rule writes to exists in the registry.
    ///
    /// Kept separate from pack validation because packs are portable while device registries are
    /// per-site (spec §9).
    pub fn check_devices(&self) -> Vec<Diagnostic> {
        let mut out = Vec::new();
        for (ri, rule) in self.pack.rules.iter().enumerate() {
            let main = rule
                .actions
                .iter()
                .enumerate()
                .map(|(i, s)| (format!("rules[{ri}].actions[{i}].device"), s));
            let fallback = rule
                .policy
                .fallback
                .iter()
                .enumerate()
                .map(|(i, s)| (format!("rules[{ri}].policy.fallback[{i}].device"), s));
            for (location, step) in main.chain(fallback) {
                if let Some(device) = step.target() {
                    if !self.registry.contains(device) {
                        out.push(Diagnostic::error(
                            location,
                            format!("device `{device}` is not in the device registry"),
                        ));
                    }
                }
            }
        }
        out
    }

    pub(crate) fn evaluation_context(&self, rule: &Rule) -> EvaluationContext {
        let now = self.clock.now();
        EvaluationContext {
            local_minutes: Some(self.config.local_clock.local(now).minutes_since_midnight),
            device_health: self
                .registry
                .devices()
                .map(|d| (d.id.as_str().to_owned(), d.health))
                .collect::<BTreeMap<String, DeviceHealth>>(),
            dmx: self.registry.dmx_snapshot(),
            parameters: self.registry.parameter_snapshot(),
            armed_rules: self
                .armed
                .iter()
                .filter(|(_, armed)| **armed)
                .map(|(id, _)| id.clone())
                .collect(),
            known_rules: self.pack.rules.iter().map(|r| r.id.as_str().to_owned()).collect(),
            rule_tags: rule.tags.clone(),
        }
    }

    /// Registers a callback that receives the restart state *before* time-driven rules execute.
    ///
    /// The scheduler marks a firing as done the moment it decides to fire. Persisting that decision
    /// first makes one-shot schedules at-most-once even if the process is killed while the chain is
    /// running: after the restart the one-shot is already recorded as fired and is not repeated.
    pub fn set_state_hook(&mut self, hook: impl FnMut(&EngineState) + Send + 'static) {
        self.state_hook = Some(Box::new(hook));
    }

    /// Advances time: fires due schedules and checks device heartbeats.
    pub fn tick(&mut self) -> Vec<ExecutionRecord> {
        let now = self.clock.now();
        let mut events = self.scheduler.poll(now, self.config.local_clock);
        if !events.is_empty() {
            if let Some(mut hook) = self.state_hook.take() {
                hook(&self.snapshot());
                self.state_hook = Some(hook);
            }
        }
        events.extend(self.registry.check_heartbeats(now));
        let mut records = Vec::new();
        for event in events {
            records.extend(self.handle_event(event));
        }
        records
    }

    /// Records a device heartbeat and dispatches any resulting health transition.
    pub fn device_heartbeat(&mut self, device: &str) -> Result<Vec<ExecutionRecord>> {
        let now = self.clock.now();
        let events = self.registry.heartbeat(device, now)?;
        let mut records = Vec::new();
        for event in events {
            records.extend(self.handle_event(event));
        }
        Ok(records)
    }

    /// Runs a rule now, regardless of its trigger (operator "run now" / API call, spec §7.6).
    pub fn run_rule(&mut self, rule_id: &str) -> Result<Vec<ExecutionRecord>> {
        if self.pack.rule(rule_id).is_none() {
            return Err(Error::NotFound {
                kind: "rule",
                id: rule_id.to_owned(),
            });
        }
        Ok(self.handle_event(Event::Manual {
            rule: Some(rule_id.to_owned()),
        }))
    }

    /// Snapshot of the state that must survive a restart (spec §11).
    pub fn snapshot(&self) -> EngineState {
        EngineState {
            next_execution: self.next_execution,
            scheduler: self.scheduler.state().clone(),
            armed: self.armed.clone(),
        }
    }

    /// Restores state after a restart. Unknown rule ids in the snapshot are ignored.
    pub fn restore(&mut self, state: EngineState) {
        self.next_execution = self.next_execution.max(state.next_execution);
        self.scheduler.restore(state.scheduler);
        for (id, armed) in state.armed {
            if let Some(slot) = self.armed.get_mut(&id) {
                *slot = armed;
            }
        }
    }
}

impl std::fmt::Debug for Engine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Engine")
            .field("pack", &self.pack.name)
            .field("mode", &self.config.mode)
            .field("rules", &self.pack.rules.len())
            .finish()
    }
}
