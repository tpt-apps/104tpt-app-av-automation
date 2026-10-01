# Rule model

The domain model from spec §6, as implemented in `tpt-app-av-automation-model`.

## Rule

`id`, `name`, `version`, `trigger`, `conditions`, `actions`, `policy`, `priority`, `armed`, `tags`.

* **`armed: false`** — the rule is loaded and evaluated but never sends anything; it records what
  would have happened. This is the default. Editing a rule (bumping `version`) and hot-reloading a
  pack disarms it again; only an unchanged `id` + `version` keeps its armed state.
* **`priority`** orders rules matched by the same event and decides conflicts (see
  [architecture](architecture.md)).

## Condition: three values

A condition is `Met`, `NotMet` or `Unknown`. `Unknown` means the condition could not be evaluated —
a device that has never reported, a DMX channel never observed, a rule that does not exist. A rule
**does not fire on `Unknown`** unless that condition sets `fire_on_unknown: true`. `NotMet` always
blocks, whatever the flag says.

| `type` | Fields | Unknown when |
|--------|--------|--------------|
| `device_health` | `device`, `equals` | the device is unregistered or has not reported |
| `time_window` | `from`, `to` | no wall-clock time is available. `from` inclusive, `to` exclusive |
| `dmx_channel` | `universe`, `channel`, `comparison`, `value` | the channel was never observed |
| `device_parameter` | `device`, `parameter`, `comparison`, `value` | the parameter was never reported |
| `rule_armed` | `rule` | the rule does not exist |
| `rule_has_tag` | `tag` | never (checks the rule being evaluated) |

Comparisons: `eq`, `ne`, `gt`, `gte`, `lt`, `lte`.

All conditions are evaluated (no short-circuit) and all are recorded with what was observed, so the
record explains the decision.

## Execution record (spec §3.3, §6.6)

Every execution — fired, skipped, simulated or live — produces one record containing: the execution
id, rule id/name/**version**, trigger type and the exact event that matched, every condition's
result and observed value, every action's outcome, detail, duration and failure policy, the overall
status, whether it was simulated, and the total duration.

Statuses: `success`, `partial_failure`, `failed`, `skipped`. A chain in which some steps succeeded and
some failed is `partial_failure`, never `success` (spec §3.4).

## Determinism (spec §3.2)

Given the same pack, registry state, clock reading and event, the engine returns the same decision
and the same ordered records (there is a test comparing serialized output of two runs). The things
that are not deterministic — wall-clock time, device responses, real sleeping — are injected and
isolated behind `Clock`, `Endpoint` and `Sleeper`.

Execution ids (`exec-000001`, …) are sequential and continue across restarts.
