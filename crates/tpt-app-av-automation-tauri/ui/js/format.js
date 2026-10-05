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

  return {
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
