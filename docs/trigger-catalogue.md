# Trigger catalogue

Every trigger is matched by a pure function of `(trigger, event)` in `model::Trigger::matches`; it
never reads a clock or a device. Each one can run against a virtual source (spec §6.2).

| `type` | Fields | Fires when | Foundation crate |
|--------|--------|-----------|------------------|
| `schedule` | exactly one of `at: "HH:MM"` (+ `days`, `once`), `cron` (five-field expression, + `once`) or `interval_ms` | the local minute matches (once per minute however often polled); a `once` schedule fires a single time ever; a cron schedule fires on every minute its expression matches; an interval fires every N ms, never as a catch-up burst | — |
| `osc` | `address` (`*` = one path segment), `arg_equals`, `min_args` | a matching OSC message arrives | `tpt-av-control-osc` |
| `midi` | `message` (15 kinds, see below), `channel`, `number`, `value` (full 32-bit for MIDI 2.0), `group` (UMP port group 0-15) | a matching MIDI 1.0 or MIDI 2.0/UMP message arrives; omitted fields match anything. Kinds after `program_change` are MIDI 2.0-only and can only match a UMP source | `tpt-av-control-midi` |
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
* adaptive backoff — a source dropped `backoff_threshold` times in a row (default 3) is muted for
  a doubling window (250 ms → 5 s) and its packets are rejected before they are even parsed. A
  source that recovers is forgiven: a refilled bucket or an accepted event clears its strikes and
  level. `backed_off` and `backoff_episodes` are reported in `/health` under `rejected_inbound`;
* any failure is `Error::MalformedMessage`, counted in `/health` (`rejected_inbound`) and logged at
  debug level. Nothing in this path can panic the engine; it is exercised with truncated, oversized
  and pseudo-random inputs in the test suite.

OSC arguments that are not numeric (strings, blobs) are skipped when building the event.

## Not yet implemented

Sunrise/sunset-relative schedules, control-surface button/fader events, and the
Phase 2 signal-condition and media-pipeline triggers (`signal_lost`, black/freeze frame, audio
silence/clipping, A/V drift, watch folders, job events, sibling-app events).

MIDI 2.0 arrives as raw Universal MIDI Packets on a `ump` UDP listener rather than from a local
`midi` port, because no desktop platform currently exposes native MIDI 2.0 ports to `midir`. The
kinds after `program_change` in the table above therefore only ever match a `ump` source. Serial
DMX512 is output-only: a `dmx512` device writes frames to a serial port, but a rule cannot trigger
on DMX read back from that port.
