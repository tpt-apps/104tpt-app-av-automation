# Clean-Machine Installation Test Report

## Environment

- **OS:** Windows 10 (21H2+) or Windows Server 2019
- **Prerequisites:** PowerShell 5.0+ (included by default on Windows 10+)
- **No development tools required** (Visual C++ Redistributable, Rust, etc.)
- **No internet connection required** for core operation

---

## Test Procedure

This procedure validates that the packaged Windows release (`tpt-av-automation-X.X.X-windows-x64.zip`)
can be installed and run on a clean machine without any missing DLL dependencies or prerequisite
software.

The test sequence is split into six phases:

### Phase 1: Basic Verification (No Admin Required)

Extract the package and verify the executable runs without missing dependencies.

```powershell
# 1. Download and extract package
# (Replace with actual URL or use pre-downloaded zip)
Invoke-WebRequest https://repo.example.com/tpt-av-automation-0.1.0-windows-x64.zip `
    -OutFile automation.zip
Expand-Archive .\automation.zip

cd tpt-av-automation-0.1.0-windows-x64

# 2. Verify exe runs and shows version
.\tpt-av-automation.exe --version
# Expected output: tpt-av-automation 0.1.0

# 3. Check help text
.\tpt-av-automation.exe --help
# Expected: Usage information, no DLL/dependency errors

# 4. List available commands
.\tpt-av-automation.exe
# Expected: help text with subcommands (validate, simulate, run, etc.)
```

**Success Criteria:**
- [ ] No "missing DLL" errors (e.g., `msvcp140.dll`, `vcruntime140.dll`)
- [ ] No "Visual C++ Redistributable" or .NET errors
- [ ] Executable responds to `--version` and `--help`
- [ ] Help text is readable and complete

**Common Failures and Fixes:**
- *Error: "DLL not found"* → Inspect `target/release/deps/` for static linking issues; may need
  to rebuild with `/MD` or `/MT` CRT option.
- *Error: "This app can't run on your PC"* → Check that binary is built for correct architecture
  (x86_64 for 64-bit Windows).

---

### Phase 2: YAML Validation (No Admin Required)

Verify the rule pack and device file parsers work without errors.

```powershell
# 5. Validate example rules
.\tpt-av-automation.exe validate `
    --rules .\rules\examples\hello.yaml `
    --devices .\rules\examples\devices.yaml
# Expected: "Pack is valid" or similar success message

# 6. Validate live-event rules (if included)
.\tpt-av-automation.exe validate `
    --rules .\rules\live-event\main-hall-event-start.yaml `
    --devices .\rules\examples\devices.yaml
# Expected: Same success message
```

**Success Criteria:**
- [ ] Both YAML files parse without errors
- [ ] Device registry is recognized
- [ ] No runtime panics or crashes

**Common Failures and Fixes:**
- *Error: "file not found"* → Verify paths are correct and package structure is intact.
- *Error: "invalid YAML"* → Ensure files were extracted correctly (not corrupted zip).

---

### Phase 3: Simulation (No Admin Required)

Verify rule simulation (dry-run) works correctly.

```powershell
# 7. Simulate a schedule trigger
.\tpt-av-automation.exe simulate `
    --rules .\rules\examples\hello.yaml `
    --event "schedule:12:00" `
    --dry-run
# Expected: Execution trace showing rule fired or no match

# 8. Simulate an OSC trigger
.\tpt-av-automation.exe simulate `
    --rules .\rules\examples\hello.yaml `
    --event "osc:/cue/1=1"
# Expected: Another execution trace
```

**Success Criteria:**
- [ ] Simulation produces execution traces without errors
- [ ] Event parsing works correctly
- [ ] No crashes or panics
- [ ] Output is readable (JSON or text format)

**Common Failures and Fixes:**
- *Error: "unknown event format"* → Check event syntax; must be `type:param` format.
- *Panic or crash* → May indicate a bug in rule parsing; report with full error output.

---

### Phase 4: Service Installation (Admin PowerShell Required)

Register the application as a Windows Task Scheduler task that runs at startup.

**Open PowerShell as Administrator:**
```powershell
# Right-click PowerShell → "Run as administrator"
# Then navigate to the extracted directory and proceed:

cd C:\Users\<username>\Downloads\tpt-av-automation-0.1.0-windows-x64

# 9. Create a stable location for rules and device file
New-Item -ItemType Directory -Path C:\show -Force
Copy-Item .\rules\examples\hello.yaml C:\show\main-hall.yaml
Copy-Item .\rules\examples\devices.yaml C:\show\devices.yaml

# 10. Run the installation script
.\install-windows-service.ps1 `
    -Rules C:\show\main-hall.yaml `
    -Devices C:\show\devices.yaml

# Expected: Task created successfully; script completes without errors

# 11. Verify the task was created
Get-ScheduledTask -TaskName "TPT AV Automation"
# Expected: Task object with Name, State, and other properties

# 12. Check state directory was created
Get-Item C:\ProgramData\TptAvAutomation -ErrorAction SilentlyContinue
# Expected: Directory exists and contains database files (or will be created on first run)
```

**Success Criteria:**
- [ ] `install-windows-service.ps1` completes without errors
- [ ] Task "TPT AV Automation" appears in Task Scheduler
- [ ] Task is enabled and scheduled for startup
- [ ] State directory exists (or creation is deferred until first run)

**Common Failures and Fixes:**
- *Error: "Access denied"* → Ensure PowerShell is running as Administrator.
- *Error: "Task already exists"* → Either delete the old task first or change the task name in
  `install-windows-service.ps1`.
- *Error: "Invalid path"* → Verify rule and device file paths are absolute and correct.

---

### Phase 5: Runtime Verification (Admin PowerShell Required)

Manually start the task and verify the process runs and creates state.

```powershell
# 13. Manually trigger the task (do not wait for reboot)
Start-ScheduledTask -TaskName "TPT AV Automation"

# 14. Wait a moment and check if process started
Start-Sleep -Seconds 3
Get-Process tpt-av-automation -ErrorAction SilentlyContinue
# Expected: One or two processes (watchdog + engine)
# If none appear, check Event Viewer for errors

# 15. Verify state database was created
Get-ChildItem C:\ProgramData\TptAvAutomation\
# Expected: Files like automation.db, automation.db-wal, automation.db-shm

# 16. Query execution history (optional, if API is enabled)
# This requires the local API to be enabled in config
# .\tpt-av-automation.exe history `
#     --state-dir C:\ProgramData\TptAvAutomation `
#     --limit 5
# Expected: Execution records from the task run
```

**Success Criteria:**
- [ ] Process starts and runs for at least 10 seconds
- [ ] Database file exists at `C:\ProgramData\TptAvAutomation\automation.db`
- [ ] No immediate crashes or unexpected exits
- [ ] Process is visible in Task Manager under "Services" or "Applications" tab

**Common Failures and Fixes:**
- *No process appears* → Check Task Scheduler task history; right-click task → "View History" to see
  errors.
- *Process exits immediately* → May indicate configuration error or missing device endpoint; check
  event log or add `2>&1` to capture stderr.
- *Permission errors on database* → Ensure `C:\ProgramData\TptAvAutomation` is writable by the user
  running the task.

---

### Phase 6: Restart Resilience (Admin PowerShell Required)

Verify the watchdog restarts the engine if it crashes and that state persists.

```powershell
# 17. Force-kill the engine and verify watchdog restarts it
Get-Process tpt-av-automation | Stop-Process -Force

Start-Sleep -Seconds 3

Get-Process tpt-av-automation -ErrorAction SilentlyContinue
# Expected: Process exists again (watchdog restarted it)

# 18. Verify database survives restart
Get-ChildItem C:\ProgramData\TptAvAutomation\
# Expected: Same database files as before

# 19. Stop the scheduled task cleanly
Stop-ScheduledTask -TaskName "TPT AV Automation"

Start-Sleep -Seconds 2

Get-Process tpt-av-automation -ErrorAction SilentlyContinue
# Expected: No processes (task stopped, watchdog did not restart)
```

**Success Criteria:**
- [ ] Watchdog successfully restarts engine after force-kill
- [ ] Database files are intact and readable after restart
- [ ] Stopping the task cleanly exits all processes
- [ ] No corruption or data loss in state directory

**Common Failures and Fixes:**
- *Watchdog does not restart* → May indicate watchdog itself crashed or was not started by the task;
  check Task Scheduler history.
- *Database is corrupted* → Run SQLite integrity check on `automation.db`; if corrupt, delete and let
  engine recreate it on next start.

---

## Test Results

### Phase 1: Basic Verification
- [ ] --version runs without DLL errors
- [ ] --help displays usage text
- [ ] exe responds to commands
- [ ] **Verdict:** PASS / FAIL

### Phase 2: YAML Validation
- [ ] hello.yaml validates successfully
- [ ] live-event rules validate successfully (if included)
- [ ] **Verdict:** PASS / FAIL

### Phase 3: Simulation
- [ ] Schedule event simulation produces traces
- [ ] OSC event simulation produces traces
- [ ] **Verdict:** PASS / FAIL

### Phase 4: Service Installation
- [ ] install-windows-service.ps1 completes
- [ ] Task Scheduler task created
- [ ] State directory created at C:\ProgramData\TptAvAutomation
- [ ] **Verdict:** PASS / FAIL

### Phase 5: Runtime Verification
- [ ] Process starts and stays running
- [ ] Database file created
- [ ] History command returns executions (if API enabled)
- [ ] **Verdict:** PASS / FAIL

### Phase 6: Restart Resilience
- [ ] Watchdog restarts engine after kill
- [ ] Database survives restart
- [ ] Clean stop works correctly
- [ ] **Verdict:** PASS / FAIL

---

## Overall Result

- [ ] **PASS** — All checks passed. Ready for production deployment.
- [ ] **FAIL** — See "Issues Found" section below. Do not deploy until resolved.

---

## Issues Found

(Describe any failures, missing features, or unexpected behaviors encountered during testing.
Include error messages, stack traces, and reproduction steps.)

### Issue 1: [Title]
- **Phase:** [which phase]
- **Error:** [exact error message]
- **Reproduction:**
  ```powershell
  [steps to reproduce]
  ```
- **Root Cause:** [suspected cause]
- **Resolution:** [how to fix or workaround]

---

## System Information

- **OS Version:** [e.g., Windows 10 21H2]
- **PowerShell Version:** `$PSVersionTable.PSVersion`
- **CPU Architecture:** x86_64 (AMD64)
- **RAM:** [available memory]
- **Network:** [connected to internet, or offline?]
- **Antivirus/Security:** [software installed, if any]
- **Test Date:** [when this test was run]

---

## Recommendations

(Any improvements, deployment notes, or observations for future releases.)

- Ensure DLL dependencies are statically linked or bundled.
- Consider shipping a `.msi` installer for easier deployment in enterprise environments.
- Verify SmartScreen warnings on first run (Windows may warn about unsigned executables).
- Recommend minimum PowerShell 5.1 (included in Windows 10+).
- Ensure Task Scheduler task has appropriate permissions and retry settings.

---

## Sign-Off

- **Tested by:** [name or alias]
- **Date:** [YYYY-MM-DD]
- **Approved for release:** YES / NO
- **Approved by:** [project lead or release manager]

---

## References

- [Deployment Guide](deployment.md)
- [Installation Script](../scripts/install-windows-service.ps1)
- [Windows Package](../scripts/package-windows.ps1)
- [Reliability & Watchdog](reliability.md)
