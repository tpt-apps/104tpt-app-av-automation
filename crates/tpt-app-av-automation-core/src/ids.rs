//! Strongly typed identifiers.

use std::fmt;

use serde::{Deserialize, Serialize};

macro_rules! string_id {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            /// Wraps a string identifier.
            pub fn new(value: impl Into<String>) -> Self {
                Self(value.into())
            }

            /// Borrows the identifier as a string slice.
            pub fn as_str(&self) -> &str {
                &self.0
            }

            /// Consumes the wrapper and returns the inner string.
            pub fn into_inner(self) -> String {
                self.0
            }

            /// Whether the identifier is empty or only whitespace.
            pub fn is_blank(&self) -> bool {
                self.0.trim().is_empty()
            }
        }

        impl Default for $name {
            /// An empty identifier.
            ///
            /// Ids default to empty in YAML so the loader can derive them from a display name;
            /// validation rejects any id that is still blank afterwards.
            fn default() -> Self {
                Self(String::new())
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self(value.to_owned())
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self(value)
            }
        }
    };
}

string_id!(
    /// Stable identifier of a rule inside a rule pack.
    RuleId
);
string_id!(
    /// Stable identifier of a condition inside a rule.
    ConditionId
);
string_id!(
    /// Stable identifier of an action step inside a rule.
    ActionId
);
string_id!(
    /// Stable identifier of a device/endpoint in the registry.
    DeviceId
);
string_id!(
    /// Identifier of a single recorded execution.
    ExecutionId
);

/// Derives a stable rule id from a display name.
///
/// Ids must be stable across exports and diffs, so the slug is derived from the name only and is
/// never regenerated from load order.
pub fn slugify_rule_id(name: &str) -> String {
    let mut slug = String::with_capacity(name.len());
    let mut previous_dash = false;
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            previous_dash = false;
        } else if !previous_dash && !slug.is_empty() {
            slug.push('-');
            previous_dash = true;
        }
    }
    while slug.ends_with('-') {
        slug.pop();
    }
    slug
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_round_trip_through_serde() {
        let id = RuleId::new("main-hall-start");
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, "\"main-hall-start\"");
        assert_eq!(serde_json::from_str::<RuleId>(&json).unwrap(), id);
    }

    #[test]
    fn blank_detection_ignores_whitespace() {
        assert!(RuleId::new("   ").is_blank());
        assert!(!RuleId::new(" a ").is_blank());
    }

    #[test]
    fn slug_is_stable_and_url_safe() {
        assert_eq!(
            slugify_rule_id("Main Hall - Event Start"),
            "main-hall-event-start"
        );
        assert_eq!(slugify_rule_id("  Spaced   Out  "), "spaced-out");
        assert_eq!(slugify_rule_id("!!!"), "");
        assert_eq!(slugify_rule_id("DMX Scene Recall"), "dmx-scene-recall");
    }
}
