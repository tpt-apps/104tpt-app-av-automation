# Trigger catalogue

Every trigger is matched by a pure function of `(trigger, event)` in `model::Trigger::matches`; it
never reads a clock or a device. Each one can run against a virtual source (spec §6.2).

| `type` | Fields | Fires when | Foundation crate |
|--------|--------|-----------|------------------|
| `schedule` | `at: "HH:MM"`, `days`, `once` **or** `interval_ms` | the local minute matches (once per minute however often polled); a `once` schedule fires a single time ever; an interval fires every N ms, never as a catch-up burst | — |
| `osc` | `address` (`*` = one path segment), `arg_equals`, `min_args` | a matching OSC message arrives | `tpt-av-control-osc` |
| `midi` | `message` (`note_on`/`note_off`/`control_change`/`program_change`), `channel`, `number`, `value` | a matching MIDI 1.0 message arrives; omitted fields match anything | `tpt-av-control-midi` |
| `dmx` | `universe`, `channel`, `comparison`, `value` | the channel satisfies `equals`, `greater_than`, `less_than`, `crossed_above`, `crossed_below` or `changed`. Edge comparisons use the previous observation; an unseen channel counts as 0 | `tpt-av-control-dmx` (Art-Net, sACN) |
| `device_state` | `device`, `state` | the device transitions to `online`, `degraded`, `offline` or `unknown` | — |
| `heartbeat_missed` | `device`, `after_ms` | the device was silent for at least `after_ms` | — |
| `device_parameter` | `device`, `parameter`, `comparison`, `value` | a reported parameter satisfies the comparison | — |
| `manual` | — | an operator runs the rule now (`run_rule`, `POST /rules/:id/run`, `simulate --event manual`) | — |
| `api` | `name` (optional) | the local API/CLI invokes that entry point | — |

## Inbound validation (spec §16)

Raw datagrams never reach rule matching. They pass through `triggers::Inbound`:

* size limit (4 KiB), event-count limit per datagram, bounded DMX universe tracking;
* parsing by the `tpt-av-control` codecs (bounds-checked; bundle nesting limited);
* per-source token-bucket rate limiting — a flooding source is dropped *and counted*;
* any failure is `Error::MalformedMessage`, counted in `/health` (`rejected_inbound`) and logged at
  debug level. Nothing in this path can panic the engine; it is exercised with truncated, oversized
  and pseudo-random inputs in the test suite.

OSC arguments that are not numeric (strings, blobs) are skipped when building the event.

## Not yet implemented

Sunrise/sunset-relative schedules, control-surface button/fader events, cron expressions, and the
Phase 2 signal-condition and media-pipeline triggers (`signal_lost`, black/freeze frame, audio
silence/clipping, A/V drift, watch folders, job events, sibling-app events).
