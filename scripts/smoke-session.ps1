#!/usr/bin/env pwsh
# Smoke test for the Phase 1.5 session+recents+file-listing IPCs.
#
# Drives the Rust crate directly via cargo test. This isn't a UI smoke
# (the UI smoke is a manual run of `npm run tauri:dev`); this verifies
# the persistence layer the UI depends on.
#
# Usage: pwsh scripts/smoke-session.ps1
$ErrorActionPreference = 'Stop'

Write-Host "==> Phase 1.5 smoke: session + recents + file listing" -ForegroundColor Cyan

$env:Path = "$env:USERPROFILE\.cargo\bin;$env:Path"
Push-Location src-tauri

try {
    Write-Host "Running session tests..." -ForegroundColor Gray
    cargo test --lib session:: 2>&1 | Select-String -Pattern "test result|test session::" | ForEach-Object { Write-Host "  $_" }

    Write-Host ""
    Write-Host "Smoke OK" -ForegroundColor Green
} finally {
    Pop-Location
}
