//! Outbound endpoint abstraction and the fault-injecting virtual endpoint (spec §18.2).

use std::sync::{Arc, Mutex};

use crate::command::Command;

/// Why an endpoint could not deliver a command.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EndpointError {
    /// The device could not be reached (offline, network down).
    #[error("device unreachable: {0}")]
    Unreachable(String),
    /// The command is not supported or was rejected by the endpoint.
    #[error("command rejected: {0}")]
    Rejected(String),
    /// The endpoint did not answer in time.
    #[error("endpoint timed out")]
    Timeout,
}

impl EndpointError {
    /// Whether this failure means the device itself is unhealthy (as opposed to a bad command).
    pub fn is_device_fault(&self) -> bool {
        matches!(self, EndpointError::Unreachable(_) | EndpointError::Timeout)
    }
}

/// An outbound control endpoint for one device.
pub trait Endpoint: Send + Sync {
    /// Delivers a command.
    fn send(&self, command: &Command) -> Result<(), EndpointError>;

    /// Probes reachability. The default assumes a connectionless transport is reachable.
    fn ping(&self) -> Result<(), EndpointError> {
        Ok(())
    }
}

/// Faults a [`VirtualEndpoint`] can inject.
#[derive(Debug, Clone, Default)]
pub struct Faults {
    /// Every send fails as unreachable.
    pub offline: bool,
    /// The next `n` sends are silently dropped: reported as success but never recorded.
    pub drop_next: u32,
    /// After this many further successful sends the endpoint goes offline ("mid-chain dropout").
    pub offline_after: Option<u32>,
    /// Every send times out.
    pub timeout: bool,
    /// Every send blocks this long before completing (for exercising action timeouts).
    pub delay_ms: u64,
}

#[derive(Debug, Default)]
struct VirtualState {
    sent: Vec<Command>,
    faults: Faults,
    attempts: u64,
}

/// An in-memory endpoint that records commands and injects faults.
#[derive(Debug, Clone, Default)]
pub struct VirtualEndpoint {
    state: Arc<Mutex<VirtualState>>,
}

impl VirtualEndpoint {
    /// Creates a healthy endpoint.
    pub fn new() -> Self {
        Self::default()
    }

    /// Commands delivered so far.
    pub fn sent(&self) -> Vec<Command> {
        self.lock().sent.clone()
    }

    /// Number of send attempts, including failed and dropped ones.
    pub fn attempts(&self) -> u64 {
        self.lock().attempts
    }

    /// Replaces the injected faults.
    pub fn set_faults(&self, faults: Faults) {
        self.lock().faults = faults;
    }

    /// Takes the device offline or brings it back.
    pub fn set_offline(&self, offline: bool) {
        self.lock().faults.offline = offline;
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, VirtualState> {
        // A poisoned lock only means a test thread panicked; the data is still usable.
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl Endpoint for VirtualEndpoint {
    fn send(&self, command: &Command) -> Result<(), EndpointError> {
        let delay = self.lock().faults.delay_ms;
        if delay > 0 {
            std::thread::sleep(std::time::Duration::from_millis(delay));
        }
        let mut state = self.lock();
        state.attempts += 1;
        if state.faults.offline {
            return Err(EndpointError::Unreachable("virtual device offline".into()));
        }
        if state.faults.timeout {
            return Err(EndpointError::Timeout);
        }
        if state.faults.drop_next > 0 {
            state.faults.drop_next -= 1;
            return Ok(());
        }
        if let Some(remaining) = state.faults.offline_after {
            if remaining == 0 {
                state.faults.offline = true;
                state.faults.offline_after = None;
                return Err(EndpointError::Unreachable(
                    "virtual device went offline".into(),
                ));
            }
            state.faults.offline_after = Some(remaining - 1);
        }
        state.sent.push(command.clone());
        Ok(())
    }

    fn ping(&self) -> Result<(), EndpointError> {
        let state = self.lock();
        if state.faults.offline {
            Err(EndpointError::Unreachable("virtual device offline".into()))
        } else if state.faults.timeout {
            Err(EndpointError::Timeout)
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn osc() -> Command {
        Command::Osc {
            address: "/a".into(),
            args: vec![1.0],
        }
    }

    #[test]
    fn records_commands() {
        let ep = VirtualEndpoint::new();
        ep.send(&osc()).unwrap();
        assert_eq!(ep.sent(), vec![osc()]);
    }

    #[test]
    fn offline_fails_and_recovers() {
        let ep = VirtualEndpoint::new();
        ep.set_offline(true);
        assert!(matches!(ep.send(&osc()), Err(EndpointError::Unreachable(_))));
        assert!(ep.ping().is_err());
        ep.set_offline(false);
        assert!(ep.send(&osc()).is_ok());
        assert_eq!(ep.attempts(), 2);
    }

    #[test]
    fn dropped_messages_report_success_but_are_not_recorded() {
        let ep = VirtualEndpoint::new();
        ep.set_faults(Faults {
            drop_next: 1,
            ..Faults::default()
        });
        ep.send(&osc()).unwrap();
        ep.send(&osc()).unwrap();
        assert_eq!(ep.sent().len(), 1);
    }

    #[test]
    fn goes_offline_mid_chain() {
        let ep = VirtualEndpoint::new();
        ep.set_faults(Faults {
            offline_after: Some(1),
            ..Faults::default()
        });
        assert!(ep.send(&osc()).is_ok());
        let err = ep.send(&osc()).unwrap_err();
        assert!(err.is_device_fault());
        assert!(ep.send(&osc()).is_err());
    }

    #[test]
    fn timeouts_are_device_faults() {
        let ep = VirtualEndpoint::new();
        ep.set_faults(Faults {
            timeout: true,
            ..Faults::default()
        });
        assert_eq!(ep.send(&osc()), Err(EndpointError::Timeout));
    }
}
