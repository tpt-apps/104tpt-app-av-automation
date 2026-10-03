//! Conflict detection between action chains targeting the same endpoint (spec §10.1).
//!
//! Resolution is deterministic: rules matched by one event run in `(priority, rule id)` order, and
//! the first rule to write to a slot of a device owns that slot for the rest of the dispatch. A
//! later rule writing a *different* value to an overlapping slot is rejected and the conflict is
//! recorded on its step as `Skipped`. Writing the same value twice is not a conflict.

use std::collections::BTreeMap;

use tpt_app_av_automation_devices::Command;

/// Addressable parts of a device that a command writes.
fn slots(command: &Command) -> Vec<String> {
    match command {
        Command::Osc { address, .. } => vec![format!("osc:{address}")],
        Command::Midi {
            channel,
            kind,
            number,
            ..
        } => vec![format!("midi:{channel}:{kind}:{number}")],
        Command::DmxChannels {
            universe,
            start_channel,
            values,
            ..
        } => (0..values.len())
            .map(|i| format!("dmx:{universe}:{}", usize::from(*start_channel) + i))
            .collect(),
        Command::DmxUniverse { universe, .. } => vec![format!("dmx:{universe}:*")],
        Command::DmxScene { .. } => vec!["dmx:*".to_string()],
        Command::VideoSource { .. } => vec!["video".to_string()],
        Command::AudioRoute { to, .. } => vec![format!("audio:{to}")],
    }
}

fn overlaps(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    match (a.strip_suffix('*'), b.strip_suffix('*')) {
        (Some(prefix), _) if b.starts_with(prefix) => true,
        (_, Some(prefix)) if a.starts_with(prefix) => true,
        _ => false,
    }
}

#[derive(Debug, Clone)]
struct Claim {
    rule: String,
    priority: String,
    command: Command,
    slots: Vec<String>,
}

/// Claims made so far during one event dispatch.
#[derive(Debug, Default)]
pub(crate) struct Claims {
    live: BTreeMap<String, Vec<Claim>>,
    simulated: BTreeMap<String, Vec<Claim>>,
}

impl Claims {
    /// Checks a write against existing claims and registers it when it does not conflict.
    ///
    /// Live writes only conflict with other live writes — a disarmed rule never really sends, so it
    /// must not block an armed one. Simulated writes are checked against both, so a simulation
    /// shows the conflicts that arming the rule would cause.
    pub(crate) fn claim(
        &mut self,
        device: &str,
        rule: &str,
        priority: &str,
        command: &Command,
        simulated: bool,
    ) -> Result<(), String> {
        let mine = slots(command);
        let conflicting = |claims: &BTreeMap<String, Vec<Claim>>| {
            claims.get(device).and_then(|list| {
                list.iter()
                    .find(|c| {
                        c.rule != rule
                            && c.command != *command
                            && c.slots.iter().any(|a| mine.iter().any(|b| overlaps(a, b)))
                    })
                    .cloned()
            })
        };
        let hit = conflicting(&self.live).or_else(|| {
            if simulated {
                conflicting(&self.simulated)
            } else {
                None
            }
        });
        if let Some(owner) = hit {
            return Err(format!(
                "conflict on device `{device}`: slot already written by rule `{}` ({} priority)",
                owner.rule, owner.priority
            ));
        }
        let map = if simulated {
            &mut self.simulated
        } else {
            &mut self.live
        };
        map.entry(device.to_owned()).or_default().push(Claim {
            rule: rule.to_owned(),
            priority: priority.to_owned(),
            command: command.clone(),
            slots: mine,
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tpt_app_av_automation_model::DmxTransport;

    fn osc(address: &str, v: f64) -> Command {
        Command::Osc {
            address: address.into(),
            args: vec![v],
        }
    }

    fn dmx(start: u16, values: Vec<u8>) -> Command {
        Command::DmxChannels {
            universe: 1,
            start_channel: start,
            values,
            transport: DmxTransport::ArtNet,
        }
    }

    #[test]
    fn different_values_to_the_same_address_conflict() {
        let mut c = Claims::default();
        c.claim("d", "a", "critical", &osc("/x", 1.0), false)
            .unwrap();
        let err = c
            .claim("d", "b", "normal", &osc("/x", 2.0), false)
            .unwrap_err();
        assert!(err.contains("rule `a`"));
    }

    #[test]
    fn identical_writes_and_different_slots_do_not_conflict() {
        let mut c = Claims::default();
        c.claim("d", "a", "normal", &osc("/x", 1.0), false).unwrap();
        c.claim("d", "b", "normal", &osc("/x", 1.0), false).unwrap();
        c.claim("d", "b", "normal", &osc("/y", 9.0), false).unwrap();
        c.claim("other", "b", "normal", &osc("/x", 9.0), false)
            .unwrap();
    }

    #[test]
    fn a_rule_never_conflicts_with_itself() {
        let mut c = Claims::default();
        c.claim("d", "a", "normal", &osc("/x", 1.0), false).unwrap();
        c.claim("d", "a", "normal", &osc("/x", 2.0), false).unwrap();
    }

    #[test]
    fn dmx_ranges_conflict_only_when_they_overlap() {
        let mut c = Claims::default();
        c.claim("d", "a", "normal", &dmx(0, vec![1, 2, 3]), false)
            .unwrap();
        c.claim("d", "b", "normal", &dmx(3, vec![9]), false)
            .unwrap();
        assert!(c
            .claim("d", "b", "normal", &dmx(2, vec![9]), false)
            .is_err());
    }

    #[test]
    fn scenes_and_universes_claim_wildcards() {
        let mut c = Claims::default();
        c.claim(
            "d",
            "a",
            "normal",
            &Command::DmxScene {
                scene: "s".into(),
                fade_ms: 0,
            },
            false,
        )
        .unwrap();
        assert!(c
            .claim("d", "b", "normal", &dmx(0, vec![1]), false)
            .is_err());
    }

    #[test]
    fn disarmed_rules_do_not_block_live_ones_but_simulation_shows_the_conflict() {
        let mut c = Claims::default();
        c.claim("d", "sim", "critical", &osc("/x", 1.0), true)
            .unwrap();
        c.claim("d", "live", "normal", &osc("/x", 2.0), false)
            .unwrap();
        assert!(c
            .claim("d", "sim2", "normal", &osc("/x", 3.0), true)
            .is_err());
    }
}
