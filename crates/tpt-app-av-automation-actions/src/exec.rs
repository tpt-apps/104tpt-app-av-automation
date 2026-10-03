//! Sandboxed, timeout-bounded external program execution (spec §8.4, §16).
//!
//! The sandbox is deliberately conservative and does not rely on OS-specific isolation:
//!
//! * disabled unless the operator enables it, and then only for an explicit allow-list of programs;
//! * no shell — the program and its arguments are passed as an argv vector, so metacharacters in
//!   arguments are inert;
//! * the environment is cleared, stdin is closed and stdout/stderr are discarded;
//! * the process is killed when its wall-clock bound expires;
//! * the engine never elevates privileges, so the child runs as the service account.

use std::collections::BTreeSet;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use tpt_app_av_automation_model::ActionOutcome;

/// Which external programs an operator has permitted.
#[derive(Debug, Clone, Default)]
pub struct ExecPolicy {
    /// Master switch. Off by default.
    pub enabled: bool,
    /// Exact program strings that may run (as written in the rule).
    pub allow: BTreeSet<String>,
}

impl ExecPolicy {
    /// A policy that permits exactly the given programs.
    pub fn allowing<I, S>(programs: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            enabled: true,
            allow: programs.into_iter().map(Into::into).collect(),
        }
    }
}

/// Runs `program` under `policy`, bounded by `timeout_ms`.
pub(crate) fn run(
    policy: &ExecPolicy,
    program: &str,
    args: &[String],
    timeout_ms: u64,
) -> ActionOutcome {
    if !policy.enabled {
        return ActionOutcome::Failed {
            reason: "external program execution is disabled".into(),
        };
    }
    if !policy.allow.contains(program) {
        return ActionOutcome::Failed {
            reason: format!("program `{program}` is not on the allow-list"),
        };
    }
    let mut child = match Command::new(program)
        .args(args)
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(e) => {
            return ActionOutcome::Failed {
                reason: format!("could not start `{program}`: {e}"),
            }
        }
    };
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return ActionOutcome::Success,
            Ok(Some(status)) => {
                return ActionOutcome::Failed {
                    reason: match status.code() {
                        Some(code) => format!("`{program}` exited with status {code}"),
                        None => format!("`{program}` was terminated by a signal"),
                    },
                }
            }
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return ActionOutcome::TimedOut {
                        after_ms: timeout_ms,
                    };
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(e) => {
                let _ = child.kill();
                return ActionOutcome::Failed {
                    reason: format!("waiting for `{program}` failed: {e}"),
                };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    const SHELL: &str = "cmd";
    #[cfg(not(windows))]
    const SHELL: &str = "sh";

    fn shell_args(script: &str) -> Vec<String> {
        #[cfg(windows)]
        return vec!["/C".into(), script.into()];
        #[cfg(not(windows))]
        return vec!["-c".into(), script.into()];
    }

    #[test]
    fn disabled_by_default() {
        let out = run(&ExecPolicy::default(), SHELL, &shell_args("exit 0"), 1000);
        assert!(matches!(out, ActionOutcome::Failed { .. }));
    }

    #[test]
    fn programs_outside_the_allow_list_are_refused() {
        let policy = ExecPolicy::allowing(["something-else"]);
        let out = run(&policy, SHELL, &shell_args("exit 0"), 1000);
        assert!(matches!(out, ActionOutcome::Failed { reason } if reason.contains("allow-list")));
    }

    #[test]
    fn allowed_program_success_and_failure_exit_codes() {
        let policy = ExecPolicy::allowing([SHELL]);
        assert_eq!(
            run(&policy, SHELL, &shell_args("exit 0"), 5000),
            ActionOutcome::Success
        );
        assert!(matches!(
            run(&policy, SHELL, &shell_args("exit 3"), 5000),
            ActionOutcome::Failed { reason } if reason.contains("status 3")
        ));
    }

    #[test]
    fn missing_program_fails_to_start() {
        let policy = ExecPolicy::allowing(["definitely-not-a-real-program-xyz"]);
        assert!(matches!(
            run(&policy, "definitely-not-a-real-program-xyz", &[], 1000),
            ActionOutcome::Failed { .. }
        ));
    }

    #[test]
    fn runaway_programs_are_killed_at_the_bound() {
        let policy = ExecPolicy::allowing([SHELL]);
        #[cfg(windows)]
        let script = r"C:\Windows\System32\ping.exe -n 30 127.0.0.1 >nul";
        #[cfg(not(windows))]
        let script = "sleep 30";
        let started = Instant::now();
        let out = run(&policy, SHELL, &shell_args(script), 200);
        assert_eq!(out, ActionOutcome::TimedOut { after_ms: 200 });
        assert!(started.elapsed() < Duration::from_secs(10));
    }
}
