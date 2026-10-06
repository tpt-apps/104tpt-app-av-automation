<#
.SYNOPSIS
  Registers the TPT AV Automation watchdog as a Windows scheduled task that starts at boot.

.DESCRIPTION
  Uses the Task Scheduler (no extra tooling, no native service wrapper). The task runs as SYSTEM
  whether or not anyone is logged on, restarts after failure, and runs the watchdog, which in turn
  supervises the engine. Run from an elevated PowerShell. Use -Uninstall to remove it.

.EXAMPLE
  ./install-windows-service.ps1 -Rules C:\show\main-hall.yaml -Devices C:\show\devices.yaml
#>
param(
    [string]$Rules,
    [string]$Devices,
    [string]$Config,
    [string]$StateDir = "$env:ProgramData\TptAvAutomation",
    [string]$Exe = (Join-Path $PSScriptRoot "tpt-av-automation.exe"),
    [string]$TaskName = "TPT AV Automation",
    [switch]$Uninstall
)

$ErrorActionPreference = "Stop"

$principal = [Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw "Run this script from an elevated (Administrator) PowerShell."
}

if ($Uninstall) {
    if (Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue) {
        Stop-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
        Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false
        Write-Host "Removed task '$TaskName'. State in $StateDir was left in place."
    } else {
        Write-Host "Task '$TaskName' is not installed."
    }
    return
}

if (-not $Rules -or -not $Devices) { throw "-Rules and -Devices are required." }
foreach ($path in $Exe, $Rules, $Devices) {
    if (-not (Test-Path $path)) { throw "Not found: $path" }
}
if ($Config -and -not (Test-Path $Config)) { throw "Not found: $Config" }

New-Item -ItemType Directory -Force $StateDir | Out-Null

$arguments = "watchdog -- run --service --rules `"$Rules`" --devices `"$Devices`" --state-dir `"$StateDir`""
if ($Config) { $arguments += " --config `"$Config`"" }

$action = New-ScheduledTaskAction -Execute (Resolve-Path $Exe) -Argument $arguments -WorkingDirectory $StateDir
$trigger = New-ScheduledTaskTrigger -AtStartup
$principalSpec = New-ScheduledTaskPrincipal -UserId "SYSTEM" -LogonType ServiceAccount -RunLevel Highest
$settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries `
    -StartWhenAvailable -RestartCount 999 -RestartInterval (New-TimeSpan -Minutes 1) `
    -ExecutionTimeLimit ([TimeSpan]::Zero) -MultipleInstances IgnoreNew

Register-ScheduledTask -TaskName $TaskName -Action $action -Trigger $trigger `
    -Principal $principalSpec -Settings $settings -Force | Out-Null

Write-Host "Installed task '$TaskName' (starts at boot as SYSTEM)."
Write-Host "Start now:  Start-ScheduledTask -TaskName '$TaskName'"
Write-Host "Remove:     ./install-windows-service.ps1 -Uninstall"
