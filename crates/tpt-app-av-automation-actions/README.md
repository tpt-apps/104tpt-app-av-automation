# tpt-app-av-automation-actions

Action executors and outbound sinks for [TPT AV Automation](../../README.md).

`Actions` is the single place where a rule's action step touches the outside world. It offers two
entry points with deliberately identical signatures:

| Method | Behaviour |
|--------|-----------|
| `Actions::execute` | Performs the step: sends the control message, raises the alert, runs the sandboxed program. |
| `Actions::simulate` | Describes what `execute` would do and performs **nothing**. |

`simulate` has no access to endpoints at all. "Simulation never sends a real control message" is
therefore a structural guarantee of the type system, not a convention someone has to remember.

## Supported actions

OSC, MIDI (1.0 and 2.0), DMX channels / universe / scene (Art-Net, sACN, serial DMX512), media
(`media.video_source`, `media.audio_route`, delivered as OSC), notify operator, log incident, wait,
invoke rule, library fragments, and a **sandboxed external program**.

## Safety properties

- **Cancellation:** every chain carries a `CancelToken`; waits and long steps observe it, so an
  operator can stop a running chain.
- **Sandboxed exec:** `ExecPolicy` is an allow-list. External programs are **disabled by default**
  and only programs explicitly allowed run, with no shell interpretation of arguments.
- **Pluggable sinks:** operator notices and incidents go through the `Notifier` and `IncidentSink`
  traits (`TracingSink` for logging, `MemorySink` for tests). Time-based waiting goes through the
  `Sleeper` trait (`ThreadSleeper`, `NullSleeper`), so tests never actually sleep.
- **Honest outcomes:** each step yields an `ActionOutcome`. A partly failed chain is never reported
  as a success.

## Example

```rust,ignore
use tpt_app_av_automation_actions::{Actions, CancelToken};

let cancel = CancelToken::default();
// Same shape for both calls; only `execute` can reach a device.
let described = actions.simulate(&step);
let outcome = actions.execute(&step, &cancel);
```

## Where it fits

Depends on `core`, `model` and `devices`; used by the `engine`. See
[docs/action-catalogue.md](../../docs/action-catalogue.md).

## Licence

Dual-licensed under [MIT](../../LICENSE-MIT) or [Apache-2.0](../../LICENSE-APACHE), at your option.
