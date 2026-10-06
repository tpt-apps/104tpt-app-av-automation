# tpt-app-av-automation-model

The versioned rule-pack domain model and validator for [TPT AV Automation](../../README.md).

A *rule pack* is a human-readable YAML file: **WHEN** a trigger fires, **IF** conditions hold,
**THEN** run an ordered chain of actions. This crate defines those types, parses them and checks
them. It is pure data plus validation: **no I/O, no networking, no clock reads**. That is what lets
the engine, CLI, service and desktop UI share one implementation of the format.

## What is in it

- **Types:** `RulePack`, `Rule`, `Trigger` / `TriggerSpec`, `Condition` / `ConditionSpec`,
  `ActionStep` / `ActionSpec`, `Policy`, `Priority`, `FailurePolicy`.
- **Triggers:** schedule (cron-like, interval, one-shot, solar sunrise/sunset with offsets), OSC,
  MIDI 1.0 and 2.0 message kinds, DMX level comparisons (`DmxComparison`, including edge crossings),
  device health, heartbeat-missed, device parameter, manual, API.
- **Conditions:** device health, time window, DMX channel level, device parameter, evaluated to a
  three-valued `ConditionResult`.
- **Actions:** OSC, MIDI, DMX channels / scene, media (`MediaOperation`), notify operator, log
  incident, wait, invoke rule, library fragments, sandboxed external program.
- **Outcomes:** `ExecutionRecord`, `ExecutionStatus`, `ActionOutcome` and per-condition and
  per-action result records, so every execution can say *why* it fired.
- **Schedules:** `CronSchedule` and a self-contained solar calculator (`solar_day`, `SolarSite`).
- **Validation:** `validate_pack` returns located `Diagnostic`s (errors and warnings), not a boolean.

## Example

```rust
use tpt_app_av_automation_model::{has_errors, validate_pack, RulePack};

let yaml = r#"
format_version: 1
name: Main Hall
revision: 1
rules:
  - id: event-start
    name: Main Hall - Event Start
    version: 1
    armed: false
    trigger:
      type: schedule
      at: "18:55"
      days: [mon, tue, wed, thu, fri]
    actions:
      - id: note
        type: notify.operator
        message: "House lights to half"
"#;

let pack = RulePack::from_yaml_str(yaml).unwrap();
assert!(!pack.rules[0].armed, "rules load disarmed by default");
assert!(!has_errors(&validate_pack(&pack)));
```

## Guarantees

- `deny_unknown_fields` throughout: a typo in a live-show pack is a hard error, never a silently
  ignored key.
- Semantic checks the type system cannot express: duplicate rule ids, runaway action chains
  (`MAX_CHAIN_LENGTH`), unknown action types, DMX channels outside 0–511, unsatisfiable
  trigger/condition pairs, invalid cron expressions.
- `format_version: 1` is the stable on-disk format (`FORMAT_VERSION`).

See [docs/rule-format.md](../../docs/rule-format.md), [docs/rule-model.md](../../docs/rule-model.md),
[docs/trigger-catalogue.md](../../docs/trigger-catalogue.md) and
[docs/action-catalogue.md](../../docs/action-catalogue.md).

## Licence

Dual-licensed under [MIT](../../LICENSE-MIT) or [Apache-2.0](../../LICENSE-APACHE), at your option.
