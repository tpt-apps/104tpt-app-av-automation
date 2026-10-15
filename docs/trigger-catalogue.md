# Trigger catalogue

Every trigger is matched by a pure function of `(trigger, event)` in `model::Trigger::matches`; it
never reads a clock or a device. Each one can run against a virtual source (spec §6.2).

| `type` | Fields | Fires when | Foundation crate |
|--------|--------|-----------|------------------|
| `schedule` | exactly one of `at: "HH:MM"` (+ `days`, `once`), `cron` (five-field expression, + `once`), `solar` (`{event: sunrise\|sunset, minutes: N}`, negative = before) or `interval_ms` | the local minute matches (once per minute however often polled); a `once` schedule fires a single time ever; a cron schedule fires on every minute its expression matches; a `solar` schedule fires at that day's sunrise/sunset plus the offset, following the sun as it moves; an interval fires every N ms, never as a catch-up burst | — |
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

## Media-pipeline triggers (spec §7.5)

`file_arrived` fires when a file appears in a watched folder. The service polls each folder named by
such a trigger every `watch_interval_ms` (default 1000) with plain file-system reads, so it needs no
platform notification backend. Two rules keep it honest:

* the first scan of a folder only records what is already there, so starting the service never
  fires for old files;
* a file is reported once its size is the same on two consecutive scans, so one still being copied
  is not handed to the rule half-written.

```yaml
trigger: { type: file_arrived, folder: '/media/ingest', extensions: [mov, mxf] }
```

`extensions` is optional, case-insensitive and written without a dot. Files that arrive while the
service is stopped are not picked up later. This polls with `std::fs`; it is **not** the
`tpt-av-asset` watcher.

`job` fires when a producer reports that a background job finished. Producers (TPT Media QC,
TPT Media Forensics, a transcode pipeline) post to `POST /events/job` on the local API (see
[integrations](integrations.md#local-api-spec-14)). The trigger matches on producer, outcome and an
optional category:

```yaml
trigger: { type: job, source: qc, outcome: failed, job_kind: transcode }
```

`source` and `outcome` are optional; leaving `outcome` out matches both. `simulate` accepts
`file:/path/clip.mov` and `job:qc:failed[:kind]` for testing.

## Signal triggers (spec §7.4)

`signal` fires on a measurement a producer reports to `POST /events/signal`. The engine never
measures audio or video itself: a producer that already has the signal (an audio desk, a playout
system, a monitoring probe) does the analysis and reports the result. That keeps the rule engine
free of capture and analysis dependencies. `tpt-visual`, `tpt-dsp` or `tpt-av-sync` can be the
producer, but that is not wired up in this repository.

| `signal` | Carries a value | Meaning |
|----------|-----------------|---------|
| `lost` · `restored` | no | the signal disappeared or came back |
| `silence` · `clipping` | no | audio is silent or clipping |
| `black` · `freeze` | no | video is black or frozen |
| `level` | dBFS | audio level, tested with `comparison` and `value` |
| `drift` | ms | audio/video drift, tested with `comparison` and `value` |

```yaml
trigger: { type: signal, source: audio-desk, channel: main-L, signal: level, comparison: lt, value: -20 }
trigger: { type: signal, source: playout, signal: black }
```

`source` and `channel` are optional filters. A yes/no kind must not have a `comparison` or `value`,
and `level` and `drift` must have both. For testing, `simulate` accepts `signal:audio-desk:level=-25`
or `signal:playout:black`.

## Not yet implemented

Control-surface button/fader events. The media-pipeline triggers are **not** wired to `tpt-av-asset`
yet: job results arrive from producers over the API, and watch folders are polled directly.

MIDI 2.0 arrives as raw Universal MIDI Packets on a `ump` UDP listener rather than from a local
`midi` port, because no desktop platform currently exposes native MIDI 2.0 ports to `midir`. The
kinds after `program_change` in the table above therefore only ever match a `ump` source.

A `solar` schedule needs the pack to declare a `site`, because a solar crossing cannot be computed
without knowing where on the Earth the venue is:

```yaml
site:
  latitude: 51.5074
  longitude: -0.1278
  # elevation_deg defaults to -0.833 (sunrise/sunset proper); -6 gives civil twilight
rules:
  - id: exterior-lights
    trigger:
      type: schedule
      solar: { event: sunset, minutes: -30 }
```

Above the polar circles the sun does not cross on some days; such a schedule simply does not fire
that day rather than being given an invented time.

Serial DMX512 works in both directions: a `dmx512` device writes frames to a serial port, and a
`dmx512` service listener reads one back, recovering frame boundaries from inter-byte timing and
diffing each frame into the same channel-change events Art-Net and sACN produce.

