//! Error taxonomy and located diagnostics.

use std::fmt;

use serde::{Deserialize, Serialize};

/// Convenient result alias used across the workspace.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Severity of a diagnostic emitted while validating configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// Configuration is unusable; the rule/pack will not load.
    Error,
    /// Configuration loads but behaves in a surprising way.
    Warning,
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Severity::Error => write!(f, "error"),
            Severity::Warning => write!(f, "warning"),
        }
    }
}

/// A single configuration diagnostic with a human-readable location.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnostic {
    /// Severity of the diagnostic.
    pub severity: Severity,
    /// Dotted path to the offending value, e.g. `rules[0].trigger.address`.
    pub location: String,
    /// Human-readable explanation.
    pub message: String,
}

impl Diagnostic {
    /// Creates an error-level diagnostic.
    pub fn error(location: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Error,
            location: location.into(),
            message: message.into(),
        }
    }

    /// Creates a warning-level diagnostic.
    pub fn warning(location: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Warning,
            location: location.into(),
            message: message.into(),
        }
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} at {}: {}",
            self.severity, self.location, self.message
        )
    }
}

/// The workspace-wide error taxonomy.
///
/// Spec §16: inbound control traffic is untrusted, so this taxonomy is deliberately total. There
/// is no "protocol error" variant that callers must map to a panic or a generic string; every
/// failure path has a variant an operator can act on.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// YAML/JSON rule-pack source could not be parsed.
    #[error("rule pack parse error: {0}")]
    Parse(String),

    /// The rule pack parsed but failed validation.
    #[error("rule pack validation failed with {} error(s)", count_errors(.0))]
    Validation(Vec<Diagnostic>),

    /// A referenced device, rule, scene or endpoint does not exist.
    #[error("unknown {kind} `{id}`")]
    NotFound {
        /// What kind of entity was referenced, e.g. `device`.
        kind: &'static str,
        /// The identifier that could not be resolved.
        id: String,
    },

    /// A rule id was used twice within one rule pack.
    #[error("duplicate rule id `{0}`")]
    DuplicateRuleId(String),

    /// An operation was rejected because it would mutate a rule pack or registry unsafely.
    #[error("invalid operation: {0}")]
    InvalidOperation(String),

    /// An outbound control-protocol operation failed.
    #[error("control protocol error: {0}")]
    Control(String),

    /// An inbound control-protocol message was malformed or failed validation.
    #[error("malformed inbound message: {0}")]
    MalformedMessage(String),

    /// A bounded, sandboxed subprocess action failed.
    #[error("subprocess action failed: {0}")]
    Subprocess(String),

    /// Local persistence failed.
    #[error("storage error: {0}")]
    Storage(String),

    /// A bounded operation exceeded its timeout.
    #[error("operation timed out after {0} ms")]
    Timeout(u64),

    /// Underlying I/O failure.
    #[error("io error: {0}")]
    Io(String),

    /// A rule was cancelled by the operator mid-chain.
    #[error("execution cancelled")]
    Cancelled,

    /// Catch-all for internal invariant violations.
    #[error("internal error: {0}")]
    Internal(String),
}

fn count_errors(diagnostics: &[Diagnostic]) -> usize {
    diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .count()
}
impl Error {
    /// Collects multiple diagnostics into a single validation error.
    pub fn validation(diagnostics: Vec<Diagnostic>) -> Self {
        Error::Validation(diagnostics)
    }

    /// Returns the diagnostics when this is a [`Error::Validation`].
    pub fn diagnostics(&self) -> &[Diagnostic] {
        match self {
            Error::Validation(d) => d,
            _ => &[],
        }
    }

    /// Whether this error describes invalid configuration rather than a runtime failure.
    ///
    /// Shells use this to pick exit code 4 (CONFIGURATION_ERROR) rather than 6 (INTERNAL_ERROR),
    /// keeping the CLI exit-code contract stable (spec §13).
    pub fn is_configuration(&self) -> bool {
        matches!(
            self,
            Error::Parse(_) | Error::Validation(_) | Error::NotFound { .. }
        )
    }
}

impl From<std::io::Error> for Error {
    fn from(value: std::io::Error) -> Self {
        Error::Io(value.to_string())
    }
}

impl From<serde_json::Error> for Error {
    fn from(value: serde_json::Error) -> Self {
        Error::Parse(value.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostics_render_with_location() {
        let d = Diagnostic::error("rules[0].trigger.address", "must start with '/'");
        assert_eq!(
            d.to_string(),
            "error at rules[0].trigger.address: must start with '/'"
        );
    }

    #[test]
    fn validation_error_counts_only_errors() {
        let err = Error::validation(vec![
            Diagnostic::error("a", "bad"),
            Diagnostic::warning("b", "iffy"),
        ]);
        assert_eq!(err.diagnostics().len(), 2);
        assert!(err.to_string().contains("1 error(s)"));
    }

    #[test]
    fn configuration_errors_are_classified() {
        assert!(Error::Parse("bad yaml".into()).is_configuration());
        assert!(Error::NotFound {
            kind: "device",
            id: "d1".into()
        }
        .is_configuration());
        assert!(!Error::Control("no route".into()).is_configuration());
        assert!(!Error::Internal("bug".into()).is_configuration());
    }
}
