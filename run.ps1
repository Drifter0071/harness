# Harness Dev Server
# Starts tauri dev with cargo in PATH

$ErrorActionPreference = 'Stop'

# Ensure cargo is in PATH
$cargoBin = "$env:USERPROFILE\.cargo\bin"
$userPath = [System.Environment]::GetEnvironmentVariable("Path", "User")
$machinePath = [System.Environment]::GetEnvironmentVariable("Path", "Machine")
$env:PATH = "$cargoBin;$userPath;$machinePath"

# Verify cargo is available
if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    Write-Host "Error: cargo not found. Is Rust installed?" -ForegroundColor Red
    exit 1
}

Write-Host "Starting Harness dev server..." -ForegroundColor Cyan
Write-Host "Cargo: $(cargo --version)" -ForegroundColor DarkGray
Write-Host ""

npx tauri dev
