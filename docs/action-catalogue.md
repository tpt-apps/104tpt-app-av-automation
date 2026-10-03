# Action catalogue

Actions are executed by `actions::Actions::execute` and described by `Actions::simulate`. Each step
records an outcome of **success**, **failed**, **timed_out** or **skipped**.

| `type` | Key fields | Effect |
|--------|-----------|--------|
| `control.osc` | `device`, `address`, `args: [numbers]` | send an OSC message (integral numbers go as `i`, others as `f`) |
| `control.midi` | `device`, `channel`, `kind`, `number`, `value`, `group`, `value32`, `index` | send a MIDI message to the device's output. A `midi` device emits MIDI 1.0 (`note_on`, `note_off`, `cc`, `program_change`, 7-bit `value`). A `ump` device emits MIDI 2.0 Universal MIDI Packets over UDP and also accepts the MIDI 2.0-only kinds (`per_note_rcc`, `per_note_acc`, `rpn`, `nrpn`, `relative_rpn`, `relative_nrpn`, `per_note_pitch_bend`, `polyphonic_pressure`, `channel_pressure`, `pitch_bend`), which carry a 32-bit `value32`. `group` (0-15) selects the UMP port; `index` is the enumeration index for the kinds that carry one. Ranges are validated at load time, and a MIDI 1.0 port rejects `group`/`value32` rather than truncating them |
| `control.dmx_channels` | `device`, `universe`, `start_channel`, `values`, `transport` | write consecutive channels; merged into the endpoint's universe state and sent as a full frame. `transport` is `art_net`, `sacn` or `dmx512`, and must match the endpoint |
| `control.dmx_universe` | `device`, `universe`, `values`, `transport` | replace a whole universe |
| `control.dmx_scene` | `device`, `scene`, `fade_ms` | recall a scene from the device's scene table (`fade_ms` is recorded; fading is not implemented by the Art-Net/sACN endpoints) |
| `media.video_source` | `device`, `source`, `operation` | start/stop/switch a video source — see *Media* below |
| `media.audio_route` | `device`, `from`, `to`, `operation` | start/stop/switch an audio route — see *Media* below |
| `notify.operator` | `message`, `severity` | raise an operator alert (severities: `info`, `normal`, `high`, `critical`) |
| `log.incident` | `severity`, `message` | write to the persistent incident log (SQLite, `GET /incidents`) and the process log; message defaults to the rule name |
| `workflow.wait` | `ms` | pause the chain; interruptible by cancellation; bounded by the step timeout |
| `workflow.invoke_rule` | `rule` | run another rule as a nested execution (depth-limited, cycle-checked); its status becomes this step's outcome |
| `workflow.exec` | `program`, `args`, `timeout_ms` | run a sandboxed external program — see below |
| `workflow.use_action` | `library`, plus `device` etc. | instantiate a fragment from `action_library` |

## Failure policy and timeouts

`policy.on_failure` decides what a failed or timed-out step does:

* `stop_chain` (default) — remaining steps are recorded `skipped` with the reason;
* `continue_chain` — carry on;
* `run_fallback` — skip the remaining main steps and run `policy.fallback`.

The overall status folds every step: all ok → `success`; some ok and some failed → `partial_failure`
(a fallback that succeeds does **not** turn this back into success); none ok → `failed`; nothing
attempted → `skipped`.

A step's timeout is its own `timeout_ms`, else what remains of `policy.timeout_ms`. Endpoint sends
that exceed it are abandoned and recorded as `timed_out`.

## Device faults feed back into health

When a send fails because the device is unreachable or times out, a device the registry believed
`online` becomes `degraded` and a `device_state` event is emitted — so a failover rule can react.

## Sandboxed external programs (spec §8.4, §16)

`workflow.exec` is **disabled by default**. The operator enables it by listing exact program strings
in `exec_allow`. Then: no shell (argv vector, so metacharacters are inert); cleared environment;
closed stdin; discarded stdout/stderr; killed at the wall-clock bound; never elevated. A program
not on the allow-list, or a non-zero exit, is a `failed` step.

## Media

`media.*` actions are routed to the endpoint bound to `device`, exactly like control actions, and
simulate correctly. **No endpoint implements them yet**: the `tpt-kinetix` and `tpt-cadence`
repositories are codec/pipeline libraries without a source-control API to call, so the UDP and MIDI
endpoints reject these commands (`failed`, never silently "ok"). The `virtual` endpoint records them,
which is what the test suite uses. A kinetix-backed `Endpoint` implementation is the integration
point (`devices::Endpoint`).

## Not yet implemented

Control-surface feedback states, `media.transcode`/`proxy` jobs, file move/copy, local sound/visual
alerts, opt-in outbound webhook/email connectors.
