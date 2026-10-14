# Profile the sustained_event_load test with samply to generate a flame graph.
#
# Requirements:
#   - Rust toolchain installed
#   - samply installed: `cargo install samply`
#
# Usage:
#   .\scripts\profile-flame-graph.ps1

$ErrorActionPreference = "Stop"

Write-Host "Building profiling binary..." -ForegroundColor Green
cargo build --profile perf `
    -p tpt-app-av-automation-test `
    --test chaos

if ($LASTEXITCODE -ne 0) {
    Write-Host "Build failed" -ForegroundColor Red
    exit 1
}

$testExe = Get-Item ".\target\perf\deps\chaos-*.exe" | Select-Object -First 1
if (-not $testExe) {
    Write-Host "Test executable not found in target\perf\deps\" -ForegroundColor Red
    exit 1
}

Write-Host "Found test binary: $($testExe.FullName)" -ForegroundColor Green
Write-Host ""
Write-Host "Running profiler (samply)..." -ForegroundColor Green
Write-Host "This will record CPU stack samples during the sustained_event_load test." -ForegroundColor Gray
Write-Host ""

samply record `
    "$($testExe.FullName)" `
    --nocapture `
    --test-threads=1 `
    sustained_event_load_is_processed_and_history_stays_bounded

if ($LASTEXITCODE -ne 0) {
    Write-Host "Profiler failed. Make sure samply is installed: cargo install samply" -ForegroundColor Red
    exit 1
}

Write-Host ""
Write-Host "Profiling complete!" -ForegroundColor Green
Write-Host "Flame graph: samply.html (in current directory)" -ForegroundColor Green
Write-Host "Open in your web browser to analyze CPU hotspots." -ForegroundColor Green
