//! A lightweight process supervisor (spec §11).
//!
//! The watchdog owns no automation logic. It starts the engine process, restarts it when it exits
//! unexpectedly (crash, kill, out-of-memory), backs off between restarts, and gives up — loudly —
//! if the engine is crash-looping, so a broken configuration cannot spin forever.
//!
//! An exit status of `0` means the engine stopped on purpose (operator shutdown) and the watchdog
//! exits too.

use std::io;
use std::process::Child;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// Restart behaviour.
#[derive(Debug, Clone)]
pub struct WatchdogPolicy {
    /// Restarts allowed within [`WatchdogPolicy::window`] before the watchdog gives up.
    pub max_restarts: u32,
    /// Sliding window over which restarts are counted.
    pub window: Duration,
    /// First delay before a restart.
    pub backoff_initial: Duration,
    /// Longest delay before a restart.
    pub backoff_max: Duration,
    /// A child that stays up this long is considered healthy, resetting the backoff.
    pub healthy_after: Duration,
    /// How often the child is polled.
    pub poll: Duration,
}

impl Default for WatchdogPolicy {
    fn default() -> Self {
        Self {
            max_restarts: 10,
            window: Duration::from_secs(60),
            backoff_initial: Duration::from_millis(250),
            backoff_max: Duration::from_secs(5),
            healthy_after: Duration::from_secs(10),
            poll: Duration::from_millis(100),
        }
    }
}

/// Something worth logging.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WatchdogEvent {
    /// A child was started.
    Started {
        /// Process id.
        pid: u32,
        /// 1 for the first start, 2 for the first restart, ...
        attempt: u32,
    },
    /// The child exited.
    Exited {
        /// Exit code, or `None` when it was killed by a signal.
        code: Option<i32>,
    },
    /// The child could not be started.
    SpawnFailed(String),
    /// A restart is scheduled.
    Restarting {
        /// Delay before the restart.
        after: Duration,
    },
}

/// How supervision ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The child exited with status 0.
    CleanExit,
    /// The watchdog was asked to stop and terminated the child.
    Stopped,
    /// The child kept failing; the watchdog stopped restarting it.
    GaveUp {
        /// Restarts attempted inside the window.
        restarts: u32,
        /// Last exit code seen.
        last_code: Option<i32>,
    },
}

/// Supervises a child process. `spawn` is called with the attempt number (1-based).
pub fn supervise<S, E>(
    mut spawn: S,
    policy: &WatchdogPolicy,
    stop: &AtomicBool,
    mut on_event: E,
) -> Outcome
where
    S: FnMut(u32) -> io::Result<Child>,
    E: FnMut(WatchdogEvent),
{
    let mut attempt = 0u32;
    let mut restarts: Vec<Instant> = Vec::new();
    let mut backoff = policy.backoff_initial;
    let mut last_code: Option<i32> = None;

    loop {
        if stop.load(Ordering::SeqCst) {
            return Outcome::Stopped;
        }
        attempt += 1;
        let started = Instant::now();
        match spawn(attempt) {
            Ok(mut child) => {
                on_event(WatchdogEvent::Started {
                    pid: child.id(),
                    attempt,
                });
                loop {
                    if stop.load(Ordering::SeqCst) {
                        let _ = child.kill();
                        let _ = child.wait();
                        return Outcome::Stopped;
                    }
                    match child.try_wait() {
                        Ok(Some(status)) => {
                            last_code = status.code();
                            on_event(WatchdogEvent::Exited { code: last_code });
                            if status.success() {
                                return Outcome::CleanExit;
                            }
                            break;
                        }
                        Ok(None) => std::thread::sleep(policy.poll),
                        Err(e) => {
                            on_event(WatchdogEvent::SpawnFailed(format!("wait failed: {e}")));
                            let _ = child.kill();
                            break;
                        }
                    }
                }
                if started.elapsed() >= policy.healthy_after {
                    backoff = policy.backoff_initial;
                    restarts.clear();
                }
            }
            Err(e) => on_event(WatchdogEvent::SpawnFailed(e.to_string())),
        }

        let now = Instant::now();
        restarts.retain(|t| now.duration_since(*t) < policy.window);
        restarts.push(now);
        if restarts.len() as u32 > policy.max_restarts {
            return Outcome::GaveUp {
                restarts: restarts.len() as u32 - 1,
                last_code,
            };
        }

        on_event(WatchdogEvent::Restarting { after: backoff });
        let deadline = Instant::now() + backoff;
        while Instant::now() < deadline {
            if stop.load(Ordering::SeqCst) {
                return Outcome::Stopped;
            }
            std::thread::sleep(policy.poll.min(Duration::from_millis(20)));
        }
        backoff = (backoff * 2).min(policy.backoff_max);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};
    use std::sync::Mutex;

    fn shell(script: &str) -> io::Result<Child> {
        #[cfg(windows)]
        let mut cmd = {
            let mut c = Command::new("cmd");
            c.args(["/C", script]);
            c
        };
        #[cfg(not(windows))]
        let mut cmd = {
            let mut c = Command::new("sh");
            c.args(["-c", script]);
            c
        };
        cmd.stdout(Stdio::null()).stderr(Stdio::null()).spawn()
    }

    fn fast() -> WatchdogPolicy {
        WatchdogPolicy {
            max_restarts: 2,
            window: Duration::from_secs(30),
            backoff_initial: Duration::from_millis(10),
            backoff_max: Duration::from_millis(40),
            healthy_after: Duration::from_secs(30),
            poll: Duration::from_millis(5),
        }
    }

    #[test]
    fn a_clean_exit_ends_supervision_without_restarting() {
        let stop = AtomicBool::new(false);
        let mut spawns = 0;
        let outcome = supervise(
            |_| {
                spawns += 1;
                shell("exit 0")
            },
            &fast(),
            &stop,
            |_| {},
        );
        assert_eq!(outcome, Outcome::CleanExit);
        assert_eq!(spawns, 1);
    }

    #[test]
    fn a_crashed_child_is_restarted_until_it_exits_cleanly() {
        let stop = AtomicBool::new(false);
        let events = Mutex::new(Vec::new());
        let outcome = supervise(
            |attempt| shell(if attempt == 1 { "exit 7" } else { "exit 0" }),
            &fast(),
            &stop,
            |e| events.lock().unwrap().push(e),
        );
        assert_eq!(outcome, Outcome::CleanExit);
        let events = events.into_inner().unwrap();
        assert!(events.contains(&WatchdogEvent::Exited { code: Some(7) }));
        assert!(events
            .iter()
            .any(|e| matches!(e, WatchdogEvent::Restarting { .. })));
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e, WatchdogEvent::Started { .. }))
                .count(),
            2
        );
    }

    #[test]
    fn a_crash_loop_is_abandoned() {
        let stop = AtomicBool::new(false);
        let mut spawns = 0;
        let outcome = supervise(
            |_| {
                spawns += 1;
                shell("exit 3")
            },
            &fast(),
            &stop,
            |_| {},
        );
        assert_eq!(
            outcome,
            Outcome::GaveUp {
                restarts: 2,
                last_code: Some(3)
            }
        );
        assert_eq!(spawns, 3, "the first start plus two restarts");
    }

    #[test]
    fn spawn_failures_are_retried_then_abandoned() {
        let stop = AtomicBool::new(false);
        let outcome = supervise(
            |_| Err(io::Error::new(io::ErrorKind::NotFound, "no such program")),
            &fast(),
            &stop,
            |_| {},
        );
        assert!(matches!(
            outcome,
            Outcome::GaveUp {
                last_code: None,
                ..
            }
        ));
    }

    #[test]
    fn stopping_the_watchdog_terminates_the_child() {
        let stop = std::sync::Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let stopper = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            flag.store(true, Ordering::SeqCst);
        });
        #[cfg(windows)]
        let long = "ping -n 60 127.0.0.1 >nul";
        #[cfg(not(windows))]
        let long = "sleep 60";
        let started = Instant::now();
        let outcome = supervise(|_| shell(long), &fast(), &stop, |_| {});
        stopper.join().unwrap();
        assert_eq!(outcome, Outcome::Stopped);
        assert!(started.elapsed() < Duration::from_secs(20));
    }
}
