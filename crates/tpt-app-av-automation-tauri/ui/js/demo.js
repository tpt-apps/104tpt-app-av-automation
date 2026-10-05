// Browser-preview fixtures for the TPT AV Automation UI.
//
// When the UI runs inside Tauri, every command goes to the embedded service through
// `window.__TAURI__.core.invoke`. When it is opened in a plain browser (no Tauri global), this
// module answers instead, with the same JSON shapes the real bridge returns — so the screens can
// be laid out and screenshot-tested without a webview. It never runs in the packaged app: the
// transport in app.js only falls back to it when the Tauri global is absent.

(() => {
  const now = Date.now();
  const min = 60_000;

  const devices = [
    { id: "projector-1", name: "Projector, house", kind: "video", protocol: "osc", address: "192.168.10.21:8000", health: "online", last_seen_ms: now - 4_000 },
    { id: "switcher-1", name: "Vision switcher", kind: "video", protocol: "artnet", address: "192.168.10.30:6454", health: "online", last_seen_ms: now - 2_000 },
    { id: "house-lights", name: "House lighting", kind: "lighting", protocol: "sacn", address: "192.168.10.40:5568", health: "degraded", last_seen_ms: now - 48_000 },
    { id: "main-pa", name: "Main PA processor", kind: "audio", protocol: "osc", address: "192.168.10.55:8001", health: "unknown", last_seen_ms: null },
    { id: "backup-pa", name: "Backup PA processor", kind: "audio", protocol: "osc", address: "192.168.10.56:8001", health: "online", last_seen_ms: now - 6_000 },
    { id: "show-cue-midi", name: "Show cue MIDI", kind: "audio", protocol: "midi", address: "MOTU Midi Express", health: "online", last_seen_ms: now - 1_000 },
  ];

  const pack = {
    format_version: 1,
    name: "Riverside Sessions — Main Hall",
    revision: 7,
    rules: [
      {
        id: "main-display-failover",
        name: "Main display failover",
        version: 2,
        description: "If the projector stops reporting, cut the backup feed.",
        armed: true,
        priority: "critical",
        tags: ["failover", "video"],
        trigger: { type: "device_state", device: "projector-1", state: "offline" },
        conditions: [
          { id: "c1", description: "during show hours", type: "time_window", from: "18:00", to: "23:30" },
        ],
        actions: [
          { id: "a1", type: "media.video_source", source: "display-1-input", operation: "switch" },
          { id: "a2", type: "control.osc", device: "switcher-1", address: "/input/main", args: [2] },
          { id: "a3", type: "notify.operator", message: "Main display lost signal; switched to backup", severity: "high" },
          { id: "a4", type: "log.incident", severity: "high", message: "Main display failover ran" },
        ],
        policy: { on_failure: "continue" },
      },
      {
        id: "house-to-half",
        name: "House to half at doors",
        version: 1,
        armed: true,
        priority: "normal",
        tags: ["opening"],
        trigger: { type: "schedule", at: "18:55", days: ["mon", "tue", "wed", "thu", "fri", "sat"] },
        actions: [
          { id: "a1", type: "control.dmx_scene", device: "house-lights", scene: "house-to-half", fade_ms: 5000 },
        ],
      },
      {
        id: "sunset-warmup",
        name: "Sunset warm-up",
        version: 3,
        armed: false,
        priority: "low",
        trigger: { type: "schedule", solar: { event: "sunset", minutes: -30 } },
        actions: [
          { id: "a1", type: "notify.operator", message: "Sunset in 30 minutes", severity: "info" },
        ],
      },
      {
        id: "foh-midi-go",
        name: "FOH MIDI go",
        version: 1,
        armed: true,
        priority: "high",
        trigger: { type: "midi", message: "note_on", channel: 0, number: 60 },
        conditions: [],
        actions: [
          { id: "a1", type: "workflow.invoke_rule", rule: "main-display-failover" },
        ],
      },
      {
        id: "amp-overheat",
        name: "Amp overheat watch",
        version: 1,
        armed: true,
        priority: "emergency",
        trigger: { type: "device_parameter", device: "main-pa", parameter: "temperature", comparison: "gt", value: 70 },
        actions: [
          { id: "a1", type: "media.audio_route", from: "main-pa", to: "backup-pa", operation: "start" },
          { id: "a2", type: "notify.operator", message: "Main PA overheating; routed to backup", severity: "critical" },
        ],
      },
    ],
  };

  const executions = [
    {
      execution_id: "e-1031", rule_id: "house-to-half", rule_name: "House to half at doors",
      rule_version: 1, triggered_at_ms: now - 21 * min, trigger_type: "schedule",
      trigger_detail: { type: "schedule", at: "18:55" },
      condition_results: [],
      action_results: [
        { id: "a1", status: "success", duration_ms: 5034, device: "house-lights" },
      ],
      overall_status: "success", simulated: false, duration_ms: 5102,
    },
    {
      execution_id: "e-1032", rule_id: "main-display-failover", rule_name: "Main display failover",
      rule_version: 2, triggered_at_ms: now - 9 * min, trigger_type: "device_state",
      trigger_detail: { type: "device_state", device: "projector-1", state: "offline" },
      condition_results: [
        { id: "c1", description: "during show hours", passed: true },
      ],
      action_results: [
        { id: "a1", status: "success", duration_ms: 41 },
        { id: "a2", status: "success", duration_ms: 6 },
        { id: "a3", status: "success", duration_ms: 1 },
        { id: "a4", status: "failure", duration_ms: 2, error: "incident sink unreachable" },
      ],
      overall_status: "partial", simulated: false, duration_ms: 63,
    },
    {
      execution_id: "e-1033", rule_id: "sunset-warmup", rule_name: "Sunset warm-up",
      rule_version: 3, triggered_at_ms: now - 3 * min, trigger_type: "schedule",
      trigger_detail: { type: "schedule", solar: { event: "sunset", minutes: -30 } },
      condition_results: [],
      action_results: [
        { id: "a1", status: "success", duration_ms: 1 },
      ],
      overall_status: "success", simulated: true, duration_ms: 2,
    },
  ];

  const incidents = [
    { at_ms: now - 9 * min, rule_id: "main-display-failover", execution_id: "e-1032", severity: "high", message: "Main display lost signal; failover chain ran" },
    { at_ms: now - 52 * min, rule_id: "amp-overheat", execution_id: "e-1029", severity: "critical", message: "Main PA at 78 C" },
  ];

  const history = [
    { id: 12, name: "Riverside Sessions — Main Hall", revision: 7, saved_at_ms: now - 30 * min },
    { id: 11, name: "Riverside Sessions — Main Hall", revision: 6, saved_at_ms: now - (26 * 60 + 12) * 1000 },
    { id: 10, name: "Riverside Sessions — Main Hall", revision: 5, saved_at_ms: now - (5 * 3600 + 3 * 60) * 1000 },
  ];

  const catalogue = [
    {
      kind: "trigger", label: "Triggers",
      entries: [
        { kind: "schedule", label: "Schedule (fixed time, cron, interval or solar)", example: { type: "schedule", at: "18:55", days: ["mon", "tue", "wed", "thu", "fri"] } },
        { kind: "osc", label: "OSC message", example: { type: "osc", address: "/cue/go", arg_equals: 1 } },
        { kind: "midi", label: "MIDI message (1.0 or 2.0/UMP)", example: { type: "midi", message: "note_on", channel: 0, number: 60 } },
        { kind: "dmx", label: "DMX channel change", example: { type: "dmx", universe: 1, channel: 0, comparison: "crossed_above", value: 128 } },
        { kind: "device_state", label: "Device health transition", example: { type: "device_state", device: "projector-1", state: "offline" } },
        { kind: "heartbeat_missed", label: "Heartbeat missed", example: { type: "heartbeat_missed", device: "switcher-1", after_ms: 5000 } },
        { kind: "device_parameter", label: "Device parameter change", example: { type: "device_parameter", device: "amp-1", parameter: "temperature", comparison: "gt", value: 60 } },
        { kind: "manual", label: "Operator \"run now\"", example: { type: "manual" } },
        { kind: "api", label: "API / CLI invocation", example: { type: "api", name: "panic-run" } },
      ],
    },
    {
      kind: "condition", label: "Conditions",
      entries: [
        { kind: "device_health", label: "Device health is", example: { type: "device_health", device: "projector-1", equals: "online" } },
        { kind: "time_window", label: "Time of day inside window", example: { type: "time_window", from: "09:00", to: "17:00" } },
        { kind: "dmx_channel", label: "DMX channel comparison", example: { type: "dmx_channel", universe: 1, channel: 10, comparison: "gt", value: 128 } },
        { kind: "device_parameter", label: "Device parameter comparison", example: { type: "device_parameter", device: "amp-1", parameter: "temperature", comparison: "lt", value: 80 } },
        { kind: "rule_armed", label: "Another rule is armed", example: { type: "rule_armed", rule: "other-rule" } },
        { kind: "rule_has_tag", label: "Rule has tag", example: { type: "rule_has_tag", tag: "opening" } },
      ],
    },
    {
      kind: "action", label: "Actions",
      entries: [
        { kind: "control.osc", label: "Send OSC", example: { type: "control.osc", address: "/projector/1/power", args: [1] } },
        { kind: "control.midi", label: "Send MIDI (1.0 or 2.0/UMP)", example: { type: "control.midi", channel: 0, kind: "cc", number: 1, value: 64 } },
        { kind: "control.dmx_channels", label: "Set DMX channels", example: { type: "control.dmx_channels", universe: 1, start_channel: 0, values: [255, 255, 0] } },
        { kind: "control.dmx_scene", label: "Recall DMX scene", example: { type: "control.dmx_scene", scene: "house-to-half", fade_ms: 2000 } },
        { kind: "control.dmx_universe", label: "Send DMX universe snapshot", example: { type: "control.dmx_universe", universe: 1, values: Array(512).fill(0) } },
        { kind: "media.video_source", label: "Video source start/stop/switch", example: { type: "media.video_source", source: "display-1-input", operation: "switch" } },
        { kind: "media.audio_route", label: "Audio route start/stop", example: { type: "media.audio_route", from: "main-pa", to: "backup-pa", operation: "start" } },
        { kind: "notify.operator", label: "Notify operator", example: { type: "notify.operator", message: "Main display lost signal", severity: "high" } },
        { kind: "log.incident", label: "Log incident", example: { type: "log.incident", severity: "high", message: "Failover ran" } },
        { kind: "workflow.wait", label: "Wait", example: { type: "workflow.wait", ms: 1000 } },
        { kind: "workflow.invoke_rule", label: "Invoke another rule", example: { type: "workflow.invoke_rule", rule: "other-rule" } },
        { kind: "workflow.exec", label: "Run external program (sandboxed)", example: { type: "workflow.exec", program: "C:/tools/report.exe", args: [], timeout_ms: 5000 } },
        { kind: "workflow.use_action", label: "Use action-library fragment", example: { type: "workflow.use_action", library: "key" } },
      ],
    },
  ];

  const dashboard = () => {
    const counts = (h) => devices.filter((d) => d.health === h).length;
    const lostListeners = 0;
    const status = lostListeners > 0 ? "degraded"
      : counts("offline") > 0 ? "offline"
        : counts("degraded") > 0 ? "degraded" : "nominal";
    return {
      status: {
        status,
        pack: pack.name,
        mode: state.simulation ? "simulation" : "live",
        rules: pack.rules.length,
        armed_rules: pack.rules.filter((r) => r.armed).length,
        listeners: { expected: 3, live: 3, lost: lostListeners },
        devices: {
          total: devices.length,
          online: counts("online"),
          degraded: counts("degraded"),
          offline: counts("offline"),
          unknown: counts("unknown"),
        },
        active_chains: state.chains.length,
        rejected_inbound: { malformed: 0, rate_limited: 0, backed_off: 0, backoff_episodes: 0, queue_dropped: 0 },
      },
      rules: pack.rules.map((r) => ({
        id: r.id, name: r.name, version: r.version ?? 1, armed: !!r.armed,
        priority: r.priority ?? "normal", trigger: r.trigger?.type ?? "", tags: r.tags ?? [],
      })),
      devices,
      active_chains: state.chains,
      recent_executions: [...executions].reverse().slice(0, 20),
    };
  };

  const state = { simulation: false, chains: [] };

  const findRule = (id) => pack.rules.find((r) => r.id === id);

  const handlers = {
    dashboard,
    devices: () => ({ devices }),
    app_info: () => ({ version: "0.1.0", pack_file: "rules/main-hall.yaml" }),
    catalogue: () => catalogue,
    pack_json: () => JSON.parse(JSON.stringify(pack)),
    pack_yaml: () =>
      "# Browser preview fixture — the real text comes from the engine.\n" +
      pack.rules
        .map((r) => `  - id: ${r.id}\n    name: ${r.name}\n    trigger: { type: ${r.trigger.type} }\n`)
        .join(""),
    validate_pack: (args) =>
      String(args.yaml || "").includes("bogus")
        ? {
          valid: false,
          diagnostics: [
            { severity: "error", location: "rules[2].actions[0].device", message: "device `ghost` is not in the device registry" },
            { severity: "warning", location: "rules[2].version", message: "rule edits should raise the version so they reload disarmed" },
          ],
        }
        : { valid: true, name: pack.name, revision: pack.revision, rules: pack.rules.length },
    apply_pack: (args) => {
      const check = handlers.validate_pack(args);
      return check;
    },
    save_pack_file: () => ({ saved: "rules/main-hall.yaml" }),
    executions: (args) => {
      let rows = executions.filter((e) =>
        (!args.rule || e.rule_id === args.rule) &&
        (!args.status || e.overall_status === args.status) &&
        (!args.device || (e.action_results ?? []).some((a) => a.device === args.device)) &&
        (!args.since || e.triggered_at_ms >= args.since) &&
        (!args.until || e.triggered_at_ms < args.until));
      if (args.simulated === false) rows = rows.filter((e) => !e.simulated);
      if (args.simulated === true) rows = rows.filter((e) => e.simulated);
      return { executions: rows };
    },
    incidents: () => incidents.slice(),
    pack_history: () => history,
    restore_pack_version: () => ({ valid: true, name: pack.name, revision: 5, rules: pack.rules.length }),
    arm_rule: (args) => { const r = findRule(args.id); if (!r) throw `unknown rule \`${args.id}\``; r.armed = true; return { rule: args.id, armed: true }; },
    disarm_rule: (args) => { const r = findRule(args.id); if (!r) throw `unknown rule \`${args.id}\``; r.armed = false; return { rule: args.id, armed: false }; },
    run_rule: (args) => {
      const r = findRule(args.id);
      if (!r) throw `unknown rule \`${args.id}\``;
      return { executions: [{ execution_id: `e-${1000 + executions.length}`, rule_id: r.id, overall_status: "success", simulated: !r.armed || state.simulation }] };
    },
    cancel_execution: (args) => {
      const index = state.chains.findIndex((c) => c.execution_id === args.id);
      if (index < 0) throw `no running execution \`${args.id}\``;
      state.chains.splice(index, 1);
      return { execution: args.id, cancelled: true };
    },
    set_simulation: (args) => { state.simulation = args.on; return { simulation: args.on }; },
    ping_device: (args) => {
      const d = devices.find((x) => x.id === args.id);
      if (!d) throw `unknown device \`${args.id}\``;
      return { device: d.id, reachable: d.health !== "offline" };
    },
    upsert_rule: (args) => {
      const rule = JSON.parse(JSON.stringify(args.rule));
      if (!rule.id) rule.id = rule.name.toLowerCase().replace(/[^a-z0-9]+/g, "-");
      const index = pack.rules.findIndex((r) => r.id === rule.id);
      if (index >= 0) pack.rules[index] = rule; else pack.rules.push(rule);
      return { valid: true, name: pack.name, revision: pack.revision, rules: pack.rules.length };
    },
    remove_rule: (args) => {
      const index = pack.rules.findIndex((r) => r.id === args.id);
      if (index < 0) throw `unknown rule \`${args.id}\``;
      pack.rules.splice(index, 1);
      return { valid: true, rules: pack.rules.length };
    },
  };

  /** Fallback transport: answers `invoke` from the fixtures above. */
  window.TptDemo = {
    active: () => typeof window.__TAURI__ === "undefined",
    async call(command, args = {}) {
      await new Promise((resolve) => setTimeout(resolve, 40));
      const handler = handlers[command];
      if (!handler) throw `no demo handler for \`${command}\``;
      return handler(args);
    },
    devices,
    state,
  };
})();
