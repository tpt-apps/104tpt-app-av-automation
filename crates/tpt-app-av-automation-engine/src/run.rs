//! Event dispatch and action-chain execution.

use std::collections::VecDeque;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::time::Instant;

use tpt_app_av_automation_actions::{ActionContext, CancelToken};
use tpt_app_av_automation_conditions as conditions;
use tpt_app_av_automation_core::{DeviceHealth, Event};
use tpt_app_av_automation_devices::Command;
use tpt_app_av_automation_model::{
    ActionOutcome, ActionResultRecord, ActionSpec, ActionStep, ConditionResultRecord,
    ExecutionRecord, ExecutionStatus, FailurePolicy, Rule,
};

use crate::conflict::Claims;
use crate::{Engine, Mode};

/// Per-dispatch execution context threaded through nested rule invocations.
struct RunCx<'a> {
    /// Whether an ancestor rule is being simulated; children inherit it.
    simulated: bool,
    token: CancelToken,
    depth: usize,
    /// Rule ids currently being executed, outermost first.
    stack: Vec<String>,
    claims: &'a mut Claims,
    out: &'a mut Vec<ExecutionRecord>,
    followups: &'a mut Vec<Event>,
}

impl Engine {
    /// Feeds one event to the engine and returns the executions it caused, in execution order.
    ///
    /// Events produced as a side effect (a device that failed mid-chain becomes `Degraded`) are
    /// processed too, up to [`crate::EngineConfig::max_cascade`] events, so health changes can
    /// themselves trigger rules without being able to loop forever.
    pub fn handle_event(&mut self, event: Event) -> Vec<ExecutionRecord> {
        let mut out = Vec::new();
        let mut queue = VecDeque::new();
        queue.push_back(event);
        let mut dispatched = 0usize;
        while let Some(event) = queue.pop_front() {
            if dispatched >= self.config.max_cascade {
                tracing::error!(
                    limit = self.config.max_cascade,
                    "event cascade limit reached; dropping {} pending event(s)",
                    queue.len() + 1
                );
                break;
            }
            dispatched += 1;
            let mut followups = Vec::new();
            self.dispatch(&event, &mut out, &mut followups);
            queue.extend(followups);
        }
        for record in &out {
            self.history.push_back(record.clone());
            while self.history.len() > self.config.history_limit.max(1) {
                self.history.pop_front();
            }
        }
        out
    }

    /// Updates registry state from an observed event and returns the previous DMX value (an unseen
    /// channel counts as 0, matching the inbound tracker's baseline).
    fn ingest(&mut self, event: &Event) -> Option<u8> {
        match event {
            Event::Dmx(level) => Some(self.registry.observe_dmx(*level).unwrap_or(0)),
            Event::DeviceParameter {
                device,
                parameter,
                value,
            } => {
                let _ = self.registry.report_parameter(device, parameter, *value);
                None
            }
            Event::DeviceState {
                device, current, ..
            } => {
                let _ = self.registry.set_health(device, *current);
                None
            }
            _ => None,
        }
    }

    fn rule_matches(rule: &Rule, event: &Event, previous_dmx: Option<u8>) -> bool {
        match event {
            // An operator "run now" names its rule and bypasses the trigger type.
            Event::Manual { rule: Some(id) } => rule.id.as_str() == id,
            _ => {
                let mut trigger = rule.trigger.clone();
                if trigger.needs_previous_dmx() {
                    trigger.dmx_previous = previous_dmx;
                }
                trigger.matches(event)
            }
        }
    }

    fn dispatch(&mut self, event: &Event, out: &mut Vec<ExecutionRecord>, followups: &mut Vec<Event>) {
        let previous_dmx = self.ingest(event);
        let mut matched: Vec<Rule> = self
            .pack
            .rules
            .iter()
            .filter(|r| Self::rule_matches(r, event, previous_dmx))
            .cloned()
            .collect();
        // Deterministic order: priority first (Critical sorts lowest), then rule id (spec §10.1).
        matched.sort_by(|a, b| a.priority.cmp(&b.priority).then_with(|| a.id.cmp(&b.id)));

        let mut claims = Claims::default();
        for rule in matched {
            let mut cx = RunCx {
                simulated: false,
                token: CancelToken::new(),
                depth: 0,
                stack: vec![rule.id.as_str().to_owned()],
                claims: &mut claims,
                out: &mut *out,
                followups: &mut *followups,
            };
            let record = self.execute_rule(&rule, event, &mut cx);
            out.push(record);
        }
    }

    fn allocate_execution_id(&mut self) -> String {
        let id = format!("exec-{:06}", self.next_execution);
        self.next_execution += 1;
        id
    }

    fn execute_rule(&mut self, rule: &Rule, event: &Event, cx: &mut RunCx<'_>) -> ExecutionRecord {
        let execution_id = self.allocate_execution_id();
        let started = self.clock.now();
        let simulated =
            cx.simulated || self.config.mode == Mode::Simulation || !self.is_armed(rule.id.as_str());

        if cx.depth == 0 {
            self.cancel
                .register(&execution_id, rule.id.as_str(), cx.token.clone());
        }

        let context = self.evaluation_context(rule);
        let gate = conditions::gate(&rule.conditions, &context);
        let condition_results = gate
            .results
            .iter()
            .map(|(id, eval, permits)| ConditionResultRecord {
                condition_id: id.as_str().into(),
                result: eval.result,
                permits: *permits,
                observed: Some(eval.observed.clone()),
            })
            .collect();

        let (action_results, overall_status) = if gate.permits {
            self.run_chain(rule, event, &execution_id, simulated, cx)
        } else {
            (Vec::new(), ExecutionStatus::Skipped)
        };

        if cx.depth == 0 {
            self.cancel.unregister(&execution_id);
        }

        let finished = self.clock.now();
        ExecutionRecord {
            execution_id,
            rule_id: rule.id.clone(),
            rule_name: rule.name.clone(),
            rule_version: rule.version,
            triggered_at_ms: started.as_millis(),
            trigger_detail: event.clone(),
            trigger_type: rule.trigger.type_label().to_owned(),
            condition_results,
            action_results,
            overall_status,
            simulated,
            duration_ms: finished.millis_since(started),
        }
    }

    fn run_chain(
        &mut self,
        rule: &Rule,
        event: &Event,
        execution_id: &str,
        simulated: bool,
        cx: &mut RunCx<'_>,
    ) -> (Vec<ActionResultRecord>, ExecutionStatus) {
        let policy = rule.policy.on_failure.clone();
        let budget = rule.policy.timeout_ms;
        let started = Instant::now();
        let remaining = |started: &Instant| {
            budget.map(|b| b.saturating_sub(u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)))
        };

        let mut results: Vec<ActionResultRecord> = Vec::new();
        let mut status = ExecutionStatus::Skipped;
        let mut cancelled = false;
        let mut run_fallback = false;
        let main = &rule.actions;

        for (i, step) in main.iter().enumerate() {
            if cx.token.is_cancelled() {
                skip_rest(&main[i..], "cancelled by operator", &policy, simulated, &mut results);
                cancelled = true;
                break;
            }
            let record = self.run_step(
                step,
                rule,
                event,
                execution_id,
                simulated,
                remaining(&started),
                budget,
                cx,
            );
            status = status.fold(&record.outcome);
            let failed = record.outcome.is_failure();
            let failed_id = record.action_id.to_string();
            results.push(record);
            if failed {
                match policy {
                    FailurePolicy::ContinueChain => {}
                    FailurePolicy::StopChain => {
                        let reason = format!("chain stopped after failure of `{failed_id}`");
                        skip_rest(&main[i + 1..], &reason, &policy, simulated, &mut results);
                        break;
                    }
                    FailurePolicy::RunFallback => {
                        let reason = format!("replaced by fallback after failure of `{failed_id}`");
                        skip_rest(&main[i + 1..], &reason, &policy, simulated, &mut results);
                        run_fallback = true;
                        break;
                    }
                }
            }
        }

        if run_fallback && !cancelled {
            let fallback = &rule.policy.fallback;
            for (i, step) in fallback.iter().enumerate() {
                if cx.token.is_cancelled() {
                    skip_rest(&fallback[i..], "cancelled by operator", &policy, simulated, &mut results);
                    break;
                }
                let record = self.run_step(
                    step,
                    rule,
                    event,
                    execution_id,
                    simulated,
                    remaining(&started),
                    budget,
                    cx,
                );
                status = status.fold(&record.outcome);
                results.push(record);
            }
        }

        (results, status)
    }

    #[allow(clippy::too_many_arguments)]
    fn run_step(
        &mut self,
        step: &ActionStep,
        rule: &Rule,
        event: &Event,
        execution_id: &str,
        simulated: bool,
        remaining_ms: Option<u64>,
        budget_total: Option<u64>,
        cx: &mut RunCx<'_>,
    ) -> ActionResultRecord {
        let started = self.clock.now();
        let policy = rule.policy.on_failure.clone();
        let device = step.target().map(str::to_owned);
        let make = |action_type: &str, outcome: ActionOutcome, detail: Option<String>, ms: u64| ActionResultRecord {
            action_id: step.id.clone(),
            action_type: action_type.to_owned(),
            device: device.clone(),
            outcome,
            duration_ms: ms,
            simulated,
            failure_policy: policy.clone(),
            detail,
        };

        let spec = match step.spec.resolve_library(&self.pack.action_library) {
            Ok(spec) => spec.clone(),
            Err(message) => {
                return make(
                    step.spec.type_label(),
                    ActionOutcome::Failed {
                        reason: message.to_owned(),
                    },
                    None,
                    0,
                )
            }
        };
        let label = spec.type_label();

        if let (Some(command), Some(target)) = (Command::from_spec(&spec), step.target()) {
            if let Err(reason) = cx.claims.claim(
                target,
                rule.id.as_str(),
                &rule.priority.to_string(),
                &command,
                simulated,
            ) {
                tracing::warn!(rule = %rule.id, "{reason}");
                return make(label, ActionOutcome::Skipped { reason }, None, 0);
            }
        }

        let (outcome, detail) = match &spec {
            ActionSpec::InvokeRule { rule: target } => self.invoke(target, event, simulated, cx),
            _ if simulated => (
                ActionOutcome::Success,
                Some(self.actions.simulate(step, &spec)),
            ),
            _ if remaining_ms == Some(0) => (
                ActionOutcome::TimedOut {
                    after_ms: budget_total.unwrap_or(0),
                },
                None,
            ),
            _ => {
                let ctx = ActionContext {
                    rule_id: rule.id.to_string(),
                    rule_name: rule.name.clone(),
                    execution_id: execution_id.to_owned(),
                };
                let timeout = step.effective_timeout_ms(remaining_ms);
                let attempt = catch_unwind(AssertUnwindSafe(|| {
                    self.actions.execute(step, &spec, &ctx, timeout, &cx.token)
                }));
                match attempt {
                    Ok(result) => {
                        if let Some(faulty) = &result.device_fault {
                            // Only downgrade a device we believed was healthy; one already
                            // offline or unknown has nothing further to report.
                            if self.registry.health(faulty) == Some(DeviceHealth::Online) {
                                if let Ok(Some(ev)) =
                                    self.registry.set_health(faulty, DeviceHealth::Degraded)
                                {
                                    cx.followups.push(ev);
                                }
                            }
                        }
                        (result.outcome, Some(self.actions.simulate(step, &spec)))
                    }
                    Err(_) => (
                        ActionOutcome::Failed {
                            reason: "internal error: action panicked".into(),
                        },
                        None,
                    ),
                }
            }
        };

        let finished = self.clock.now();
        make(label, outcome, detail, finished.millis_since(started))
    }

    /// Runs another rule as a step of the current chain (spec §8.4).
    fn invoke(
        &mut self,
        target: &str,
        event: &Event,
        simulated: bool,
        cx: &mut RunCx<'_>,
    ) -> (ActionOutcome, Option<String>) {
        let failed = |reason: String| (ActionOutcome::Failed { reason }, None);
        if cx.depth + 1 > self.config.max_invoke_depth {
            return failed(format!(
                "rule invocation deeper than {} levels",
                self.config.max_invoke_depth
            ));
        }
        if cx.stack.iter().any(|s| s == target) {
            return failed(format!("rule `{target}` invokes itself (cycle)"));
        }
        let Some(child) = self.pack.rule(target).cloned() else {
            return failed(format!("invoked rule `{target}` does not exist"));
        };

        let mut stack = cx.stack.clone();
        stack.push(target.to_owned());
        let record = {
            let mut child_cx = RunCx {
                simulated,
                token: cx.token.clone(),
                depth: cx.depth + 1,
                stack,
                claims: &mut *cx.claims,
                out: &mut *cx.out,
                followups: &mut *cx.followups,
            };
            self.execute_rule(&child, event, &mut child_cx)
        };

        let detail = Some(format!(
            "invoked rule `{target}` as {} ({})",
            record.execution_id, record.overall_status
        ));
        let outcome = match record.overall_status {
            ExecutionStatus::Success => ActionOutcome::Success,
            ExecutionStatus::Skipped => ActionOutcome::Skipped {
                reason: format!("invoked rule `{target}` did not run (conditions not met or no actions)"),
            },
            status => ActionOutcome::Failed {
                reason: format!("invoked rule `{target}` finished with {status}"),
            },
        };
        cx.out.push(record);
        (outcome, detail)
    }
}

fn skip_rest(
    steps: &[ActionStep],
    reason: &str,
    policy: &FailurePolicy,
    simulated: bool,
    results: &mut Vec<ActionResultRecord>,
) {
    for step in steps {
        results.push(ActionResultRecord {
            action_id: step.id.clone(),
            action_type: step.spec.type_label().to_owned(),
            device: step.target().map(str::to_owned),
            outcome: ActionOutcome::Skipped {
                reason: reason.to_owned(),
            },
            duration_ms: 0,
            simulated,
            failure_policy: policy.clone(),
            detail: None,
        });
    }
}
