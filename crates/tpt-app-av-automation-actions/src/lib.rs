//! Action execution and simulation (spec §6.4, §8).
//!
//! [`Actions`] is the single place where a rule's action step touches the outside world. It offers
//! two entry points with deliberately identical signatures:
//!
//! * [`Actions::execute`] performs the step — sends the control message, raises the alert, runs
//!   the sandboxed program.
//! * [`Actions::simulate`] describes what `execute` would do and performs **nothing**. It has no
//!   access to endpoints at all, which is what makes "simulation never sends a real control
//!   message" (spec §3.5, §24) a structural guarantee rather than a convention.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod exec;
mod sinks;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::Duration;

use tpt_app_av_automation_devices::{Command, Endpoint};
use tpt_app_av_automation_model::{ActionOutcome, ActionSpec, ActionStep, AlertSeverity};

pub use exec::ExecPolicy;
pub use sinks::{
    Incident, IncidentSink, MemorySink, Notice, Notifier, NullSleeper, Sleeper, ThreadSleeper,
    TracingSink,
};

/// Cooperative cancellation flag shared between the operator and a running chain (spec §10.2).
#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    /// A fresh, un-cancelled token.
    pub fn new() -> Self {
        Self::default()
    }

    /// Requests cancellation. Idempotent.
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    /// Whether cancellation was requested.
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// Facts about the execution a step belongs to, for logs and incidents.
#[derive(Debug, Clone)]
pub struct ActionContext {
    /// Rule id.
    pub rule_id: String,
    /// Rule display name.
    pub rule_name: String,
    /// Execution id.
    pub execution_id: String,
}

/// What executing one step produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepResult {
    /// The outcome to record.
    pub outcome: ActionOutcome,
    /// Set when the failure means the target device itself is unhealthy, so the caller can feed it
    /// back into health tracking.
    pub device_fault: Option<String>,
}

impl StepResult {
    fn ok() -> Self {
        Self {
            outcome: ActionOutcome::Success,
            device_fault: None,
        }
    }

    fn failed(reason: impl Into<String>) -> Self {
        Self {
            outcome: ActionOutcome::Failed {
                reason: reason.into(),
            },
            device_fault: None,
        }
    }
}

/// Executes and simulates action steps.
pub struct Actions {
    endpoints: HashMap<String, Arc<dyn Endpoint>>,
    notifier: Arc<dyn Notifier>,
    incidents: Arc<dyn IncidentSink>,
    sleeper: Arc<dyn Sleeper>,
    exec_policy: ExecPolicy,
}

impl Actions {
    /// Builds a dispatcher with the given sinks and no endpoints.
    pub fn new(
        notifier: Arc<dyn Notifier>,
        incidents: Arc<dyn IncidentSink>,
        sleeper: Arc<dyn Sleeper>,
    ) -> Self {
        Self {
            endpoints: HashMap::new(),
            notifier,
            incidents,
            sleeper,
            exec_policy: ExecPolicy::default(),
        }
    }

    /// A dispatcher that logs through `tracing`, sleeps on the real clock and runs no programs.
    pub fn with_defaults() -> Self {
        let sink = Arc::new(TracingSink);
        Self::new(sink.clone(), sink, Arc::new(ThreadSleeper))
    }

    /// Binds the endpoint for a device id.
    pub fn bind_endpoint(&mut self, device: impl Into<String>, endpoint: Arc<dyn Endpoint>) {
        self.endpoints.insert(device.into(), endpoint);
    }

    /// Removes the endpoint bound to a device; returns whether one was bound.
    pub fn unbind_endpoint(&mut self, device: &str) -> bool {
        self.endpoints.remove(device).is_some()
    }

    /// The endpoint bound to a device, if any.
    pub fn endpoint(&self, device: &str) -> Option<&Arc<dyn Endpoint>> {
        self.endpoints.get(device)
    }

    /// Replaces the external-program policy.
    pub fn set_exec_policy(&mut self, policy: ExecPolicy) {
        self.exec_policy = policy;
    }

    /// Describes what a step would do, without side effects.
    pub fn simulate(&self, step: &ActionStep, spec: &ActionSpec) -> String {
        match Command::from_spec(spec) {
            Some(command) => {
                let target = step.target().unwrap_or("<no device>");
                format!("{} -> {target}", command.describe())
            }
            None => match spec {
                ActionSpec::NotifyOperator { message, severity } => {
                    format!("notify.operator [{}] \"{message}\"", severity.as_str())
                }
                ActionSpec::LogIncident { severity, message } => format!(
                    "log.incident severity={}{}",
                    severity.as_str(),
                    message
                        .as_deref()
                        .map(|m| format!(" \"{m}\""))
                        .unwrap_or_default()
                ),
                ActionSpec::Wait { ms } => format!("workflow.wait {ms}ms"),
                ActionSpec::InvokeRule { rule } => format!("workflow.invoke_rule `{rule}`"),
                ActionSpec::Exec { program, args, .. } => {
                    format!("workflow.exec {program} {}", args.join(" "))
                        .trim_end()
                        .to_string()
                }
                other => other.type_label().to_string(),
            },
        }
    }

    /// Performs a step for real.
    ///
    /// `timeout_ms` bounds endpoint delivery and program runs; `None` means unbounded. Waits are
    /// interruptible through `cancel`.
    pub fn execute(
        &self,
        step: &ActionStep,
        spec: &ActionSpec,
        ctx: &ActionContext,
        timeout_ms: Option<u64>,
        cancel: &CancelToken,
    ) -> StepResult {
        if let Some(command) = Command::from_spec(spec) {
            return self.send(step, command, timeout_ms);
        }
        match spec {
            ActionSpec::NotifyOperator { message, severity } => {
                match self.notifier.notify(Notice {
                    severity: *severity,
                    message: message.clone(),
                    rule_id: ctx.rule_id.clone(),
                }) {
                    Ok(()) => StepResult::ok(),
                    Err(e) => StepResult::failed(format!("notification failed: {e}")),
                }
            }
            ActionSpec::LogIncident { severity, message } => {
                let message = message
                    .clone()
                    .unwrap_or_else(|| format!("rule `{}` executed", ctx.rule_name));
                match self.incidents.record(Incident {
                    severity: *severity,
                    message,
                    rule_id: ctx.rule_id.clone(),
                    execution_id: ctx.execution_id.clone(),
                }) {
                    Ok(()) => StepResult::ok(),
                    Err(e) => StepResult::failed(format!("incident log failed: {e}")),
                }
            }
            ActionSpec::Wait { ms } => {
                if let Some(limit) = timeout_ms {
                    if *ms > limit {
                        self.sleeper.sleep(limit, cancel);
                        return StepResult {
                            outcome: ActionOutcome::TimedOut { after_ms: limit },
                            device_fault: None,
                        };
                    }
                }
                self.sleeper.sleep(*ms, cancel);
                StepResult::ok()
            }
            ActionSpec::Exec {
                program,
                args,
                timeout_ms: exec_timeout,
            } => {
                let bound = timeout_ms.map_or(*exec_timeout, |t| t.min(*exec_timeout));
                StepResult {
                    outcome: exec::run(&self.exec_policy, program, args, bound),
                    device_fault: None,
                }
            }
            ActionSpec::InvokeRule { .. } | ActionSpec::UseAction { .. } => StepResult::failed(
                format!("`{}` must be resolved by the engine", spec.type_label()),
            ),
            _ => StepResult::failed(format!("unsupported action `{}`", spec.type_label())),
        }
    }

    fn send(&self, step: &ActionStep, command: Command, timeout_ms: Option<u64>) -> StepResult {
        let Some(device) = step.target() else {
            return StepResult::failed("action has no device target");
        };
        let Some(endpoint) = self.endpoints.get(device).cloned() else {
            return StepResult::failed(format!("no endpoint bound for device `{device}`"));
        };
        let result = match timeout_ms {
            None => endpoint.send(&command),
            Some(limit) => {
                let (tx, rx) = mpsc::channel();
                let worker = endpoint.clone();
                std::thread::spawn(move || {
                    let _ = tx.send(worker.send(&command));
                });
                match rx.recv_timeout(Duration::from_millis(limit)) {
                    Ok(result) => result,
                    Err(_) => {
                        return StepResult {
                            outcome: ActionOutcome::TimedOut { after_ms: limit },
                            device_fault: Some(device.to_owned()),
                        }
                    }
                }
            }
        };
        match result {
            Ok(()) => StepResult::ok(),
            Err(e) => StepResult {
                device_fault: e.is_device_fault().then(|| device.to_owned()),
                outcome: match e {
                    tpt_app_av_automation_devices::EndpointError::Timeout => {
                        ActionOutcome::TimedOut {
                            after_ms: timeout_ms.unwrap_or(0),
                        }
                    }
                    other => ActionOutcome::Failed {
                        reason: other.to_string(),
                    },
                },
            },
        }
    }
}

impl std::fmt::Debug for Actions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Actions")
            .field("devices", &self.endpoints.keys().collect::<Vec<_>>())
            .finish()
    }
}

/// Convenience for tests and shells: severity ordering helper.
pub fn at_least(severity: AlertSeverity, floor: AlertSeverity) -> bool {
    severity >= floor
}

#[cfg(test)]
mod tests {
    use super::*;
    use tpt_app_av_automation_devices::{Faults, VirtualEndpoint};

    fn ctx() -> ActionContext {
        ActionContext {
            rule_id: "r".into(),
            rule_name: "Rule".into(),
            execution_id: "e1".into(),
        }
    }

    fn setup() -> (Actions, VirtualEndpoint, Arc<MemorySink>) {
        let sink = Arc::new(MemorySink::default());
        let mut actions = Actions::new(sink.clone(), sink.clone(), Arc::new(NullSleeper));
        let ep = VirtualEndpoint::new();
        actions.bind_endpoint("proj", Arc::new(ep.clone()));
        (actions, ep, sink)
    }

    fn osc_step() -> ActionStep {
        ActionStep::new(
            "a1",
            ActionSpec::Osc {
                address: "/power".into(),
                args: vec![1.0],
            },
        )
        .targeting("proj")
    }

    #[test]
    fn executes_endpoint_writes() {
        let (actions, ep, _) = setup();
        let step = osc_step();
        let r = actions.execute(&step, &step.spec, &ctx(), None, &CancelToken::new());
        assert_eq!(r.outcome, ActionOutcome::Success);
        assert_eq!(ep.sent().len(), 1);
    }

    #[test]
    fn a_media_action_reaches_a_media_device_as_osc() {
        use std::net::UdpSocket;
        use std::time::Duration;
        use tpt_app_av_automation_devices::MediaEndpoint;
        use tpt_app_av_automation_model::MediaOperation;

        let rx = UdpSocket::bind("127.0.0.1:0").unwrap();
        rx.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let (mut actions, _, _) = setup();
        let ep = MediaEndpoint::new(&rx.local_addr().unwrap().to_string(), None).unwrap();
        actions.bind_endpoint("media", Arc::new(ep));
        let step = ActionStep::new(
            "m1",
            ActionSpec::MediaVideoSource {
                source: "cam2".into(),
                operation: MediaOperation::Start,
            },
        )
        .targeting("media");
        let r = actions.execute(&step, &step.spec, &ctx(), None, &CancelToken::new());
        assert_eq!(r.outcome, ActionOutcome::Success);
        let mut buf = [0u8; 256];
        let n = rx.recv(&mut buf).unwrap();
        assert!(buf[..n].starts_with(b"/media/video/cam2/start "));
    }

    #[test]
    fn simulate_has_no_side_effects_and_describes_the_step() {
        let (actions, ep, sink) = setup();
        let step = osc_step();
        let text = actions.simulate(&step, &step.spec);
        assert_eq!(text, "osc /power = 1 -> proj");
        assert_eq!(ep.attempts(), 0);
        assert!(sink.notices().is_empty());
    }

    #[test]
    fn missing_endpoint_or_target_fails_cleanly() {
        let (actions, _, _) = setup();
        let orphan = ActionStep::new(
            "a",
            ActionSpec::Osc {
                address: "/x".into(),
                args: vec![],
            },
        );
        let r = actions.execute(&orphan, &orphan.spec, &ctx(), None, &CancelToken::new());
        assert!(matches!(r.outcome, ActionOutcome::Failed { .. }));
        let unbound = orphan.clone().targeting("ghost");
        let r = actions.execute(&unbound, &unbound.spec, &ctx(), None, &CancelToken::new());
        assert!(matches!(r.outcome, ActionOutcome::Failed { .. }));
    }

    #[test]
    fn offline_device_fails_and_reports_a_device_fault() {
        let (actions, ep, _) = setup();
        ep.set_offline(true);
        let step = osc_step();
        let r = actions.execute(&step, &step.spec, &ctx(), None, &CancelToken::new());
        assert!(matches!(r.outcome, ActionOutcome::Failed { .. }));
        assert_eq!(r.device_fault.as_deref(), Some("proj"));
    }

    #[test]
    fn slow_device_times_out() {
        let (actions, ep, _) = setup();
        ep.set_faults(Faults {
            delay_ms: 400,
            ..Faults::default()
        });
        let step = osc_step();
        let r = actions.execute(&step, &step.spec, &ctx(), Some(30), &CancelToken::new());
        assert_eq!(r.outcome, ActionOutcome::TimedOut { after_ms: 30 });
        assert_eq!(r.device_fault.as_deref(), Some("proj"));
    }

    #[test]
    fn notify_and_log_reach_their_sinks() {
        let (actions, _, sink) = setup();
        let notify = ActionStep::new(
            "n",
            ActionSpec::NotifyOperator {
                message: "lamp".into(),
                severity: AlertSeverity::High,
            },
        );
        let log = ActionStep::new(
            "l",
            ActionSpec::LogIncident {
                severity: AlertSeverity::Critical,
                message: None,
            },
        );
        assert!(actions
            .execute(&notify, &notify.spec, &ctx(), None, &CancelToken::new())
            .outcome
            .is_success());
        assert!(actions
            .execute(&log, &log.spec, &ctx(), None, &CancelToken::new())
            .outcome
            .is_success());
        assert_eq!(sink.notices()[0].message, "lamp");
        assert_eq!(sink.incidents()[0].message, "rule `Rule` executed");
        assert_eq!(sink.incidents()[0].severity, AlertSeverity::Critical);
    }

    #[test]
    fn wait_beyond_its_bound_times_out() {
        let (actions, _, _) = setup();
        let wait = ActionStep::new("w", ActionSpec::Wait { ms: 5_000 });
        let r = actions.execute(&wait, &wait.spec, &ctx(), Some(100), &CancelToken::new());
        assert_eq!(r.outcome, ActionOutcome::TimedOut { after_ms: 100 });
        let ok = ActionStep::new("w", ActionSpec::Wait { ms: 10 });
        assert!(actions
            .execute(&ok, &ok.spec, &ctx(), Some(100), &CancelToken::new())
            .outcome
            .is_success());
    }

    #[test]
    fn workflow_indirection_must_be_resolved_by_the_engine() {
        let (actions, _, _) = setup();
        let invoke = ActionStep::new("i", ActionSpec::InvokeRule { rule: "x".into() });
        let r = actions.execute(&invoke, &invoke.spec, &ctx(), None, &CancelToken::new());
        assert!(matches!(r.outcome, ActionOutcome::Failed { .. }));
    }

    #[test]
    fn cancel_token_is_shared_and_idempotent() {
        let t = CancelToken::new();
        let t2 = t.clone();
        assert!(!t2.is_cancelled());
        t.cancel();
        t.cancel();
        assert!(t2.is_cancelled());
    }
}
