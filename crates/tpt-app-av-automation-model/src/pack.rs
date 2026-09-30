//! The rule pack: the central product abstraction (spec §9).

use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use tpt_app_av_automation_core::{Diagnostic, Error, Result};

use crate::action::ActionSpec;
use crate::rule::Rule;

/// The rule-pack format version understood by this build.
///
/// A pack declaring a higher `format_version` is rejected rather than silently mis-parsed, so an
/// operator can never arm a pack written for a future engine (spec §9).
pub const FORMAT_VERSION: u32 = 1;

/// Longest permitted action chain, main or fallback.
///
/// Action chains execute inline; an unbounded chain is a configuration bug that would hang a show,
/// so it is capped at validation time.
pub const MAX_CHAIN_LENGTH: usize = 64;

/// A named, versioned collection of rules.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RulePack {
    /// Rule-pack format version.
    pub format_version: u32,
    /// Human-readable pack name.
    pub name: String,
    /// Optional free-form description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Declared pack revision, for diffing and history.
    #[serde(default = "default_revision")]
    pub revision: u32,
    /// The rules in this pack. Order is preserved and is the load order.
    #[serde(default, rename = "rules")]
    pub rules: Vec<Rule>,
    /// Named reusable action fragments referenced by `workflow.use_action`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub action_library: BTreeMap<String, ActionSpec>,
}

fn default_revision() -> u32 {
    1
}

impl RulePack {
    /// Builds an empty pack with the current format version.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            format_version: FORMAT_VERSION,
            name: name.into(),
            description: None,
            revision: 1,
            rules: Vec::new(),
            action_library: BTreeMap::new(),
        }
    }

    /// Parses and validates a pack from YAML source.
    pub fn from_yaml_str(source: &str) -> Result<Self> {
        let pack: RulePack =
            serde_yaml::from_str(source).map_err(|e| Error::Parse(format!("{e}")))?;
        pack.validate()?;
        Ok(pack)
    }

    /// Parses and validates a pack from a file on disk.
    pub fn from_path(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let source = std::fs::read_to_string(path)
            .map_err(|e| Error::Storage(format!("{}: {e}", path.display())))?;
        Self::from_yaml_str(&source)
            .map_err(|e| Error::Storage(format!("{}: {e}", path.display())))
    }

    /// Runs validation and returns every diagnostic, errors and warnings alike.
    ///
    /// Returns `Err` when at least one diagnostic is an error; warnings alone still load.
    pub fn validate(&self) -> Result<Vec<Diagnostic>> {
        crate::validate::validate_pack(self)
    }

    /// Looks up a rule by id.
    pub fn rule(&self, id: &str) -> Option<&Rule> {
        self.rules.iter().find(|r| r.id.as_str() == id)
    }

    /// Whether a rule with this id exists.
    pub fn contains(&self, id: &str) -> bool {
        self.rule(id).is_some()
    }

    /// Rules that are armed, i.e. permitted to perform real side effects.
    pub fn armed_rules(&self) -> impl Iterator<Item = &Rule> {
        self.rules.iter().filter(|r| r.armed)
    }

    /// Serializes the pack back to YAML.
    pub fn to_yaml_string(&self) -> Result<String> {
        serde_yaml::to_string(self).map_err(|e| Error::Parse(e.to_string()))
    }

    /// Summary counts for the CLI and desktop dashboard.
    pub fn summary(&self) -> PackSummary {
        PackSummary {
            rules: self.rules.len(),
            armed: self.rules.iter().filter(|r| r.armed).count(),
            disarmed: self.rules.iter().filter(|r| !r.armed).count(),
            library_fragments: self.action_library.len(),
        }
    }
}

impl fmt::Display for RulePack {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} (v{} revision {}, {} rules)",
            self.name,
            self.format_version,
            self.revision,
            self.rules.len()
        )
    }
}

/// Counted view of a rule pack, used by `validate` and the dashboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackSummary {
    /// Total rules in the pack.
    pub rules: usize,
    /// Rules permitted to send real actions.
    pub armed: usize,
    /// Rules loaded simulation-only.
    pub disarmed: usize,
    /// Number of reusable action fragments.
    pub library_fragments: usize,
}

/// A YAML source that has already been validated.
///
/// Produced by [`YamlPackSource::from_str`] so a shell cannot accidentally feed an unvalidated
/// source to the engine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct YamlPackSource(pub String);

impl FromStr for YamlPackSource {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        RulePack::from_yaml_str(s)?;
        Ok(YamlPackSource(s.to_owned()))
    }
}