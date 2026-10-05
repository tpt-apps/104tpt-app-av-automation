// Automated tests for the UI's pure helpers (`node --test ui/test`).
//
// The webview renderer can only be exercised interactively, but everything it computes —
// formatting, field typing, rule assembly — lives in `js/format.js` and is covered here.

const test = require("node:test");
const assert = require("node:assert/strict");
const TptFormat = require("../js/format.js");

const T0 = Date.UTC(2026, 0, 15, 10, 30, 0); // 2026-01-15 10:30:00 UTC

test("clock formatting pads and localises", () => {
  const ms = T0 + 5 * 3600_000 + 7 * 60_000 + 9_000; // 15:37:09 UTC
  const date = new Date(ms);
  const expected = `${TptFormat.pad2(date.getHours())}:${TptFormat.pad2(date.getMinutes())}:09`;
  assert.equal(TptFormat.fmtClock(ms), expected);
  assert.equal(TptFormat.fmtClock(null), "—");
  assert.equal(TptFormat.fmtDateTime(null), "—");
});

test("relative times are human", () => {
  const now = 1_000_000;
  assert.equal(TptFormat.fmtAgo(now - 5_000, now), "5s ago");
  assert.equal(TptFormat.fmtAgo(now - 5 * 60_000, now), "5m ago");
  assert.equal(TptFormat.fmtAgo(now - 3 * 3600_000, now), "3h ago");
  assert.equal(TptFormat.fmtAgo(null, now), "never");
});

test("durations choose a sensible unit", () => {
  assert.equal(TptFormat.fmtDuration(63), "63 ms");
  assert.equal(TptFormat.fmtDuration(999), "999 ms");
  assert.equal(TptFormat.fmtDuration(1200), "1.2 s");
  assert.equal(TptFormat.fmtDuration(null), "—");
});

test("escaping neutralises markup", () => {
  assert.equal(TptFormat.esc(`<img src=x onerror="a&b">`), "&lt;img src=x onerror=&quot;a&amp;b&quot;&gt;");
  assert.equal(TptFormat.esc(undefined), "");
});

test("statuses and health map to classes", () => {
  assert.equal(TptFormat.statusClass("success"), "chip-ok");
  assert.equal(TptFormat.statusClass("partial"), "chip-warn");
  assert.equal(TptFormat.statusClass("failure"), "chip-fail");
  assert.equal(TptFormat.statusClass("mystery"), "");
  assert.equal(TptFormat.healthClass("degraded"), "degraded");
  assert.equal(TptFormat.healthClass("weird"), "unknown");
});

test("field specs type themselves from the value", () => {
  assert.deepEqual(TptFormat.fieldSpec("value", 128), { input: "number", value: 128 });
  assert.deepEqual(TptFormat.fieldSpec("once", false), { input: "checkbox", value: false });
  assert.deepEqual(TptFormat.fieldSpec("address", "/go"), { input: "text", value: "/go" });
  const comparison = TptFormat.fieldSpec("comparison", "gt");
  assert.equal(comparison.input, "select");
  assert.ok(comparison.options.includes("gte"));
  const days = TptFormat.fieldSpec("days", ["mon", "fri"]);
  assert.equal(days.input, "list");
  assert.equal(days.value, "mon, fri");
});

test("unknown enum values still round-trip as text", () => {
  // A field the catalogue does not know must not lose data: text in, text out.
  const spec = TptFormat.fieldSpec("expression", "30 */6 * * *");
  assert.equal(spec.input, "text");
  assert.equal(TptFormat.fieldToJson(spec, "30 */6 * * *"), "30 */6 * * *");
});

test("field values convert back to typed JSON", () => {
  assert.equal(TptFormat.fieldToJson({ input: "number" }, "42"), 42);
  assert.equal(TptFormat.fieldToJson({ input: "checkbox" }, true), true);
  assert.deepEqual(TptFormat.fieldToJson({ input: "list" }, "255, 128, 0"), [255, 128, 0]);
  assert.deepEqual(TptFormat.fieldToJson({ input: "list", options: TptFormat.OPTIONS.days }, "mon, fri"), ["mon", "fri"]);
  assert.deepEqual(TptFormat.fieldToJson({ input: "list" }, "  "), []);
  assert.equal(TptFormat.fieldToJson({ input: "text" }, "/cue/go"), "/cue/go");
});

test("endpoint actions need a device, notifications do not", () => {
  assert.equal(TptFormat.actionNeedsDevice("control.osc"), true);
  assert.equal(TptFormat.actionNeedsDevice("control.dmx_scene"), true);
  assert.equal(TptFormat.actionNeedsDevice("notify.operator"), false);
  assert.equal(TptFormat.actionNeedsDevice("workflow.wait"), false);
  assert.equal(TptFormat.actionNeedsDevice("log.incident"), false);
});

test("new rules are assembled from catalogue examples", () => {
  const rule = TptFormat.newRuleJson({
    name: "Panic run",
    triggerExample: { type: "manual" },
    actionExample: { type: "control.osc", address: "/power", args: [1] },
    deviceId: "proj",
  });
  assert.equal(rule.name, "Panic run");
  assert.deepEqual(rule.trigger, { type: "manual" });
  assert.equal(rule.actions.length, 1);
  assert.equal(rule.actions[0].device, "proj", "endpoint actions get the picked device");
  assert.equal(rule.actions[0].address, "/power");
});

test("new rules without a device pick keep the action untouched", () => {
  const rule = TptFormat.newRuleJson({
    name: "Note",
    triggerExample: { type: "api" },
    actionExample: { type: "notify.operator", message: "hi", severity: "high" },
    deviceId: null,
  });
  assert.equal(rule.actions[0].device, undefined);
  assert.equal(rule.actions[0].severity, "high");
});

test("merging edits preserves what the editor does not render", () => {
  const loaded = {
    id: "failover",
    name: "Failover",
    version: 2,
    armed: true,
    reentrant: true,
    priority: "critical",
    tags: ["video"],
    trigger: { type: "device_state", device: "proj-1", state: "offline" },
    conditions: [{ id: "c1", type: "time_window", from: "18:00", to: "23:30" }],
    actions: [{ id: "a1", type: "notify.operator", message: "go" }],
    policy: { on_failure: "continue", fallback: [{ id: "f1", type: "notify.operator", message: "fallback" }] },
  };
  const merged = TptFormat.mergeRule(loaded, {
    name: "Failover v2",
    version: 3,
    tags: "video, opening",
    description: "",
    conditions: [],
  });
  assert.equal(merged.name, "Failover v2");
  assert.equal(merged.version, 3);
  assert.deepEqual(merged.tags, ["video", "opening"]);
  assert.equal(merged.description, undefined, "empty descriptions are removed, not kept as empty");
  assert.deepEqual(merged.conditions, undefined, "an emptied condition list is omitted");
  // Everything the editor does not touch rides along untouched.
  assert.equal(merged.id, "failover");
  assert.equal(merged.armed, true);
  assert.equal(merged.reentrant, true);
  assert.deepEqual(merged.policy, loaded.policy);
  assert.deepEqual(merged.actions, loaded.actions);
  assert.deepEqual(merged.trigger, loaded.trigger);
});
