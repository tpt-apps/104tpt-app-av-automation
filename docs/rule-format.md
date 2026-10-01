# Rule pack format (`format_version: 1`)

A rule pack is one YAML file: human-readable, versioned, diffable and importable. The CLI, the
service and the desktop application all read exactly this format.

```yaml
format_version: 1            # required; newer than this build understands = error
name: Main Hall              # required
description: optional text
revision: 3                  # bump when you edit the pack; stored in version history
action_library:              # optional reusable action fragments (see workflow.use_action)
  projector-on: { type: control.osc, address: /projector/1/power, args: [1] }
rules:
  - id: event-start          # required, unique, stable (used by the API, history and rule_armed)
    name: Main Hall - Event Start
    version: 2               # recorded in every execution
    armed: false             # default false: disarmed = simulate only
    priority: critical       # critical | normal (default) | low | informational
    tags: [show, lighting]
    reentrant: false
    trigger: { type: schedule, at: "18:55", days: [mon, tue, wed, thu, fri] }
    conditions:
      - { id: house-closed, type: time_window, from: "18:00", to: "23:00" }
    actions:
      - { id: house-to-half, type: control.dmx_scene, device: lighting-rack, scene: house-to-half, fade_ms: 3000 }
      - { id: projector-on,  type: control.osc, device: projector-1, address: /projector/1/power, args: [1] }
    policy:
      on_failure: continue_chain   # stop_chain (default) | continue_chain | run_fallback
      timeout_ms: 10000            # whole-chain budget
      fallback:                    # used by run_fallback
        - { id: escalate, type: notify.operator, message: "Manual intervention required", severity: critical }
```

## Rules the validator enforces

* Rule ids, condition ids and action ids are non-empty and unique within their scope.
* Every rule has a trigger and at least one action; `run_fallback` needs a `fallback` chain.
* A step that writes to hardware (`control.*`, `media.*`) must name a `device`.
* Numeric ranges: DMX universe/channel/value, MIDI channel/number/value, OSC address shape, schedule
  times, `time_window` (`from` before `to`).
* `workflow.invoke_rule` targets must exist and must not form a cycle; `workflow.use_action` targets
  must exist in `action_library` and must not reference another fragment.
* Chains are limited to 64 steps.
* Warnings (the pack still loads): no rules, older `format_version`, armed reentrant rules, step
  timeouts that exceed the chain budget, steps with an ignored `device`.

Device *existence* is checked separately against a device file (`validate --devices`), because packs
are portable and registries are per-site.

## Step fields common to every action

| Field | Meaning |
|-------|---------|
| `id` | unique within the chain; appears in execution records |
| `type` | the action type, e.g. `control.osc` |
| `device` | target device id (hardware writes only) |
| `timeout_ms` | per-step bound; defaults to what is left of the chain budget |
| `concurrent_safe` | declared for future parallel execution; must not follow a non-safe step |
| `estimated_cost_ms` | used to warn about chains that cannot fit their budget |

## Device file (`devices.yaml`)

```yaml
devices:
  - id: lighting-rack
    name: Lighting Rack            # defaults to the id
    kind: lighting                 # lighting | display | audio | switcher | media | other
    protocol: artnet               # osc | artnet | sacn | midi | virtual
    address: 127.0.0.1:6454        # host:port; for midi, part of the output port name
    heartbeat_ms: 15000            # optional deadline; silence beyond it = offline
    scenes:                        # named DMX scenes for control.dmx_scene
      house-to-half: { universe: 1, start_channel: 0, values: [128, 128, 128, 128] }
```

## Service config (`service.yaml`)

```yaml
state_db: ./state/automation.db    # omit for no persistence (or use run --state-dir)
utc_offset_minutes: 780            # site timezone for schedules and time windows
tick_ms: 250
heartbeat_interval_ms: 5000
listeners:                         # inbound control traffic
  - { protocol: osc,    bind: "0.0.0.0:9000" }
  - { protocol: artnet, bind: "0.0.0.0:6454" }
  - { protocol: midi,   bind: "USB MIDI" }   # part of a MIDI input port name
exec_allow: []                     # programs workflow.exec may run; empty = exec disabled
simulate: false                    # force simulation regardless of arming
api:
  enabled: false                   # off by default
  bind: "127.0.0.1:8787"           # loopback only; anything else is rejected
  token: "at-least-16-characters"  # required when enabled
```
