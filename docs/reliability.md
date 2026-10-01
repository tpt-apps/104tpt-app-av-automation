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
practice. No profiler run has been done.

## Known limits

* Conflict resolution covers rules matched by the *same* event on the engine thread. Chains do not
  run concurrently, so two chains cannot interleave writes to one device.
* `workflow.wait` blocks the engine thread for its duration (cancellation interrupts it). Long waits
  in a chain delay other rules; keep waits short or split the chain.
* Schedules use a fixed UTC offset (`utc_offset_minutes`); there is no timezone database, so daylight
  saving changes need a config update.
