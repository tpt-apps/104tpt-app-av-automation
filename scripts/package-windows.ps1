<#
.SYNOPSIS
  Builds and packages the Windows x64 release of the TPT AV Automation CLI/service.

.DESCRIPTION
  Produces dist/tpt-av-automation-<version>-windows-x64.zip containing the executable, the example
  rule packs, the documentation and the licences, plus a .sha256 file. The executable is linked
  with a statically linked C runtime so it runs on a machine with no Visual C++ redistributable and
  no development tooling installed. It also has no network requirement: nothing in it opens an
  outbound connection.

  The packaged executable is smoke-tested from an empty directory before the archive is written.

.EXAMPLE
  ./scripts/package-windows.ps1 -Version 0.1.0
#>
param(
    [string]$Version = "0.1.0"
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

# Static CRT: no vcruntime140.dll dependency. A separate target dir keeps these flags from
# invalidating the normal development build cache.
$env:RUSTFLAGS = "-C target-feature=+crt-static"
cargo build --release -p tpt-app-av-automation-cli --target-dir target/package
if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }

$name = "tpt-av-automation-$Version-windows-x64"
$stage = Join-Path $root "dist/$name"
if (Test-Path $stage) { Remove-Item -Recurse -Force $stage }
New-Item -ItemType Directory -Force $stage | Out-Null

Copy-Item "target/package/release/tpt-av-automation.exe" $stage
foreach ($file in "README.md", "CHANGELOG.md", "LICENSE-MIT", "LICENSE-APACHE") {
    Copy-Item $file $stage
}
Copy-Item "scripts/install-windows-service.ps1" $stage
Copy-Item -Recurse "rules" (Join-Path $stage "rules")
Copy-Item -Recurse "docs" (Join-Path $stage "docs")

# Smoke test from an empty working directory: the package must work on its own.
$exe = Join-Path $stage "tpt-av-automation.exe"
$empty = Join-Path ([System.IO.Path]::GetTempPath()) ("tpt-av-smoke-" + [guid]::NewGuid())
New-Item -ItemType Directory -Force $empty | Out-Null
Push-Location $empty
try {
    & $exe --version
    if ($LASTEXITCODE -ne 0) { throw "--version failed" }
    & $exe validate --rules (Join-Path $stage "rules/live-event/main-hall-event-start.yaml") `
        --devices (Join-Path $stage "rules/examples/devices.yaml")
    if ($LASTEXITCODE -ne 0) { throw "validate failed (exit $LASTEXITCODE)" }
    & $exe simulate --rules (Join-Path $stage "rules/examples/hello.yaml") --event "osc:/cue/1=1"
    if ($LASTEXITCODE -ne 0) { throw "simulate failed (exit $LASTEXITCODE)" }
}
finally {
    Pop-Location
    Remove-Item -Recurse -Force $empty
}

$zip = Join-Path $root "dist/$name.zip"
if (Test-Path $zip) { Remove-Item -Force $zip }
Compress-Archive -Path $stage -DestinationPath $zip
$hash = (Get-FileHash -Algorithm SHA256 $zip).Hash.ToLower()
"$hash  $name.zip" | Set-Content -Encoding ascii "$zip.sha256"

Write-Host "Packaged $zip"
Write-Host "SHA-256  $hash"
