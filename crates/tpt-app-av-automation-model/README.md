# tpt-app-av-automation-model

The versioned rule-pack domain model (spec §6, §9).

Responsibilities:

- `RulePack` / `Rule` / `Trigger` / `Condition` / `ActionStep` types.
- YAML deserialization with `deny_unknown_fields`, so a typo in a live-show rule pack is a hard
  error rather than a silently ignored key.
- A validator producing located [`Diagnostic`](tpt_app_av-automation-core)s (errors **and**
  warnings) instead of a boolean.
- Semantic checks the type system cannot express: duplicate rule ids, runaway action chains,
  unknown action types, DMX channels outside 0-511, unsatisfiable trigger/condition pairs.

The model crate is pure data plus validation. It performs no I/O, no networking and no clock reads,
which is what allows the engine, CLI and desktop UI to share one implementation (spec §3.6).

## Licence

Dual MIT / Apache-2.0.

```text
d:\Programming\1PRODUCTION\Open Source\104tpt-app-av-automation\crates\tpt-app-av-automation-model\README.md
```