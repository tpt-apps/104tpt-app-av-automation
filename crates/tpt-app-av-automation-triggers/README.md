# tpt-app-av-automation-triggers

Trigger sources for [TPT AV Automation](../../README.md): the schedule driver and the inbound
control-message gate. This crate turns the outside world into normalized, deterministic `Event`s
(see the `core` crate). Matching an event against a rule's trigger is a pure function that lives in
the `model` crate.

## Components

### `Scheduler`

Turns the passage of time into schedule events.

- **Edge-triggered and idempotent within a minute:** polling twice in the same minute fires once.
- Handles cron-like, interval, one-shot and solar (sunrise/sunset ± offset) schedules.
- `SchedulerState` is serializable, so a restart never re-fires a one-shot.
- `LocalClock` / `LocalMoment` convert an injected clock into local wall time using a configured
  UTC offset. Nothing reads the OS clock behind your back.

### `Inbound`

Parses and validates raw **OSC, MIDI (1.0 and UMP/2.0), Art-Net and sACN** datagrams into events.

- All of these protocols are unauthenticated, so everything is treated as untrusted: malformed
  packets are rejected and counted, never panicked on.
- Per-source rate limiting with adaptive backoff, configured through `InboundLimits`. Counters
  (`rate_limited`, `backed_off`, `backoff_episodes`) feed the service health report.
- `DmxTracker` remembers the last level of each watched DMX channel so edge triggers
  (crossed above / crossed below) work across packets.

## Example

```rust,ignore
use tpt_app_av_automation_triggers::{Inbound, InboundLimits};
use tpt_app_av_automation_core::Timestamp;

let inbound = Inbound::new(InboundLimits::default());
// `source` identifies the sender (used for rate limiting); the result is zero or more events.
let events = inbound.osc("192.168.1.20:9000", &datagram, Timestamp::now())?;
```

## Foundation crates

Wire decoding is delegated to the open `tpt-av-control-osc`, `tpt-av-control-midi` and
`tpt-av-control-dmx` codecs. This crate adds validation, limiting and event normalization.

See [docs/trigger-catalogue.md](../../docs/trigger-catalogue.md).

## Licence

Dual-licensed under [MIT](../../LICENSE-MIT) or [Apache-2.0](../../LICENSE-APACHE), at your option.
