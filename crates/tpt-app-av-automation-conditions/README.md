# tpt-app-av-automation-conditions

Condition evaluation for the deterministic rule core of [TPT AV Automation](../../README.md).

Evaluation is a **pure function** of a `Condition` and an `EvaluationContext` snapshot. The context
carries everything a condition may read: local time, device health, DMX levels, device parameters
and the armed set. The same inputs always give the same answer.

## Three-valued results

| Result | Meaning |
|--------|---------|
| `Met` | The condition is known to hold. |
| `NotMet` | The condition is known **not** to hold. |
| `Unknown` | The data needed is missing, e.g. a device we have never heard from. |

"We have never heard from that device" is reported as `Unknown`, **never** as `NotMet`. The engine
skips a rule whose gate is not fully `Met`. That is deliberate for live production: a silent device
must not look like a healthy one.

## API

- `evaluate(&Condition, &EvaluationContext) -> Evaluation`: one condition plus the reason string
  recorded in the execution log.
- `gate(&[Condition], &EvaluationContext) -> Gate`: all of a rule's conditions at once, with every
  individual result retained so the record can explain *why* a rule did or did not run.
- `EvaluationContext`: the snapshot. No I/O.

Supported conditions: `DeviceHealth`, `TimeWindow` (including windows that cross midnight and
day-of-week filters), `DmxChannel` and `DeviceParameter`, with `Comparison` operators.

## Example

```rust,ignore
use tpt_app_av_automation_conditions::{gate, EvaluationContext};

let ctx = EvaluationContext::default(); // populate time, device health, DMX levels, ...
let result = gate(&rule.conditions, &ctx);
// Inspect `result` to see which condition blocked the rule and why.
```

See [docs/rule-model.md](../../docs/rule-model.md) for the condition forms as written in YAML.

## Licence

Dual-licensed under [MIT](../../LICENSE-MIT) or [Apache-2.0](../../LICENSE-APACHE), at your option.
