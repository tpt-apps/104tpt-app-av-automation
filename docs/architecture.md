# Architecture

The product centre of gravity is **Trigger → Condition → Action → Explain → Recover** (spec §23).
The deterministic rule core is separate from every shell that hosts it (spec §3.6): the CLI, the
headless service and the desktop application all drive the *same* `Engine`.

```text
                    CLI        Service (headless)        Desktop (Tauri)
                     │               │                          │
                     └───────────────┼──────────────────────────┘
                                     ▼
                                  Engine ───────────────▶ ExecutionRecord ─▶ store / API / UI
                  ┌──────────────────┼───────────────────┐
              Triggers           Conditions           Actions
           (model + triggers)    (conditions)      (actions + devices)
```

The desktop app embeds the headless service unchanged. Its webview renders JSON from the
`service::ui` bridge — the same status document the local API serves — and every edit (pack YAML,
rule builder, arm/disarm, simulation switch) is a control request to the engine thread, so it is
validated by exactly the code the CLI validates with. The renderer holds no business logic.

## Crate dependency direction

```text
core ◀── model ◀── triggers ─┐
  ▲        ▲                 ├──▶ engine ◀── service ◀── cli
  │        ├── conditions ───┤       ▲          ▲
  │        └── devices ◀── actions ──┘      report
                           └── tauri (excluded from the default workspace build)
```

* `model` is pure data + validation: no I/O, no clock, no network. It is where "what is a valid rule"
  is decided, once, for everyone.
* `engine` owns *no* I/O of its own. Time comes from an injected `Clock`; device I/O goes through
  `Actions` and its `Endpoint`s; sleeping goes through a `Sleeper`. That is what makes evaluation
  reproducible (spec §3.2) and testable against virtual devices.
* `service` runs the engine on **one thread**. Listeners, the API and the operator talk to it through
  a bounded queue. This removes a class of concurrency bugs; the only cross-thread operation is
  cancellation (`CancelHandle`), which is deliberately lock-light.

## Event flow

1. A source produces a normalized `Event`: the `Scheduler` (time), `Inbound` (validated OSC/MIDI/
   Art-Net/sACN), the `DeviceRegistry` (health, heartbeats), an operator, or the API.
2. `Engine::handle_event` **ingests** it (updates DMX/parameter/health state) then **matches** every
   rule in `(priority, rule id)` order.
3. **Conditions** are all evaluated (no short-circuit, so the record explains every one). `Unknown`
   blocks the rule unless that condition opted in with `fire_on_unknown`.
4. **Conflict resolution** per device slot, deterministic (see below).
5. The **chain** runs — `Actions::execute` if the rule is armed and the engine is live,
   `Actions::simulate` otherwise — applying the failure policy, per-step and chain timeouts, and
   cancellation.
6. An `ExecutionRecord` is produced, persisted, and streamed.

Side effects can cascade: a device that fails mid-chain becomes `Degraded`, which emits a
`device_state` event that can trigger a failover rule. The cascade is bounded (`max_cascade`).

## Simulation is structural, not a flag check

`Actions::simulate` has no access to endpoints. A disarmed rule, a rule in `Mode::Simulation`, and
every `simulate` CLI invocation go through it. There is no code path from simulation to a socket.
The CLI `simulate` command additionally builds its engine with *no endpoints bound at all*.

## Conflict resolution (spec §10.1)

Rules matched by one event run in priority order (`critical` first), ties broken by rule id. The first
rule to write a *slot* of a device owns it for that dispatch; a later rule writing a **different**
value to an overlapping slot has that step recorded as `Skipped` with the owner named. Slots:

| Command | Slot |
|---------|------|
| OSC | the address |
| MIDI | channel + kind + number |
| DMX channels | each channel of the universe |
| DMX universe | the whole universe |
| DMX scene | the whole device's DMX |
| video source | the device's source |
| audio route | the destination |

Identical writes never conflict. A disarmed (simulated) rule never blocks an armed one, but a
simulation still shows the conflict that arming would cause.

## Persistence (spec §15)

SQLite (WAL mode): rule pack versions (deduplicated), the device registry, execution history
(pruned), preferences, and the engine restart state. Raw protocol payloads are never stored.
Device health is deliberately **not** persisted: after a restart each device must prove itself with a
heartbeat.
