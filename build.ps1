#requires -Version 5.1
<#
build.ps1 - Optimized production build for Harness (single portable exe, < 5 MB).

What it does:
  1. Repairs Cargo.toml size optimizations if missing (idempotent, no-op on
     healthy trees):
       - tauri "compression" feature  -> brotli-compresses the embedded assets
       - [profile.release]            -> opt-level "z", lto "fat",
                                          codegen-units 1, panic "abort",
                                          strip "symbols", incremental false
  2. Builds the frontend (tsc + vite build, production-minified). A smaller
     JS bundle means less WebView2 memory at runtime - the biggest RAM lever
     that exists at build time.
  3. Builds the Rust app directly with `cargo build --release` (avoids tauri's
     redundant beforeBuildCommand re-run; identical output exe).
  4. Copies the single exe to .\release\harness.exe and reports its size
     against the target.

Why not `tauri build`? It re-runs `npm run build` internally and then shells
out installers (MSI/NSIS) that push way past 5 MB. We build the frontend
once ourselves and produce only the bare exe. Pass -KeepInstallers if you
actually want the full installer set.

RAM notes: process memory is dominated by the system WebView2 runtime
(shared, fixed cost) plus the JS the page loads. Keeping the bundle lean is
the real lever; tokio's runtime size is compile-time only, and its worker
thread count is the CPU count at runtime (not configurable from here).

Usage:
  .\build.ps1               full optimized build -> .\release\harness.exe
  .\build.ps1 -SkipFrontend reuse existing dist/ (frontend already built)
  .\build.ps1 -SkipInstall  skip npm install if node_modules exists
  .\build.ps1 -SkipRepair   never touch Cargo.toml
  .\build.ps1 -KeepInstallers  also produce MSI/NSIS installers
  .\build.ps1 -Check        verify environment + Cargo.toml, then exit
  .\build.ps1 -CleanAssetCache  clear tauri-codegen-assets cache (use after
                                 toggling the tauri `compression` feature, or
                                 if the asset cache served stale raw assets
                                 from a prior build)
  .\build.ps1 -TargetMb 6   set a different size target
#>
[CmdletBinding()]
param(
    [switch]$SkipFrontend,
    [switch]$SkipInstall,
    [switch]$SkipRepair,
    [switch]$KeepInstallers,
    [switch]$Check,
    [switch]$CleanAssetCache,
    [ValidateRange(1, 100)]
    [int]$TargetMb = 5
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$Root        = $PSScriptRoot
$CargoDir    = Join-Path $Root 'src-tauri'
$CargoToml   = Join-Path $CargoDir 'Cargo.toml'
$FrontendDir = Join-Path $Root 'dist'
$ExeSource   = Join-Path $CargoDir 'target\release\harness.exe'
$ExeOutput   = Join-Path $Root 'release\harness.exe'

function Write-Step  { param([string]$Message) Write-Host "`n==> $Message" -ForegroundColor Cyan }
function Write-Info  { param([string]$Message) Write-Host "    $Message" -ForegroundColor DarkGray }
function Write-Ok    { param([string]$Message) Write-Host "    [ok] $Message" -ForegroundColor Green }
function Write-Fix   { param([string]$Message) Write-Host "    [fix] $Message" -ForegroundColor Yellow }
function Write-Warn  { param([string]$Message) Write-Host "    [warn] $Message" -ForegroundColor Yellow }

function Invoke-Native {
    param([string]$Label, [string]$File, [string[]]$Arguments)
    Write-Info "$File $($Arguments -join ' ')"
    & $File @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "Step failed: $Label (exit code $LASTEXITCODE)"
    }
}

function Assert-Command {
    param([string]$Name, [string]$Hint)
    if (-not (Get-Command $Name -ErrorAction SilentlyContinue)) {
        throw "$Name not found. $Hint"
    }
}

function Add-CargoToPath {
    # Same pattern as run.ps1: cargo is installed under the user profile and
    # is frequently missing from the interactive PATH.
    $cargoBin = Join-Path $env:USERPROFILE '.cargo\bin'
    if ((Test-Path (Join-Path $cargoBin 'cargo.exe')) -and ($env:PATH -notlike "*$cargoBin*")) {
        $env:PATH = "$cargoBin;$env:PATH"
    }
}

function Repair-CargoOptimizations {
    # Idempotent safety net: makes sure the size-critical Cargo settings exist
    # even on a fresh clone. Only touches Cargo.toml when something is missing.
    if ($SkipRepair) {
        Write-Info 'Cargo.toml left untouched (-SkipRepair).'
        return
    }
    if (-not (Test-Path -PathType Leaf $CargoToml)) {
        throw "Cargo.toml not found at $CargoToml"
    }

    $text = Get-Content -Raw -Encoding UTF8 $CargoToml
    $changed = $false
    $utf8NoBom = New-Object System.Text.UTF8Encoding($false)

    # --- 1) tauri "compression" feature -------------------------------------
    # Enables runtime brotli decompression of the embedded assets. The build-
    # time compression itself runs inside `tauri-codegen`, which is wired up by
    # `tauri-build`'s `codegen` feature; the `tauri` `compression` feature is
    # what pulls in the `brotli` dep for the codegen step. We do NOT touch
    # `tauri-build`'s feature set here because tauri-build 2.6.x (the version
    # locked by Cargo.lock) does not expose a `compression` feature of its own.
    if ($text -notmatch 'tauri\s*=\s*\{[^}]*features\s*=\s*\[[^\]]*compression') {
        $pattern = '(tauri\s*=\s*\{[^}]*?features\s*=\s*\[)([^\]]*?)(\]\s*\})'
        if ($text -match $pattern) {
            $existing = $matches[2].Trim()
            $insert = if ($existing -eq '') { '"compression"' } else { "$existing, `"compression`"" }
            $text = $text -replace $pattern, "`$1$insert`$3"
            $changed = $true
            Write-Fix 'enabled tauri "compression" feature (brotli-compresses embedded assets at build, decompresses at runtime).'
        } else {
            Write-Warn 'could not locate the tauri dependency line; skipping compression patch.'
        }
    }

    # --- 2) [profile.release] -----------------------------------------------
    $profileKeys = @{
        'opt-level'    = '"z"'
        'lto'          = '"fat"'
        'codegen-units' = '1'
        'panic'        = '"abort"'
        'strip'        = '"symbols"'
        'incremental'  = 'false'
    }

    if ($text -notmatch '(?m)^\[profile\.release\]\s*$') {
        $text += @'

[profile.release]
opt-level = "z"
lto = "fat"
codegen-units = 1
panic = "abort"
strip = "symbols"
incremental = false
'@
        $changed = $true
        Write-Fix 'added [profile.release] size-optimized profile.'
    } else {
        $m = [regex]::Match($text, '(?ms)^\[profile\.release\][^\[]*')
        if ($m.Success) {
            $section = $m.Value
            $newSection = $section
            foreach ($key in $profileKeys.Keys) {
                if ($newSection -notmatch "(?m)^\s*$([regex]::Escape($key))\s*=") {
                    $newSection += "`n$key = $($profileKeys[$key])"
                }
            }
            if ($newSection -ne $section) {
                $text = $text.Replace($section, $newSection)
                $changed = $true
                Write-Fix 'added missing keys to [profile.release].'
            }
        }
    }

    if ($changed) {
        [System.IO.File]::WriteAllText($CargoToml, $text, $utf8NoBom)
        Write-Ok 'Cargo.toml updated (UTF-8, no BOM).'
    } else {
        Write-Ok 'Cargo.toml already fully optimized.'
    }
}

# ============================================================================
Write-Host "`nHarness optimized build" -ForegroundColor White
Write-Host ("Target: single exe <= {0} MB, min RAM footprint`n" -f $TargetMb) -ForegroundColor DarkGray

Add-CargoToPath
Assert-Command 'cargo' 'Install the Rust toolchain via rustup, then reopen the terminal.'
Assert-Command 'node'  'Install Node.js from https://nodejs.org/'
Assert-Command 'npm'   'npm should ship with Node.js.'

Repair-CargoOptimizations

if ($Check) {
    Write-Step 'Environment check'
    Write-Ok 'node, npm and cargo are all available; Cargo.toml is optimized.'
    Write-Ok 'Nothing built (-Check). Run .\build.ps1 without flags for the full build.'
    exit 0
}

# --- Optional: clear stale asset cache --------------------------------------
# tauri-codegen caches compressed assets at OUT_DIR\tauri-codegen-assets and
# only re-compresses when the content hash changes. If the `compression`
# feature was toggled, the cache holds the old (raw) blobs and the next
# cargo build won't refresh them. The moved cache dirs end up in $env:TEMP
# (manual recovery if needed).
if ($CleanAssetCache) {
    Write-Step 'Cleaning tauri-codegen-assets cache'
    $dirs = Get-ChildItem (Join-Path $CargoDir 'target\release\build') -Directory -ErrorAction SilentlyContinue |
        Where-Object { $_.Name -match '^harness-' } |
        ForEach-Object { Join-Path $_.FullName 'out\tauri-codegen-assets' } |
        Where-Object { Test-Path $_ }
    if ($dirs) {
        foreach ($d in $dirs) {
            $count = (Get-ChildItem $d -File -ErrorAction SilentlyContinue).Count
            $size  = (Get-ChildItem $d -File -ErrorAction SilentlyContinue | Measure-Object Length -Sum).Sum
            Write-Info ("move to TEMP: {0}  ({1} files, {2:N2} MB)" -f $d, $count, ($size/1MB))
            $stamped = Join-Path $env:TEMP ("harness-cache-{0}-{1}" -f [Guid]::NewGuid(), (Split-Path $d -Leaf))
            Move-Item -LiteralPath $d -Destination $stamped -Force
        }
        Write-Ok 'cache moved to $env:TEMP; restore from there if needed.'
    } else {
        Write-Info 'no asset cache to clean'
    }
}

# --- Frontend ---------------------------------------------------------------
if (-not $SkipFrontend) {
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    Write-Step 'Building frontend (tsc + vite build)'
    if (-not (Test-Path (Join-Path $Root 'node_modules'))) {
        if ($SkipInstall) {
            throw 'node_modules is missing and -SkipInstall was passed. Run without -SkipInstall first.'
        }
        Write-Info 'node_modules missing -> npm install'
        Invoke-Native 'npm install' 'npm' @('install')
    }
    Invoke-Native 'frontend build' 'npm' @('run', 'build')
    Write-Ok ("frontend built in {0:N1}s" -f $sw.Elapsed.TotalSeconds)
} else {
    if (-not (Test-Path (Join-Path $FrontendDir 'index.html'))) {
        throw 'dist/ is missing or stale - run without -SkipFrontend at least once.'
    }
    Write-Info 'Reusing existing dist/ (-SkipFrontend).'
}

# --- Rust -------------------------------------------------------------------
$sw = [System.Diagnostics.Stopwatch]::StartNew()
if ($KeepInstallers) {
    Write-Step 'Building with installers (npx tauri build)'
    Invoke-Native 'tauri build' 'npx' @('tauri', 'build')
} else {
    Write-Step 'Building single exe (cargo build --release)'
    Push-Location $CargoDir
    try {
        Invoke-Native 'cargo release build' 'cargo' @('build', '--release')
    } finally {
        Pop-Location
    }
}
Write-Ok ("Rust build finished in {0:N1}s" -f $sw.Elapsed.TotalSeconds)

if (-not (Test-Path -PathType Leaf $ExeSource)) {
    throw "Build finished but exe not found at $ExeSource"
}

# --- Deliverable ------------------------------------------------------------
New-Item -ItemType Directory -Force -Path (Split-Path $ExeOutput -Parent) | Out-Null
Copy-Item -LiteralPath $ExeSource -Destination $ExeOutput -Force

$bytes = (Get-Item -LiteralPath $ExeOutput).Length
$mb = [Math]::Round($bytes / 1MB, 2)
$distBytes = if (Test-Path $FrontendDir) {
    (Get-ChildItem $FrontendDir -Recurse -File | Measure-Object Length -Sum).Sum
} else { 0 }
$distMb = [Math]::Round($distBytes / 1MB, 2)

Write-Step 'Result'
Write-Host ("    {0}  ->  {1}" -f $ExeOutput, $(if ($bytes -gt 0) { '{0:N0} bytes ({1} MB)' -f $bytes, $mb } else { 'empty' })) -ForegroundColor White
Write-Info "Embedded frontend dist/: $distMb MB on disk"

# Honest compression report: look in the build script OUT_DIR for the cached
# asset files. Non-ASCII byte ratio is the cleanest signal: raw JS is ~0-6%
# non-ASCII; brotli q9 is ~50%. The cache total matches the linked-in size.
$assetCache = Get-ChildItem (Join-Path $CargoDir 'target\release\build') -Directory -ErrorAction SilentlyContinue |
    Where-Object { $_.Name -match '^harness-' } |
    ForEach-Object { Get-ChildItem (Join-Path $_.FullName 'out\tauri-codegen-assets') -File -ErrorAction SilentlyContinue }
if ($assetCache) {
    $cacheTotal = ($assetCache | Measure-Object Length -Sum).Sum
    $sample = $assetCache | Sort-Object Length -Descending | Select-Object -First 1
    $sampleBytes = [System.IO.File]::ReadAllBytes($sample.FullName)
    $nonAscii = 0
    for ($i = 0; $i -lt $sampleBytes.Length; $i++) { if ($sampleBytes[$i] -gt 127) { $nonAscii++ } }
    $pct = [Math]::Round(100.0 * $nonAscii / $sampleBytes.Length, 1)
    $embeddedMb = [Math]::Round($cacheTotal / 1MB, 2)
    if ($pct -gt 30) {
        Write-Info ("Brotli ON: $distMb MB raw -> $embeddedMb MB embedded (largest sample $pct% high-bit entropy)")
    } else {
        Write-Warn ("Brotli appears OFF: largest sample is only $pct% high-bit (raw JS is ~0-6%); tauri `compression` feature may not be active")
    }
}
Write-Info "RAM lever: keep the JS bundle lean - WebView2 memory scales with loaded script bytes."

if ($mb -le $TargetMb) {
    Write-Host "`n    PASS: under the $TargetMb MB target." -ForegroundColor Green
} else {
    Write-Host "`n    OVER: $mb MB exceeds the $TargetMb MB target." -ForegroundColor Red
    Write-Host '    Next levers, in order of impact:' -ForegroundColor Yellow
    Write-Host '      1. Drop unused heavy deps in package.json (mermaid, katex, cytoscape, marked, react-file-icon)'
    Write-Host '      2. Move code-split chunks behind dynamic import() so Vite can tree-shake harder'
    Write-Host '      3. Reduce Rust deps: reqwest -> tiny-http, tokio worker count, drop notify/keyring'
}

exit 0
