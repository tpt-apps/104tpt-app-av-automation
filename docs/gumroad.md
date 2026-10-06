# Gumroad listing: TPT AV Automation

Everything needed to create the product page. Copy each section into the matching Gumroad field.
Items in `[BRACKETS]` are decisions or assets you still need to supply.

---

## 1. Build and upload checklist

1. Build the installer:
   ```powershell
   cd crates\tpt-app-av-automation-tauri
   cargo tauri build --bundles nsis
   ```
2. Find it in `target\release\bundle\nsis\` (named like `TPT AV Automation_0.1.0_x64-setup.exe`).
3. Install it on a clean Windows 10/11 machine or VM and confirm it launches, loads a rule pack and
   uninstalls cleanly.
4. Compute the checksum and paste it into section 6:
   ```powershell
   Get-FileHash ".\TPT AV Automation_0.1.0_x64-setup.exe" -Algorithm SHA256
   ```
5. Optional: put the installer in a `.zip` with `README.txt` and the sample `rules/` folder.
6. Upload it under **Content** on the Gumroad product (section 5).

> The installer is **unsigned**. Windows SmartScreen will warn on first run. This is covered in the
> description and the receipt message below so buyers are not surprised.

---

## 2. Product basics

| Field | Value |
|-------|-------|
| **Name** | TPT AV Automation |
| **URL slug** | `tpt-av-automation` |
| **Price** | Three tiers, see below (spec.txt �2.1). Same installer in every tier |
| **Category** | Software / Business & Money, or Audio & Video tools |
| **Tags** | av, live production, automation, osc, midi, dmx, art-net, sacn, show control, offline |
| **Summary** (shown under the title, ~1 line) | Offline-first show automation for AV: WHEN something happens, IF conditions hold, THEN run an auditable sequence, with no programmer in the room. |


### Pricing tiers

Prices come from the spec's pricing hypothesis (spec.txt �2.1) and are **unvalidated**. Sell them as
three Gumroad products (or three versions of one product). There is no licence check, so the tiers
differ in licensed scope and support, not in features.

| Tier | Price | Licensed scope | Intended buyer |
|------|-------|----------------|----------------|
| Standalone | $999 | One room or venue, one operator | Single venue |
| Studio | $2,499 | Multiple rooms in one facility, small integration team | Integration team |
| Facility / Enterprise | $4,999+ | Multi-site, integrator-managed deployments | Integrators, multi-site |

- Perpetual licence, no subscription and no cloud costs. Paid major-version upgrades and support
  contracts can come later.
- Audience: the Gumroad listing on the website sells a supported, ready-to-install product to
  professional buyers. The open-source repo is a separate audience of people who find the source and
  build it themselves. The tiers are not aimed at them.
- `[Decide what each tier includes for support, e.g. email response time, onboarding call]`

---

## 3. Product description (paste into the description field)

**TPT AV Automation** runs your AV and live-production routines for you, reliably and offline.

> **WHEN** something happens, **IF** the right conditions hold, **THEN** run a defined, auditable
> sequence of actions.

Rules live in human-readable, versioned YAML *rule packs*. Every rule can be **simulated** before it
is **armed**. Nothing is sent during a simulation. Every run records *why* it fired, what each
condition said and what each action did, and a partly failed chain is never reported as a success.

### What it does
- **Triggers:** time schedules (including sunrise/sunset), OSC, MIDI 1.0 and 2.0, DMX512, Art-Net,
  sACN, device health changes, heartbeats and manual or API triggers.
- **Actions:** OSC, MIDI, DMX (Art-Net, sACN, serial DMX512), notifications, incident logging, waits,
  chaining other rules and sandboxed command execution.
- **Desktop app:** dashboard with armed states and device health, a visual rule builder that
  round-trips to the YAML, a device manager with manual pings, and an execution timeline with filters.
  A LIVE / SIMULATION banner is always on screen.
- **Headless service:** runs without the UI, with SQLite state that survives restarts and a watchdog
  that restarts the engine if it is killed.
- **Local API:** REST and WebSocket, off by default, token-protected and loopback only.

### Built for the show
- **Offline by design.** No account, no telemetry, no licence check, no cloud relay.
- **Safe by default.** Rules load disarmed until you arm them.
- **Deterministic.** The same inputs always produce the same result, and the tests prove it.
- **Fuzzed.** The rule parser and every inbound protocol are fuzz-tested.

### What you get
- Windows installer (`.exe`) for the desktop app, CLI and service
- Example rule packs and device files
- Free updates for `[1 year / the 1.x line / life]`

### Requirements
- Windows 10 or 11 (64-bit)
- WebView2 runtime (included with Windows 11 and current Windows 10; the installer fetches it if
  missing)
- Network or serial access to the devices you want to control

### Honest status
This is version 0.1. The engine, CLI, service and desktop app are complete and heavily tested in
simulation. These parts have **not yet been tested against real hardware**: live MIDI ports, serial
DMX512 adapters and media servers (media actions go out as OSC). Test your own rig before relying on
it for a paid show.

### Open source
The source is dual-licensed MIT or Apache-2.0: `[REPO URL]`. Buying here gets you the prebuilt
installer, the licensed scope of your tier, support and updates. You are free to build it yourself,
but that comes without a supported installer.

### Windows SmartScreen note
The installer is not code-signed, so Windows may show "Windows protected your PC".
Click **More info → Run anyway**. You can verify the download against the SHA-256 checksum listed
on this page.

---

## 4. Cover and thumbnail assets

| Asset | Spec | Suggestion |
|-------|------|------------|
| Cover image | 1280×720 PNG | Dashboard screenshot with the LIVE / SIMULATION banner visible |
| Thumbnail | 600×600 PNG | App icon on a dark background |
| Gallery | 3–5 images | Rule builder, device manager, timeline, a simulation result |
| Short video | optional | 30 to 60 seconds: build a rule, simulate it, arm it |

`[SCREENSHOTS TO CAPTURE]`

---

## 5. Content (files delivered)

| File | Notes |
|------|-------|
| `TPT AV Automation_0.1.0_x64-setup.exe` | The installer |
| `README.txt` (optional) | Install steps, SmartScreen note, support email |
| `rules/` examples (optional) | From the repo's `rules/` folder |

Gumroad settings:
- **License keys:** off. The app has no licence check.
- **Limit downloads:** off.
- **Versioning:** upload each new installer as an update and use the "send update email" option.

---

## 6. Checksum block (paste at the bottom of the description or into README.txt)

```
SHA-256: 718EB3867DFA5665BE56F1476550AF5407F46A2BFBBE8B68FABEBAEFCB8F247B
File:    TPT AV Automation_0.1.0_x64-setup.exe
Version: 0.1.0
```

---

## 7. Receipt / post-purchase message

> Thanks for buying TPT AV Automation!
>
> **Install:** download the `.exe` and run it. If Windows shows "Windows protected your PC", click
> **More info → Run anyway**. The installer is not code-signed, and you can check the SHA-256
> checksum on the product page.
>
> **First steps:** open the app with no arguments for a fresh "new show". Rules load disarmed, so
> use **Simulate** first and **Arm** only when you are happy.
>
> **Help:** `[SUPPORT EMAIL]` · docs: `[REPO URL]/tree/master/docs`

---

## 8. Policies

- **Refund policy:** `[e.g. 30-day no-questions refund]`
- **Support:** `[email, response time]`
- **Terms:** software is provided as-is under MIT OR Apache-2.0. For live shows, test on your own
  rig first.

---

## 9. Pre-publish checklist

- [ ] Installer tested on a clean machine
- [ ] SHA-256 pasted in section 6
- [ ] Tier prices validated, tier support terms, support email and refund policy filled in
- [ ] Cover, thumbnail and screenshots uploaded
- [ ] Content file attached and the "preview" or "view product" page checked
- [ ] Test purchase with a 100% discount code to check the delivery email and the download
