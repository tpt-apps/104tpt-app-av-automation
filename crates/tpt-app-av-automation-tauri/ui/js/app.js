// TPT AV Automation UI (spec §12).
//
// The webview only renders: every command goes to the embedded service through the Tauri bridge
// (or the demo fixtures in a plain browser). Business logic lives behind the bridge, where it is
// covered by the Rust test suite.

(() => {
  const { esc, fmtClock, fmtDateTime, fmtAgo, fmtDuration, statusClass, healthClass,
    fieldSpec, fieldToJson, actionNeedsDevice, newRuleJson, mergeRule,
    DEVICE_KINDS, DEVICE_PROTOCOLS, addressHint, scenesToText, deviceFromForm,
    suggestDeviceId } = window.TptFormat;

  const $ = (id) => document.getElementById(id);

  // ---------------------------------------------------------------- transport

  const demo = window.TptDemo && window.TptDemo.active();

  async function call(command, args = {}) {
    if (demo) return window.TptDemo.call(command, args);
    return window.__TAURI__.core.invoke(command, args);
  }

  function toast(message, kind = "info") {
    const el = document.createElement("div");
    el.className = `toast${kind === "error" ? " toast-error" : kind === "ok" ? " toast-ok" : ""}`;
    el.textContent = message;
    $("toasts").appendChild(el);
    setTimeout(() => el.remove(), kind === "error" ? 8000 : 4000);
  }

  const onError = (what) => (e) => toast(`${what}: ${e}`, "error");

  // ---------------------------------------------------------------- state

  const state = {
    view: "dashboard",
    catalogue: [],
    catalogueByKind: { trigger: new Map(), condition: new Map(), action: new Map() },
    pack: null,
    devices: [],
    devicesFile: null,
    selectedRule: null,
    editing: null, // the rule currently in the editor, as loaded from the pack
  };

  async function loadCatalogue() {
    state.catalogue = await call("catalogue");
    for (const group of state.catalogue) {
      state.catalogueByKind[group.kind] = new Map(group.entries.map((e) => [e.kind, e]));
    }
  }

  const entryOf = (kind) => state.catalogueByKind[kind];

  // ---------------------------------------------------------------- header (§12.5)

  function renderHeader(board) {
    const s = board.status;
    $("pack-label").textContent = s.pack;
    const pill = $("status-pill");
    pill.textContent = s.status;
    pill.className = `pill pill-${s.status}`;

    const simulation = s.mode === "simulation";
    const banner = $("mode-banner");
    banner.className = simulation ? "mode-simulation" : "mode-live";
    banner.title = simulation
      ? "The engine is in simulation mode: nothing is sent to any device"
      : "The engine is live: armed rules send real actions";
    $("mode-label").textContent = simulation
      ? "SIMULATION — no live actions are being sent"
      : "LIVE";

    const lost = s.listeners && s.listeners.lost > 0;
    $("dash-status-sub").textContent = lost
      ? `${s.listeners.lost} listener thread(s) lost — that protocol is no longer receiving`
      : `listeners ${s.listeners ? `${s.listeners.live}/${s.listeners.expected}` : "—"}`;

    const toggle = $("btn-toggle-sim");
    if (toggle) {
      toggle.textContent = simulation
        ? "Switch to live mode"
        : "Switch to simulation";
      toggle.title = simulation
        ? "Allow armed rules to send real actions again"
        : "Stop sending real actions; every execution is simulated and marked";
    }
  }

  // ---------------------------------------------------------------- dashboard (§12.1)

  function renderDashboard(board) {
    const s = board.status;
    const statusEl = $("dash-status");
    statusEl.textContent = s.status;
    statusEl.className = `card-big card-status-${s.status}`;

    $("dash-rules").textContent = `${s.armed_rules}/${s.rules}`;
    $("dash-rules-sub").textContent = "armed / total";

    $("dash-devices").textContent = s.devices.total
      ? `${s.devices.online + s.devices.degraded}/${s.devices.total}`
      : "0";
    const parts = [];
    if (s.devices.online) parts.push(`${s.devices.online} online`);
    if (s.devices.degraded) parts.push(`${s.devices.degraded} degraded`);
    if (s.devices.offline) parts.push(`${s.devices.offline} offline`);
    if (s.devices.unknown) parts.push(`${s.devices.unknown} unknown`);
    $("dash-devices-sub").textContent = parts.join(", ") || "no devices configured";

    $("dash-chains").textContent = s.active_chains;

    const rows = $("dash-rules-rows");
    rows.innerHTML = (board.rules ?? []).map((r) => `
      <tr>
        <td><a href="#" class="rule-link" data-rule="${esc(r.id)}">${esc(r.name)}</a>
            <div class="hint mono">${esc(r.id)}</div></td>
        <td>${esc(r.trigger)}</td>
        <td>${esc(r.priority)}</td>
        <td>
          <span class="switch" title="${r.armed ? "Armed — click to disarm" : "Disarmed — click to arm"}">
            <input type="checkbox" data-arm="${esc(r.id)}" ${r.armed ? "checked" : ""} />
            <span class="track"></span>
          </span>
        </td>
        <td><button class="btn btn-small" data-run="${esc(r.id)}">Run</button></td>
      </tr>`).join("") ||
      `<tr><td colspan="5" class="hint">No rules yet — build one in the Rule Builder, or start from a template on the Rules tab.</td></tr>`;

    rows.querySelectorAll("[data-arm]").forEach((input) => {
      input.addEventListener("change", () => {
        const id = input.dataset.arm;
        const want = input.checked;
        (want ? call("arm_rule", { id }) : call("disarm_rule", { id }))
          .then(() => refresh())
          .catch((e) => { onError(want ? "Arm failed" : "Disarm failed")(e); refresh(); });
      });
    });
    rows.querySelectorAll("[data-run]").forEach((btn) => {
      btn.addEventListener("click", () => {
        call("run_rule", { id: btn.dataset.run })
          .then(() => { toast(`Rule "${btn.dataset.run}" ran`); refresh(); })
          .catch(onError("Run failed"));
      });
    });
    rows.querySelectorAll(".rule-link").forEach((a) => {
      a.addEventListener("click", (ev) => { ev.preventDefault(); openRule(a.dataset.rule); });
    });

    const recent = $("dash-recent-rows");
    recent.innerHTML = (board.recent_executions ?? []).map((e) => `
      <tr>
        <td class="mono">${esc(fmtClock(e.triggered_at_ms))}</td>
        <td>${esc(e.rule_name || e.rule_id)}</td>
        <td><span class="chip ${statusClass(e.overall_status)}">${esc(e.overall_status)}</span></td>
        <td>${e.simulated ? `<span class="chip chip-sim">simulated</span>` : `<span class="chip">live</span>`}</td>
      </tr>`).join("") ||
      `<tr><td colspan="4" class="hint">Nothing has run yet.</td></tr>`;

    const chainRows = $("dash-chains-rows");
    chainRows.innerHTML = (board.active_chains ?? []).map((c) => `
      <tr>
        <td class="mono">${esc(c.execution_id)}</td>
        <td>${esc(c.rule_id)}</td>
        <td><button class="btn btn-small btn-danger" data-cancel="${esc(c.execution_id)}">Cancel</button></td>
      </tr>`).join("") ||
      `<tr><td colspan="3" class="hint">No chains are running.</td></tr>`;
    chainRows.querySelectorAll("[data-cancel]").forEach((btn) => {
      btn.addEventListener("click", () => {
        call("cancel_execution", { id: btn.dataset.cancel })
          .then(() => { toast("Chain cancelled"); refresh(); })
          .catch(onError("Cancel failed"));
      });
    });
  }

  // ---------------------------------------------------------------- rule builder (§12.2)

  const SKIP_FIELDS = ["id", "type"];

  function fieldInputs(container, obj) {
    container.innerHTML = "";
    for (const [key, value] of Object.entries(obj)) {
      if (SKIP_FIELDS.includes(key)) continue;
      const spec = fieldSpec(key, value);
      const label = document.createElement("label");
      label.append(key);
      let input;
      if (spec.input === "select") {
        input = document.createElement("select");
        for (const option of spec.options) {
          const opt = document.createElement("option");
          opt.value = option;
          opt.textContent = option;
          input.appendChild(opt);
        }
        input.value = spec.value;
      } else {
        input = document.createElement("input");
        input.type = spec.input === "checkbox" ? "checkbox" : spec.input === "number" ? "number" : "text";
        if (spec.input === "checkbox") input.checked = !!spec.value;
        else input.value = spec.value;
        if (spec.input === "number") input.step = "any";
      }
      input.dataset.field = key;
      input.dataset.kind = spec.input;
      label.appendChild(input);
      container.appendChild(label);
    }
  }

  function fieldsToJson(container, base) {
    const out = { ...(base ?? {}) };
    container.querySelectorAll("[data-field]").forEach((input) => {
      const key = input.dataset.field;
      const raw = input.dataset.kind === "checkbox" ? input.checked : input.value;
      out[key] = fieldToJson({ input: input.dataset.kind }, raw);
    });
    return out;
  }

  function typeSelect(entryKind, current) {
    const label = document.createElement("label");
    label.append("Type");
    const select = document.createElement("select");
    for (const entry of state.catalogue.find((g) => g.kind === entryKind).entries) {
      const opt = document.createElement("option");
      opt.value = entry.kind;
      opt.textContent = `${entry.kind} — ${entry.label}`;
      select.appendChild(opt);
    }
    select.value = current;
    if (select.value !== current) {
      const opt = document.createElement("option");
      opt.value = current;
      opt.textContent = current;
      select.appendChild(opt);
      select.value = current;
    }
    label.appendChild(select);
    return { label, select };
  }

  function addDeviceField(fields) {
    const label = document.createElement("label");
    label.append("device");
    const select = document.createElement("select");
    const ids = new Set(state.devices.map((d) => d.id));
    for (const id of ids) {
      const opt = document.createElement("option");
      opt.value = id;
      opt.textContent = id;
      select.appendChild(opt);
    }
    select.dataset.field = "device";
    select.dataset.kind = "text";
    label.appendChild(select);
    fields.appendChild(label);
    return select;
  }

  function chainItem(list, obj, entryKind, index) {
    const item = document.createElement("div");
    item.className = "chain-item";
    const head = document.createElement("div");
    head.className = "chain-item-head";
    head.innerHTML = `
      <span class="hint">#${index + 1}${obj.id ? ` · ${esc(obj.id)}` : ""}</span>
      <button class="btn btn-small btn-danger" data-remove>Remove</button>`;
    item.appendChild(head);

    const { label, select } = typeSelect(entryKind, obj.type);
    item.appendChild(label);

    const fields = document.createElement("div");
    fields.className = "fields";
    item.appendChild(fields);
    fieldInputs(fields, obj);
    if (entryKind === "action" && actionNeedsDevice(obj.type)) {
      const device = addDeviceField(fields);
      if (obj.device) device.value = obj.device;
    }

    select.addEventListener("change", () => {
      const entry = entryOf(entryKind).get(select.value);
      const fresh = JSON.parse(JSON.stringify(entry ? entry.example : { type: select.value }));
      fresh.id = obj.id;
      if (obj.description !== undefined) fresh.description = obj.description;
      fieldInputs(fields, fresh);
      if (entryKind === "action" && actionNeedsDevice(select.value)) addDeviceField(fields);
    });

    head.querySelector("[data-remove]").addEventListener("click", () => item.remove());
    item._toJson = () => {
      const out = fieldsToJson(fields, obj);
      out.type = select.value;
      return out;
    };
    list.appendChild(item);
  }

  function renderTriggerEditor(trigger) {
    const { label, select } = typeSelect("trigger", trigger.type);
    const slot = $("f-trigger-type-slot");
    slot.innerHTML = "";
    slot.appendChild(label);
    select.id = "f-trigger-type";
    select.addEventListener("change", () => {
      const entry = entryOf("trigger").get(select.value);
      fieldInputs($("f-trigger-fields"),
        JSON.parse(JSON.stringify(entry ? entry.example : { type: select.value })));
    });
    fieldInputs($("f-trigger-fields"), trigger);
  }

  function renderEditor(rule) {
    $("f-name").value = rule.name ?? "";
    $("f-description").value = rule.description ?? "";
    $("f-version").value = rule.version ?? 1;
    $("f-priority").value = rule.priority ?? "normal";
    $("f-tags").value = (rule.tags ?? []).join(", ");
    renderTriggerEditor(rule.trigger ?? { type: "manual" });

    const conditions = $("f-conditions");
    conditions.innerHTML = "";
    (rule.conditions ?? []).forEach((c, i) => chainItem(conditions, c, "condition", i));
    const actions = $("f-actions");
    actions.innerHTML = "";
    (rule.actions ?? []).forEach((a, i) => chainItem(actions, a, "action", i));
  }

  function readEditor() {
    return {
      name: $("f-name").value.trim(),
      description: $("f-description").value.trim(),
      version: Math.max(1, Number($("f-version").value) || 1),
      priority: $("f-priority").value,
      tags: $("f-tags").value,
      trigger: fieldsToJson($("f-trigger-fields"), { type: $("f-trigger-type").value }),
      conditions: [...$("f-conditions").children].map((item) => item._toJson()),
      actions: [...$("f-actions").children].map((item) => item._toJson()),
    };
  }

  function openRule(id) {
    if (id !== null) state.selectedRule = id;
    switchView("rules");
    renderRuleCards();
    const pack = state.pack;
    const rule = pack ? pack.rules.find((r) => r.id === state.selectedRule) : null;
    $("editor-form").hidden = !rule;
    $("editor-empty-hint").hidden = !!rule;
    $("btn-delete-rule").hidden = !rule;
    $("btn-save-rule").disabled = !rule;
    if (!rule) {
      state.editing = null;
      $("editor-title").textContent = "No rule selected";
      return;
    }
    state.editing = JSON.parse(JSON.stringify(rule));
    $("editor-title").textContent = rule.name;
    renderEditor(rule);
  }

  function renderRuleCards() {
    const rules = state.pack ? state.pack.rules : [];
    $("rule-cards").innerHTML = rules.map((r) => `
      <div class="rule-card ${r.id === state.selectedRule ? "selected" : ""}" data-rule="${esc(r.id)}">
        <div class="rule-name">
          <span>${esc(r.name)}</span>
          ${r.armed ? `<span class="chip chip-ok">armed</span>` : `<span class="chip">disarmed</span>`}
        </div>
        <div class="rule-trigger">${esc(r.trigger?.type ?? "")} · v${r.version ?? 1} · ${esc(r.priority ?? "normal")}</div>
        <div class="rule-chain">
          ${(r.conditions ?? []).map((c) => `<span class="chip">${esc(c.type)}</span>`).join("")}
          ${(r.actions ?? []).map((a) => `<span class="chip chip-ok">${esc(a.type)}</span>`).join("")}
        </div>
      </div>`).join("") || `<p class="hint">No rules in this pack.</p>`;

    document.querySelectorAll("#rule-cards .rule-card").forEach((card) => {
      card.addEventListener("click", () => openRule(card.dataset.rule));
    });
  }

  function populateSelect(select, groupKind) {
    select.innerHTML = "";
    for (const entry of state.catalogue.find((g) => g.kind === groupKind).entries) {
      const opt = document.createElement("option");
      opt.value = entry.kind;
      opt.textContent = `${entry.kind} — ${entry.label}`;
      select.appendChild(opt);
    }
  }

  function populateNewRuleDialog() {
    populateSelect($("nr-trigger"), "trigger");
    populateSelect($("nr-action"), "action");
    $("nr-trigger-hint").textContent = "";
    $("nr-action-hint").textContent = "";
    renderNewRuleDevices();
  }

  function renderNewRuleDevices() {
    const select = $("nr-device");
    select.innerHTML = "";
    const needs = actionNeedsDevice($("nr-action").value);
    select.closest("label").style.display = needs ? "" : "none";
    if (!state.devices.length && needs) {
      const opt = document.createElement("option");
      opt.value = "";
      opt.textContent = "No devices yet — add one on the Devices tab";
      select.appendChild(opt);
    }
    for (const d of state.devices) {
      const opt = document.createElement("option");
      opt.value = d.id;
      opt.textContent = `${d.id} (${d.protocol})`;
      select.appendChild(opt);
    }
  }

  // ---------------------------------------------------------------- devices (§12.3)

  function renderDevices() {
    const rows = $("device-rows");
    rows.innerHTML = state.devices.map((d) => `
      <tr>
        <td><span class="dot dot-${healthClass(d.health)}"></span>${esc(d.health)}</td>
        <td>${esc(d.name || d.id)}<div class="hint mono">${esc(d.id)}</div></td>
        <td>${esc(d.protocol)}</td>
        <td class="mono">${esc(d.address ?? "—")}</td>
        <td class="mono">${esc(fmtAgo(d.last_seen_ms, Date.now()))}</td>
        <td class="row-actions">
          <button class="btn btn-small" data-ping="${esc(d.id)}">Ping</button>
          <button class="btn btn-small" data-edit-device="${esc(d.id)}">Edit</button>
          <button class="btn btn-small" data-remove-device="${esc(d.id)}">Delete</button>
        </td>
      </tr>`).join("") ||
      `<tr><td colspan="6" class="empty-state">
        <strong>No devices yet.</strong>
        <p>Devices are the projectors, lighting nodes, audio matrices and media servers your rules
        control. Click <em>+ Add device</em> to add your first one — pick <em>virtual</em> to
        rehearse without hardware. Devices are saved automatically.</p>
      </td></tr>`;
    $("devices-file-hint").textContent = state.devicesFile
      ? `Saved to ${state.devicesFile}`
      : "";

    rows.querySelectorAll("[data-edit-device]").forEach((btn) => {
      btn.addEventListener("click", () => openDeviceDialog(btn.dataset.editDevice));
    });
    rows.querySelectorAll("[data-remove-device]").forEach((btn) => {
      btn.addEventListener("click", async () => {
        const id = btn.dataset.removeDevice;
        if (!window.confirm(`Delete device "${id}"?`)) return;
        try {
          const answer = await call("remove_device", { id });
          if (answer.valid === false) { showDiagnostics("Cannot delete this device", answer); return; }
          toast(`Device "${id}" deleted`, "ok");
          await refresh();
        } catch (e) { onError("Delete failed")(e); }
      });
    });

    rows.querySelectorAll("[data-ping]").forEach((btn) => {
      btn.addEventListener("click", () => {
        btn.disabled = true;
        call("ping_device", { id: btn.dataset.ping })
          .then((r) => {
            const proto = (state.devices.find((d) => d.id === r.device) || {}).protocol;
            // UDP protocols are connectionless: a ping can only prove the endpoint opened.
            const udp = ["osc", "artnet", "sacn", "ump", "media"].includes(proto);
            toast(!r.reachable
              ? `Device "${r.device}" did not answer`
              : udp
                ? `Endpoint for "${r.device}" is ready (${proto} is connectionless, so this cannot confirm the device is listening)`
                : `Device "${r.device}" is reachable`, r.reachable ? "ok" : "error");
          })
          .catch(onError("Ping failed"))
          .finally(() => { btn.disabled = false; });
      });
    });
  }

  // Device dialog: add (no id) or edit (id of an existing device).
  async function openDeviceDialog(id) {
    const dialog = $("device-dialog");
    let cfg = null;
    if (id) {
      try {
        const all = await call("device_configs");
        cfg = (all.devices || []).find((d) => d.id === id) || null;
      } catch (e) { onError("Could not load the device")(e); return; }
      if (!cfg) { toast(`Device "${id}" no longer exists`, "error"); return; }
    }
    const protocol = cfg ? cfg.protocol : "osc";
    $("dd-title").textContent = cfg ? `Edit ${cfg.id}` : "Add device";
    $("dd-id").value = cfg ? cfg.id : suggestDeviceId(protocol, state.devices.map((d) => d.id));
    $("dd-id").disabled = !!cfg;
    $("dd-name").value = cfg?.name ?? "";
    $("dd-protocol").value = protocol;
    $("dd-kind").value = cfg?.kind ?? "other";
    $("dd-address").value = cfg?.address ?? "";
    $("dd-heartbeat").value = cfg?.heartbeat_ms ?? "";
    $("dd-media-video").value = cfg?.media?.video ?? "";
    $("dd-media-audio").value = cfg?.media?.audio ?? "";
    $("dd-scenes").value = scenesToText(cfg?.scenes);
    dialog.dataset.editing = cfg ? cfg.id : "";
    syncDeviceDialog();
    dialog.showModal();
  }

  function syncDeviceDialog() {
    const protocol = $("dd-protocol").value;
    const { placeholder, hint } = addressHint(protocol);
    $("dd-address").placeholder = placeholder ?? "";
    $("dd-address").closest("label").style.display = placeholder === null ? "none" : "";
    $("dd-address-hint").textContent = hint;
    $("dd-media").style.display = protocol === "media" ? "" : "none";
    const dmx = ["artnet", "sacn", "dmx512", "virtual"].includes(protocol);
    $("dd-scenes-row").style.display = dmx ? "" : "none";
    $("dd-scenes-hint").style.display = dmx ? "" : "none";
  }

  function showDiagnostics(title, answer) {
    const diags = (answer.diagnostics ?? []).map((d) => `${d.location}: ${d.message}`).join("\n");
    toast(`${title}:\n${diags}`, "error");
  }

  function wireTemplateDialog() {
    const dialog = $("template-dialog");
    let list = [];
    const about = () => {
      const t = list.find((x) => x.name === $("tp-select").value);
      $("tp-about").textContent = t ? t.about : "";
    };
    $("btn-template").addEventListener("click", async () => {
      try { list = await call("templates"); } catch (e) { onError("Could not load templates")(e); return; }
      $("tp-select").innerHTML = "";
      for (const t of list) $("tp-select").add(new Option(t.name, t.name));
      about();
      dialog.showModal();
    });
    $("tp-select").addEventListener("change", about);
    $("tp-cancel").addEventListener("click", () => dialog.close());
    $("tp-apply").addEventListener("click", async () => {
      const name = $("tp-select").value;
      try {
        const answer = await call("apply_template", { name });
        if (answer.valid === false) { showDiagnostics("The template was rejected", answer); return; }
        dialog.close();
        const rules = (answer.added_rules || []).length;
        const devs = (answer.added_devices || []).length;
        toast(`Added ${rules} rule(s) and ${devs} device(s) from "${name}". Rules start disarmed.`, "ok");
        await refresh();
      } catch (e) { onError("Template failed")(e); }
    });
  }

  function wireDeviceDialog() {
    const dialog = $("device-dialog");
    for (const p of DEVICE_PROTOCOLS) $("dd-protocol").add(new Option(p, p));
    for (const k of DEVICE_KINDS) $("dd-kind").add(new Option(k, k));
    $("btn-add-device").addEventListener("click", () => openDeviceDialog(null));
    $("dd-cancel").addEventListener("click", () => dialog.close());
    $("dd-protocol").addEventListener("change", () => {
      syncDeviceDialog();
      if (!dialog.dataset.editing) {
        $("dd-id").value = suggestDeviceId($("dd-protocol").value, state.devices.map((d) => d.id));
      }
    });
    $("dd-save").addEventListener("click", async () => {
      const built = deviceFromForm({
        id: $("dd-id").value,
        name: $("dd-name").value,
        kind: $("dd-kind").value,
        protocol: $("dd-protocol").value,
        address: $("dd-address").value,
        heartbeat_ms: $("dd-heartbeat").value,
        scenes: $("dd-scenes-row").style.display === "none" ? "" : $("dd-scenes").value,
        media_video: $("dd-media-video").value,
        media_audio: $("dd-media-audio").value,
      });
      if (built.error) { toast(built.error, "error"); return; }
      if (!dialog.dataset.editing && state.devices.some((d) => d.id === built.device.id)) {
        toast(`A device with id "${built.device.id}" already exists`, "error");
        return;
      }
      try {
        const answer = await call("upsert_device", { device: built.device });
        if (answer.valid === false) { showDiagnostics("This device was rejected", answer); return; }
        dialog.close();
        toast(`Device "${built.device.id}" saved`, "ok");
        await refresh();
      } catch (e) { onError("Save failed")(e); }
    });
  }

  // ---------------------------------------------------------------- timeline (§12.4)

  function fetchTimeline() {
    const sinceRaw = $("flt-since").value;
    const untilRaw = $("flt-until").value;
    return call("executions", {
      rule: $("flt-rule").value.trim() || null,
      status: $("flt-status").value || null,
      device: $("flt-device").value.trim() || null,
      simulated: $("flt-simulated").value === "" ? null : $("flt-simulated").value === "true",
      since: sinceRaw ? new Date(sinceRaw).getTime() : null,
      until: untilRaw ? new Date(untilRaw).getTime() : null,
      limit: Number($("flt-limit").value) || 100,
    });
  }

  function renderTimeline() {
    return fetchTimeline().then((result) => {
      const rows = $("exec-rows");
      rows.innerHTML = (result.executions ?? []).map((e) => `
        <tr data-exec="${esc(e.execution_id)}">
          <td class="mono">${esc(fmtDateTime(e.triggered_at_ms))}</td>
          <td>${esc(e.rule_name || e.rule_id)}<div class="hint mono">${esc(e.rule_id)}</div></td>
          <td><span class="chip ${statusClass(e.overall_status)}">${esc(e.overall_status)}</span></td>
          <td class="mono">${e.action_results ? `${e.action_results.filter((a) => a.status === "success").length}/${e.action_results.length}` : "—"}</td>
          <td class="mono">${esc(fmtDuration(e.duration_ms))}</td>
          <td>${e.simulated ? `<span class="chip chip-sim">simulated</span>` : `<span class="chip">live</span>`}</td>
        </tr>`).join("") ||
        `<tr><td colspan="6" class="hint">No executions match these filters.</td></tr>`;

      rows.querySelectorAll("tr[data-exec]").forEach((tr) => {
        tr.addEventListener("click", () => {
          rows.querySelectorAll("tr").forEach((x) => x.classList.remove("selected"));
          tr.classList.add("selected");
          showExecutionDetail((result.executions ?? [])
            .find((e) => e.execution_id === tr.dataset.exec));
        });
      });
    });
  }

  function showExecutionDetail(e) {
    const detail = $("exec-detail");
    if (!e) {
      detail.hidden = true;
      return;
    }
    const stepChip = (status) => statusClass(
      status === "success" ? "success"
        : status === "skipped" ? "cancelled"
          : status === "partial" ? "partial" : "failure");
    const conditions = (e.condition_results ?? []).map((c) => `
      <div class="diag"><span class="chip ${c.passed ? "chip-ok" : "chip-fail"}">${c.passed ? "passed" : "failed"}</span>
        <span class="loc">${esc(c.id)}</span> ${esc(c.description ?? "")}</div>`).join("")
      || `<p class="hint">No conditions configured — the trigger alone decides.</p>`;
    const actions = (e.action_results ?? []).map((a) => `
      <div class="diag">
        <span class="chip ${stepChip(a.status)}">${esc(a.status)}</span>
        <span class="loc">${esc(a.id)}${a.device ? ` → ${esc(a.device)}` : ""}</span>
        ${esc(fmtDuration(a.duration_ms))}${a.error ? ` — <b>${esc(a.error)}</b>` : ""}
      </div>`).join("") || `<p class="hint">No actions ran.</p>`;
    detail.innerHTML = `
      <button class="btn btn-small detail-close" id="exec-detail-close">Close</button>
      <h2>${esc(e.rule_name || e.rule_id)} <span class="hint mono">${esc(e.execution_id)}</span></h2>
      <p class="hint">trigger <b>${esc(e.trigger_type)}</b> · rule v${e.rule_version} · ${e.simulated ? "simulated" : "live"}</p>
      <h3>Trigger detail</h3>
      <pre>${esc(JSON.stringify(e.trigger_detail, null, 2))}</pre>
      <h3>Conditions</h3>
      ${conditions}
      <h3>Actions</h3>
      ${actions}
      <div class="row-gap" style="margin-top:10px">
        <button class="btn btn-small" id="exec-goto-rule">Open rule in builder</button>
      </div>`;
    detail.hidden = false;
    $("exec-detail-close").addEventListener("click", () => { detail.hidden = true; });
    $("exec-goto-rule").addEventListener("click", () => openRule(e.rule_id));
  }

  async function renderIncidents() {
    try {
      const incidents = await call("incidents", { limit: 100 });
      $("incident-rows").innerHTML = (Array.isArray(incidents) ? incidents : []).map((i) => `
        <tr>
          <td class="mono">${esc(fmtDateTime(i.at_ms))}</td>
          <td class="mono">${esc(i.rule_id)}</td>
          <td><span class="chip ${i.severity === "critical" || i.severity === "high" ? "chip-fail" : "chip-warn"}">${esc(i.severity)}</span></td>
          <td>${esc(i.message)}</td>
        </tr>`).join("") || `<tr><td colspan="4" class="hint">No incidents logged.</td></tr>`;
    } catch {
      $("incident-rows").innerHTML = `<tr><td colspan="4" class="hint">Incident log unavailable.</td></tr>`;
    }
  }

  async function renderHistory() {
    try {
      const history = await call("pack_history");
      $("history-rows").innerHTML = history.map((v) => `
        <tr>
          <td class="mono">${esc(fmtDateTime(v.saved_at_ms))}</td>
          <td>${esc(v.name)}</td>
          <td class="mono">r${v.revision}</td>
          <td><button class="btn btn-small" data-restore="${v.id}">Restore</button></td>
        </tr>`).join("") || `<tr><td colspan="4" class="hint">No saved versions yet.</td></tr>`;
      $("history-rows").querySelectorAll("[data-restore]").forEach((btn) => {
        btn.addEventListener("click", () => {
          call("restore_pack_version", { id: Number(btn.dataset.restore) })
            .then(() => { toast("Pack version restored"); refresh(); })
            .catch(onError("Restore failed"));
        });
      });
    } catch {
      $("history-rows").innerHTML = `<tr><td colspan="4" class="hint">Version history unavailable.</td></tr>`;
    }
  }

  // ---------------------------------------------------------------- yaml view (§12.2 text side)

  async function loadYaml() {
    try {
      $("yaml-editor").value = await call("pack_yaml");
      $("yaml-result").hidden = true;
      const info = await call("app_info");
      $("yaml-file-hint").textContent = info.pack_file
        ? `Loaded from ${info.pack_file} — "Save to file" writes this text there.`
        : "This pack was created in the builder and has no file on disk; edits live in the engine and its version history.";
    } catch (e) {
      onError("Could not load the pack")(e);
    }
  }

  function showYamlResult(result) {
    const box = $("yaml-result");
    box.hidden = false;
    if (result.valid) {
      box.innerHTML = `<div class="ok">Valid — ${esc(String(result.rules ?? ""))} rule(s), revision r${esc(String(result.revision ?? "?"))}${result.name ? ` of “${esc(result.name)}”` : ""}.</div>`;
    } else {
      const diags = (result.diagnostics ?? []).map((d) => `
        <div class="diag diag-${esc(d.severity)}"><span class="loc">${esc(d.location)}</span>${esc(d.message)}</div>`).join("");
      box.innerHTML = `<div class="diag-error">Rejected.</div>${diags}`;
    }
  }

  // ---------------------------------------------------------------- view switching & refresh

  function switchView(view) {
    state.view = view;
    document.querySelectorAll(".tab").forEach((t) =>
      t.classList.toggle("active", t.dataset.view === view));
    document.querySelectorAll(".view").forEach((v) =>
      v.classList.toggle("active", v.id === `view-${view}`));
    if (view === "yaml") loadYaml();
    if (view === "timeline") { renderTimeline().catch(onError("Timeline failed")); renderIncidents(); renderHistory(); }
  }

  let refreshSeq = 0;
  async function loadDevicesFile() {
    try { state.devicesFile = (await call("app_info")).devices_file ?? null; } catch { /* hint only */ }
  }

  async function refresh() {
    const seq = ++refreshSeq;
    let board = null;
    if (state.devicesFile === null) await loadDevicesFile();
    try {
      board = await call("dashboard");
      if (seq !== refreshSeq) return;
      renderHeader(board);
      renderDashboard(board);
    } catch (e) {
      if (demo) onError("Dashboard")(e);
    }
    try {
      const devices = await call("devices");
      if (seq !== refreshSeq) return;
      state.devices = devices.devices ?? [];
      renderDevices();
    } catch (e) {
      if (demo) onError("Devices")(e);
    }
    await refreshPackDependentViews();
  }

  async function refreshPackDependentViews() {
    try {
      state.pack = await call("pack_json");
    } catch {
      return;
    }
    if (state.selectedRule && !state.pack.rules.some((r) => r.id === state.selectedRule)) {
      state.selectedRule = null;
      state.editing = null;
      if (state.view === "rules") openRule(null);
    }
    renderRuleCards();
  }

  // ---------------------------------------------------------------- wiring

  function showRuleDiagnostics(answer) {
    const diags = (answer.diagnostics ?? []).map((d) => `${d.location}: ${d.message}`).join("\n");
    toast(`The pack rejected this rule:\n${diags}`, "error");
  }

  function wire() {
    document.querySelectorAll(".tab").forEach((tab) => {
      tab.addEventListener("click", () => switchView(tab.dataset.view));
    });
    document.querySelectorAll("[data-goto]").forEach((btn) => {
      btn.addEventListener("click", () => switchView(btn.dataset.goto));
    });

    $("btn-flt-apply").addEventListener("click", () => renderTimeline().catch(onError("Timeline failed")));

    // Live ⇄ simulation switch (spec §12.5): the banner always shows which mode is running.
    $("btn-toggle-sim").addEventListener("click", () => {
      const want = !state.simulation;
      call("set_simulation", { on: want })
        .then(() => { toast(want ? "Simulation mode on — no live actions will be sent" : "Live mode on", "ok"); refresh(); })
        .catch(onError("Mode switch failed"));
    });

    $("btn-save-rule").addEventListener("click", async () => {
      if (!state.editing) return;
      const rule = mergeRule(state.editing, readEditor());
      try {
        const answer = await call("upsert_rule", { rule });
        if (answer.valid === false) { showRuleDiagnostics(answer); return; }
        toast(`Rule "${rule.name}" saved`, "ok");
        await refresh();
        openRule(state.selectedRule);
      } catch (e) { onError("Save failed")(e); }
    });

    $("btn-delete-rule").addEventListener("click", async () => {
      if (!state.selectedRule) return;
      try {
        await call("remove_rule", { id: state.selectedRule });
        toast("Rule removed", "ok");
        state.selectedRule = null;
        await refresh();
        openRule(null);
      } catch (e) { onError("Delete failed")(e); }
    });

    $("btn-add-condition").addEventListener("click", () => {
      const list = $("f-conditions");
      const entry = state.catalogue.find((g) => g.kind === "condition").entries[0];
      chainItem(list, JSON.parse(JSON.stringify(entry.example)), "condition", list.children.length);
    });
    $("btn-add-action").addEventListener("click", () => {
      const list = $("f-actions");
      const entry = state.catalogue.find((g) => g.kind === "action").entries[0];
      chainItem(list, JSON.parse(JSON.stringify(entry.example)), "action", list.children.length);
    });

    wireDeviceDialog();
    wireTemplateDialog();

    // New-rule dialog.
    const dialog = $("new-rule-dialog");
    $("btn-new-rule").addEventListener("click", () => {
      populateNewRuleDialog();
      $("nr-name").value = "";
      dialog.showModal();
    });
    $("nr-cancel").addEventListener("click", () => dialog.close());
    $("nr-action").addEventListener("change", renderNewRuleDevices);
    $("nr-create").addEventListener("click", async () => {
      const name = $("nr-name").value.trim();
      if (!name) { toast("Give the rule a name first", "error"); return; }
      const trigger = entryOf("trigger").get($("nr-trigger").value);
      const action = entryOf("action").get($("nr-action").value);
      const rule = newRuleJson({
        name,
        triggerExample: trigger.example,
        actionExample: action.example,
        deviceId: $("nr-device").value || null,
      });
      try {
        const answer = await call("upsert_rule", { rule });
        if (answer.valid === false) { showRuleDiagnostics(answer); return; }
        dialog.close();
        toast(`Rule "${name}" created — it starts disarmed`, "ok");
        await refresh();
        const created = state.pack.rules.find((r) => r.name === name);
        openRule(created ? created.id : null);
      } catch (e) { onError("Create failed")(e); }
    });

    // YAML view buttons.
    $("btn-yaml-reload").addEventListener("click", loadYaml);
    $("btn-yaml-validate").addEventListener("click", async () => {
      try { showYamlResult(await call("validate_pack", { yaml: $("yaml-editor").value })); }
      catch (e) { onError("Validate failed")(e); }
    });
    $("btn-yaml-apply").addEventListener("click", async () => {
      try {
        const answer = await call("apply_pack", { yaml: $("yaml-editor").value });
        showYamlResult(answer);
        if (answer.valid) {
          toast("Pack applied", "ok");
          state.selectedRule = null;
          await refresh();
        }
      } catch (e) { onError("Apply failed")(e); }
    });
    $("btn-yaml-save").addEventListener("click", async () => {
      try {
        const answer = await call("save_pack_file");
        toast(`Saved to ${answer.saved}`, "ok");
      } catch (e) { onError("Save failed")(e); }
    });
  }

  // ---------------------------------------------------------------- boot

  async function boot() {
    $("app-version").textContent = demo ? "browser preview" : "";
    try {
      await loadCatalogue();
    } catch (e) {
      onError("Catalogue failed")(e);
      return;
    }
    wire();
    await refresh();
    setInterval(() => { refresh().catch(() => {}); }, 2500);

    if (!demo && window.__TAURI && window.__TAURI.event && window.__TAURI.event.listen) {
      window.__TAURI__.event.listen("execution", () => {
        refresh().catch(() => {});
        if (state.view === "timeline") renderTimeline().catch(() => {});
      });
    }
  }

  document.addEventListener("DOMContentLoaded", boot);
})();
