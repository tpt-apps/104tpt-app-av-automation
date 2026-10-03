# Reliability

Live productions and unattended installations cannot tolerate silent crashes (spec §11). This is what
the software does about it, and what is verified.

## Watchdog

`tpt-av-automation watchdog -- run --service …` supervises the engine process:

* restarts it when it exits with a non-zero status or is killed, with exponential backoff
  (250 ms → 5 s);
* exits when the engine exits `0` (an intentional stop);
* gives up with exit code 6 when the engine crash-loops (more than `--max-restarts` inside
  `--window-secs`), so a broken configuration cannot spin forever;
* carries no automation logic of its own.

**Verified:** `the_watchdog_restarts_a_forcibly_killed_engine` force-kills the engine process twice
and checks a new process comes up, serves the API and has reloaded its rules.

## State across restarts

The engine persists, to SQLite, its scheduler memory, armed state and execution counter (on every
batch of executions and at least every 5 s, and again on clean shutdown). A schedule firing is
written **before** its actions run (a write-ahead hook), so a hard kill in the middle of a chain
cannot make a one-shot fire again after the restart: one-shots are at-most-once. After a restart:

* a **one-shot** schedule that already fired does not fire again (even within the same minute);
* a recurring schedule does not fire twice in the same minute;
* armed/disarmed state is restored;
* execution ids continue.

Firings missed while the engine was down are **not** replayed — replaying a cue late is usually worse
than skipping it.

Device health is not restored: each device re-proves itself with a heartbeat.

**Verified:** `a_one_shot_schedule_does_not_refire_after_a_restart` (service level, real SQLite file)
and `one_shot_schedules_do_not_refire_after_a_restart` (engine level).

## Device health and heartbeats

* Devices with `heartbeat_ms` go `offline` when silent for longer than that; the transition emits a
  `heartbeat_missed` and a `device_state` event, each of which can trigger rules.
* Network devices (OSC, Art-Net, sACN) are connectionless and cannot be pinged, so their heartbeat is
  *any valid datagram received from the device's address*. `virtual` and `midi` endpoints are probed.
* A device that fails mid-chain becomes `degraded`.
* A device that has never been heard from stays `unknown` (there is no baseline to have missed).

## Failover is an ordinary rule

There is no hard-coded failover. [`rules/failover/main-display-failover.yaml`](../rules/failover/main-display-failover.yaml)
is a rule like any other: visible, testable in simulation, versioned.

## Failure isolation

* The engine runs on one thread; action panics are caught and recorded as a failed step.
* Persistence failures are logged and never stop rule execution.
* Inbound traffic is validated, size- and rate-limited and cannot panic the parser (see the
  [trigger catalogue](trigger-catalogue.md)); the event queue is bounded and overflow is counted.
* Event cascades are capped, and rule invocation depth is capped and cycle-checked.
* The local API enforces size/time limits and a connection cap, and never panics on malformed HTTP.
* Every shared lock in the engine recovers from poisoning rather than propagating it. This matters
  most for the inbound rate limiter, which runs on every datagram of every protocol listener: a
  panic there would take that listener down, and because the process stays up the watchdog would not
  notice.
* A listener or API thread that dies is counted, not forgotten. `/health` reports
  `listeners: {expected, live, lost}` and reports `degraded` when one is lost, so a protocol that
  has silently stopped being received cannot hide behind every device looking healthy.

## Measured throughput

`sustained_event_load_is_processed_and_history_stays_bounded` (in `crates/tpt-app-av-automation-test/tests/chaos.rs`)
drives 20 000 OSC events through a pack of 20 rules, half of which match (10 000 executions, each a
real endpoint write plus an alert), on one thread, and checks that history stays capped. On the
development machine a **release** build handled roughly 265 000 events/s (≈ 130 000
executions/s), i.e. well under 10 µs per event. Run it yourself:

```sh
cargo test --release -p tpt-app-av-automation-test --test chaos sustained -- --nocapture
```

This measures the engine, not real devices: network round trips and `workflow.wait` dominate in
practice.

## Measured latency

Throughput says nothing about when a cue *lands*. `crates/tpt-app-av-automation-test/tests/latency.rs`
measures latency as percentiles — an average hides exactly the slow tail that makes a show miss a
cue — and gates on p99 against a 20 ms cue budget. Run it against an optimized build:

```sh
cargo test --release -p tpt-app-av-automation-test --test latency -- --nocapture
```

| What is measured | p50 | p90 | p99 | max |
|---|---|---|---|---|
| Engine dispatch, 20 rules, in-memory device | 2 µs | 7 µs | 13 µs | 55 µs |
| …same, event matching no rule | 0 µs | 0 µs | 1 µs | 9 µs |
| …same, 100 rules | 14 µs | 17 µs | 23 µs | 92 µs |
| …same, 500 rules | 62 µs | 68 µs | 91 µs | 269 µs |
| OSC datagram in → OSC datagram out, loopback sockets | 17 µs | 27 µs | 81 µs | 300 µs |

Release build on the development machine; absolute numbers will differ elsewhere, the shape will not.

Two things worth reading off that table:

* **The rule scan is the only thing that scales with the pack.** 5→500 rules moves the median from
  3 µs to 62 µs — linear in pack size, as expected, and still two orders of magnitude inside the
  budget. `dispatch_latency_scales_with_pack_size` fails if it stops being linear-ish, so a future
  change to matching cannot quietly turn a large pack into a slow one.
* **An unmatched event costs almost nothing** (p99 1 µs). The engine rejects it during matching and
  never enters action execution, so an idle rule set costs a venue nothing while traffic flows.

The last row is the closest thing to a device measurement available without hardware: a real OSC
datagram crosses one loopback socket into a live listener thread, is parsed by the production
`Inbound` gate (size, event-count and rate-limit checks included), matched, executed, and leaves
through a second real socket. It times everything the software controls, including the encode and the
syscalls, and it asserts every cue arrived intact and in order. It still measures loopback, not a
fixture: a physical device's own latency, a DMX fixture's settling time and a MIDI port's buffering
are outside it and can only be measured on site.

## Known limits

* Conflict resolution covers rules matched by the *same* event on the engine thread. Chains do not
  run concurrently, so two chains cannot interleave writes to one device.
* `workflow.wait` blocks the engine thread for its duration (cancellation interrupts it). Long waits
  in a chain delay other rules; keep waits short or split the chain.
* Schedules use a fixed UTC offset (`utc_offset_minutes`); there is no timezone database, so daylight
  saving changes need a config update.
