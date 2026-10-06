// Pure display/assembly helpers for the TPT AV Automation UI.
//
// No DOM, no transport: this file is loaded both by the webview (as a global) and by the
// `node --test` suite in ui/test/, so formatting and rule assembly are covered by automated
// tests even though the renderer itself needs a webview.

const TptFormat = (() => {
  const WEEKDAYS = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"];

  // Enum fields, keyed by the YAML field name. Values mirror the model's serde spellings;
  // anything unknown stays a free-text field and the engine validation reports it.
  const OPTIONS = {
    comparison: ["eq", "ne", "gt", "gte", "lt", "lte"],
    state: ["online", "degraded", "offline", "unknown"],
    equals: ["online", "degraded", "offline", "unknown"],
    operation: ["start", "stop", "switch"],
    severity: ["info", "normal", "high", "critical"],
    transport: ["art_net", "sacn"],
    days: WEEKDAYS,
    // MIDI trigger message kinds (MidiMessageKind, snake_case) and the action `kind` labels.
    message: [
      "note_on", "note_off", "control_change", "program_change", "channel_pressure",
      "polyphonic_pressure", "pitch_bend", "rpn", "nrpn", "relative_rpn", "relative_nrpn",
      "per_note_rcc", "per_note_acc",
    ],
    kind: [
      "note_on", "note_off", "cc", "program_change", "pitch_bend", "rpn", "nrpn",
      "relative_rpn", "relative_nrpn", "per_note_rcc", "per_note_acc",
      "channel_pressure", "polyphonic_pressure",
    ],
  };

  const pad2 = (n) => String(n).padStart(2, "0");

  /** Epoch ms → local `HH:MM:SS`. */
  function fmtClock(ms) {
    if (ms === null || ms === undefined || Number.isNaN(ms)) return "—";
    const d = new Date(ms);
    return `${pad2(d.getHours())}:${pad2(d.getMinutes())}:${pad2(d.getSeconds())}`;
  }

  /** Epoch ms → local `YYYY-MM-DD HH:MM:SS`. */
  function fmtDateTime(ms) {
    if (ms === null || ms === undefined || Number.isNaN(ms)) return "—";
    const d = new Date(ms);
    return `${d.getFullYear()}-${pad2(d.getMonth() + 1)}-${pad2(d.getDate())} ` +
      `${pad2(d.getHours())}:${pad2(d.getMinutes())}:${pad2(d.getSeconds())}`;
  }

  /** Epoch ms → "12s ago" relative to `now` (epoch ms). */
  function fmtAgo(ms, now) {
    if (ms === null || ms === undefined || Number.isNaN(ms)) return "never";
    const seconds = Math.max(0, Math.round((now - ms) / 1000));
    if (seconds < 60) return `${seconds}s ago`;
    const minutes = Math.round(seconds / 60);
    if (minutes < 60) return `${minutes}m ago`;
    const hours = Math.round(minutes / 60);
    if (hours < 24) return `${hours}h ago`;
    return `${Math.round(hours / 24)}d ago`;
  }

  /** Milliseconds → compact duration ("340 ms", "1.2 s", "2.0 s"). */
  function fmtDuration(ms) {
    if (ms === null || ms === undefined || Number.isNaN(ms)) return "—";
    if (ms < 1000) return `${Math.round(ms)} ms`;
    return `${(ms / 1000).toFixed(1)} s`;
  }

  /** HTML-escapes text interpolated into markup. */
  function esc(text) {
    return String(text ?? "")
      .replaceAll("&", "&amp;")
      .replaceAll("<", "&lt;")
      .replaceAll(">", "&gt;")
      .replaceAll('"', "&quot;")
      .replaceAll("'", "&#39;");
  }

  /** CSS chip class for an execution status. */
  function statusClass(status) {
    switch (status) {
      case "success": return "chip-ok";
      case "partial": return "chip-warn";
      case "failure":
      case "timed_out": return "chip-fail";
      case "cancelled": return "";
      default: return "";
    }
  }

  /** CSS dot class for a device health label. */
  function healthClass(health) {
    const known = ["online", "degraded", "offline", "unknown"];
    return known.includes(health) ? health : "unknown";
  }

  /**
   * Describes the input a field should render as.
   * Returns `{ input, value, options? }` where input is one of
   * text | number | checkbox | list | select.
   */
  function fieldSpec(key, value) {
    if (typeof value === "boolean") return { input: "checkbox", value };
    if (typeof value === "number") return { input: "number", value };
    if (Array.isArray(value)) {
      return { input: "list", value: value.join(", "), options: OPTIONS[key] };
    }
    if (OPTIONS[key]) return { input: "select", value: String(value), options: OPTIONS[key] };
    return { input: "text", value: value === null || value === undefined ? "" : String(value) };
  }

  /** Converts a rendered field's raw string back into the typed JSON value. */
  function fieldToJson(spec, raw) {
    switch (spec.input) {
      case "checkbox": return raw === "true" || raw === true;
      case "number": {
        const n = Number(raw);
        return Number.isNaN(n) ? raw : n;
      }
      case "list": {
        const text = String(raw).trim();
        if (text === "") return [];
        const numeric = spec.options
          ? spec.options.every((o) => typeof o === "number")
          : /^[-\d.,eE\s]+$/.test(text);
        const parts = text.split(",").map((s) => s.trim()).filter((s) => s !== "");
        if (!numeric) return parts;
        return parts.map((p) => {
          const n = Number(p);
          return Number.isNaN(n) ? p : n;
        });
      }
      default: return raw;
    }
  }

  /** Whether an action type writes to a device endpoint (shows the device picker). */
  function actionNeedsDevice(type) {
    return typeof type === "string" && type.startsWith("control.");
  }

  /**
   * Assembles a brand-new rule object from builder picks (the "new rule" flow).
   * `triggerExample`/`actionExample` are canonical catalogue entries, so the result always
   * validates unless the operator's own values are wrong.
   */
  function newRuleJson({ name, triggerExample, actionExample, deviceId }) {
    const action = JSON.parse(JSON.stringify(actionExample ?? { type: "notify.operator", message: "" }));
    if (deviceId && actionNeedsDevice(action.type)) action.device = deviceId;
    return {
      name,
      trigger: JSON.parse(JSON.stringify(triggerExample ?? { type: "manual" })),
      actions: [action],
    };
  }

  /**
   * Applies editor edits to a loaded rule object, preserving everything the editor does not
   * render (policy, fallback chain, reentrancy, …). The rule format stays the source of truth:
   * unknown fields ride along untouched instead of being dropped.
   */
  function mergeRule(rule, edits) {
    const merged = JSON.parse(JSON.stringify(rule));
    if (edits.name !== undefined) merged.name = edits.name;
    if (edits.description !== undefined) {
      merged.description = edits.description === "" ? undefined : edits.description;
      if (merged.description === undefined) delete merged.description;
    }
    if (edits.version !== undefined) merged.version = edits.version;
    if (edits.priority !== undefined) merged.priority = edits.priority;
    if (edits.tags !== undefined) {
      merged.tags = edits.tags.split(",").map((t) => t.trim()).filter((t) => t !== "");
      if (merged.tags.length === 0) delete merged.tags;
    }
    if (edits.trigger !== undefined) merged.trigger = edits.trigger;
    if (edits.conditions !== undefined) {
      if (edits.conditions.length === 0) delete merged.conditions;
      else merged.conditions = edits.conditions;
    }
    if (edits.actions !== undefined) merged.actions = edits.actions;
    return merged;
  }

  // ---- device editor helpers (pure; the dialog only reads and writes form values) ----

  const DEVICE_KINDS = ["lighting", "display", "audio", "switcher", "media", "other"];
  const DEVICE_PROTOCOLS = ["osc", "artnet", "sacn", "midi", "ump", "dmx512", "media", "virtual"];

  /** Per-protocol hint for the address field; a `null` placeholder means the field is unused. */
  const ADDRESS_HINTS = {
    osc: { placeholder: "192.168.1.50:9000", hint: "host:port the device listens on for OSC." },
    artnet: { placeholder: "192.168.1.60:6454", hint: "host:port of the Art-Net node (default port 6454)." },
    sacn: { placeholder: "192.168.1.60:5568", hint: "host:port of the sACN receiver (default port 5568)." },
    ump: { placeholder: "192.168.1.70:5004", hint: "host:port of the MIDI 2.0 (UMP) gateway." },
    media: { placeholder: "192.168.1.80:9002", hint: "host:port of the media server (commands go out as OSC)." },
    midi: { placeholder: "Behringer UMC", hint: "Part of the MIDI output port name." },
    dmx512: { placeholder: "COM3", hint: "Serial port of the DMX512 adapter, e.g. COM3 or /dev/ttyUSB0." },
    virtual: { placeholder: null, hint: "No address: commands are recorded in memory, for rehearsing." },
  };

  function addressHint(protocol) {
    return ADDRESS_HINTS[protocol] || { placeholder: "", hint: "" };
  }

  /** Scenes as editable text, one per line: `name | universe | start_channel | v,v,v`. */
  function scenesToText(scenes) {
    return Object.entries(scenes || {})
      .map(([name, s]) => `${name} | ${s.universe} | ${s.start_channel ?? 0} | ${(s.values || []).join(",")}`)
      .join("\n");
  }

  /** Parses [`scenesToText`] output; returns `{ scenes }` or `{ error }` naming the bad line. */
  function parseScenes(text) {
    const scenes = {};
    const lines = String(text || "").split("\n").map((l) => l.trim()).filter(Boolean);
    for (const [i, line] of lines.entries()) {
      const parts = line.split("|").map((p) => p.trim());
      if (parts.length !== 4) return { error: `Scene line ${i + 1}: use name | universe | start_channel | values` };
      const [name, universe, start, values] = parts;
      const nums = values.split(",").map((v) => v.trim()).filter((v) => v !== "").map(Number);
      const whole = (n, max) => Number.isInteger(n) && n >= 0 && n <= max;
      if (!name) return { error: `Scene line ${i + 1}: the scene needs a name` };
      if (name in scenes) return { error: `Scene line ${i + 1}: duplicate scene "${name}"` };
      if (universe === "" || !whole(Number(universe), 65535)) return { error: `Scene "${name}": universe must be a whole number` };
      if (start === "" || !whole(Number(start), 511)) return { error: `Scene "${name}": start channel must be 0-511` };
      if (nums.length === 0 || !nums.every((n) => whole(n, 255))) return { error: `Scene "${name}": values must be whole numbers 0-255, comma separated` };
      scenes[name] = { universe: Number(universe), start_channel: Number(start), values: nums };
    }
    return { scenes };
  }

  /**
   * Builds the `DeviceConfig` JSON from the dialog's fields. Empty optional fields are omitted so
   * the YAML stays minimal. Returns `{ device }` or `{ error }`.
   */
  function deviceFromForm(f) {
    const id = String(f.id || "").trim();
    if (!id) return { error: "Give the device an id" };
    if (!/^[A-Za-z0-9._-]+$/.test(id)) return { error: "The id may only contain letters, digits, - _ and ." };
    const device = { id, kind: f.kind || "other", protocol: f.protocol };
    const name = String(f.name || "").trim();
    if (name) device.name = name;
    const address = String(f.address || "").trim();
    if (address && f.protocol !== "virtual") device.address = address;
    const hb = String(f.heartbeat_ms ?? "").trim();
    if (hb) {
      const n = Number(hb);
      if (!Number.isInteger(n) || n <= 0) return { error: "Heartbeat must be a whole number of milliseconds" };
      device.heartbeat_ms = n;
    }
    const parsed = parseScenes(f.scenes);
    if (parsed.error) return { error: parsed.error };
    if (Object.keys(parsed.scenes).length) device.scenes = parsed.scenes;
    if (f.protocol === "media") {
      const media = {};
      if (String(f.media_video || "").trim()) media.video = f.media_video.trim();
      if (String(f.media_audio || "").trim()) media.audio = f.media_audio.trim();
      if (Object.keys(media).length) device.media = media;
    }
    return { device };
  }

  /** Suggests an unused id like `osc-1` for a new device. */
  function suggestDeviceId(protocol, existing) {
    const taken = new Set(existing);
    for (let n = 1; ; n++) {
      const id = `${protocol}-${n}`;
      if (!taken.has(id)) return id;
    }
  }

  return {
    DEVICE_KINDS,
    DEVICE_PROTOCOLS,
    addressHint,
    scenesToText,
    parseScenes,
    deviceFromForm,
    suggestDeviceId,
    OPTIONS,
    pad2,
    fmtClock,
    fmtDateTime,
    fmtAgo,
    fmtDuration,
    esc,
    statusClass,
    healthClass,
    fieldSpec,
    fieldToJson,
    actionNeedsDevice,
    newRuleJson,
    mergeRule,
  };
})();

if (typeof window !== "undefined") window.TptFormat = TptFormat;
if (typeof module !== "undefined" && module.exports) module.exports = TptFormat;
