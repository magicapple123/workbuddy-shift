# WorkBuddy Shift - start the dev preview
#
# Portable: every path is resolved relative to this file, so it works from any
# clone location and on any machine with Node.js + Rust installed.
#
# Sandboxed by default: the dev build runs against a copy of your data under
# $env:WB_DEV_SANDBOX (default D:\wb-shift-dev) instead of the real
# ~\.workbuddy-shift / ~\.workbuddy. Switching accounts, copying sessions and
# editing config inside the dev window then cannot touch your live setup.
# Set WB_DEV_REAL_HOME=1 to run against the real data instead.
#
# Why it closes the installed build first: tauri-plugin-single-instance is
# registered in both dev and release builds and keys off the app identifier,
# so a second instance would only focus the existing window and exit.
#
# Run via dev-start.cmd in the repo root, or directly:
#   powershell -ExecutionPolicy Bypass -File scripts\dev-start.ps1

$ErrorActionPreference = 'Stop'

$repo = Split-Path -Parent $PSScriptRoot
$appName = 'workbuddy-shift-desktop'   # Tauri binary name, see src-tauri/Cargo.toml

if (-not (Test-Path (Join-Path $repo 'package.json'))) {
    Write-Host "Not a WorkBuddy Shift checkout: $repo" -ForegroundColor Red
    exit 1
}

# --- toolchain -------------------------------------------------------------
# Prefer whatever is already on PATH; only fall back to common install dirs.
$extraPaths = @()

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    foreach ($candidate in @(
        (Join-Path $env:USERPROFILE '.cargo\bin'),
        'D:\Rust\cargo\bin',
        'C:\Program Files\Rust\cargo\bin'
    )) {
        if (Test-Path (Join-Path $candidate 'cargo.exe')) {
            $extraPaths += $candidate
            $cargoHome = Split-Path -Parent $candidate
            if (-not $env:CARGO_HOME -and (Test-Path (Join-Path $cargoHome 'registry'))) {
                $env:CARGO_HOME = $cargoHome
            }
            break
        }
    }
}

if (-not (Get-Command gcc -ErrorAction SilentlyContinue)) {
    foreach ($candidate in @('D:\mingw64\bin', 'C:\msys64\mingw64\bin', 'C:\mingw64\bin')) {
        if (Test-Path (Join-Path $candidate 'gcc.exe')) {
            $extraPaths += $candidate
            break
        }
    }
}

if (-not $env:RUSTUP_HOME) {
    foreach ($candidate in @('D:\Rust\rustup', (Join-Path $env:USERPROFILE '.rustup'))) {
        if (Test-Path $candidate) { $env:RUSTUP_HOME = $candidate; break }
    }
}

if ($extraPaths.Count -gt 0) {
    $env:Path = ($extraPaths -join ';') + ';' + $env:Path
}

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    Write-Host 'cargo not found. Install Rust from https://rustup.rs and retry.' -ForegroundColor Red
    exit 1
}
if (-not (Get-Command pnpm -ErrorAction SilentlyContinue)) {
    Write-Host 'pnpm not found. Install Node.js 22+, then run "corepack enable pnpm" (or "npm i -g pnpm") and retry.' -ForegroundColor Red
    exit 1
}

# --- sandbox ---------------------------------------------------------------
# The dev build redirects everything it resolves through home_dir() when
# WB_SHIFT_HOME is set: <sandbox>\.workbuddy-shift (accounts, backups, link
# store) and <sandbox>\.workbuddy (client database + session bodies). The real
# ones are never opened, so nothing done in the dev window can reach your live
# setup. Here we only choose the *location* (WB_DEV_SANDBOX) and seed it; the
# host turns it into WB_SHIFT_HOME itself, so a bare `pnpm run tauri dev` lands
# in the same kind of sandbox instead of your real data.
function Copy-Tree {
    param([string]$Source, [string]$Destination, [string]$Label)

    if (-not (Test-Path $Source)) {
        Write-Host "  skip $Label (source missing)" -ForegroundColor DarkGray
        return
    }
    New-Item -ItemType Directory -Path $Destination -Force | Out-Null
    $null = robocopy $Source $Destination /E /NFL /NDL /NJH /NJS /NP /R:1 /W:1
    # robocopy: 0-7 are success codes, 8+ are real failures.
    if ($LASTEXITCODE -ge 8) {
        throw "robocopy failed with code $LASTEXITCODE while copying $Label"
    }
}

if ($env:WB_DEV_REAL_HOME -eq '1') {
    Write-Host 'WB_DEV_REAL_HOME=1 - running against your REAL data (no sandbox).' -ForegroundColor Yellow
} else {
    $sandbox = if ($env:WB_DEV_SANDBOX) { $env:WB_DEV_SANDBOX } else { 'D:\wb-shift-dev' }

    if (-not (Test-Path $sandbox)) {
        New-Item -ItemType Directory -Path $sandbox -Force | Out-Null
        Write-Host "Created sandbox: $sandbox" -ForegroundColor Cyan
    }

    # Seed once. Later runs reuse whatever the dev build left behind, so you can
    # break things on purpose and keep the mess between sessions.
    $toolState = Join-Path $sandbox '.workbuddy-shift'
    if (-not (Test-Path (Join-Path $toolState 'accounts.json'))) {
        Write-Host 'Seeding sandbox with your accounts and config (one time)...' -ForegroundColor Cyan
        Copy-Tree (Join-Path $env:USERPROFILE '.workbuddy-shift') $toolState 'accounts/config'
    }

    $clientDb = Join-Path $sandbox '.workbuddy\workbuddy.db'
    if (-not (Test-Path $clientDb)) {
        Write-Host 'Seeding sandbox with client database and session bodies (one time, ~1.2 GB)...' -ForegroundColor Cyan
        $clientRoot = Join-Path $sandbox '.workbuddy'
        New-Item -ItemType Directory -Path $clientRoot -Force | Out-Null
        Copy-Item (Join-Path $env:USERPROFILE '.workbuddy\workbuddy.db') $clientRoot -Force -ErrorAction SilentlyContinue
        Copy-Tree (Join-Path $env:USERPROFILE '.workbuddy\projects') (Join-Path $clientRoot 'projects') 'session bodies'
    }

    # Export only the sandbox *location*; the dev build's own fallback turns it
    # into WB_SHIFT_HOME. This keeps a single dev data directory no matter how
    # the app is started (launcher or bare `pnpm run tauri dev`).
    # Clear any stale WB_SHIFT_HOME first: it would otherwise win over the
    # fallback and point somewhere else entirely.
    Remove-Item Env:WB_SHIFT_HOME -ErrorAction SilentlyContinue
    $env:WB_DEV_SANDBOX = $sandbox
    Write-Host "Sandbox: $sandbox" -ForegroundColor Green
    Write-Host 'Your real ~\.workbuddy-shift and ~\.workbuddy are not opened.' -ForegroundColor Green
}

# --- close the installed build --------------------------------------------
$release = Get-Process -Name $appName -ErrorAction SilentlyContinue |
    Where-Object { $_.Path -and ($_.Path -notlike "$repo\target\*") }

if ($release) {
    Write-Host 'Closing the installed build (single-instance lock)...' -ForegroundColor Yellow
    foreach ($p in $release) {
        try {
            Stop-Process -Id $p.Id -Force -ErrorAction Stop
            Write-Host "  closed pid $($p.Id)"
        } catch {
            Write-Host "  could not close pid $($p.Id): $($_.Exception.Message)" -ForegroundColor Red
        }
    }
    Start-Sleep -Seconds 2
}

$leftover = Get-Process -Name $appName -ErrorAction SilentlyContinue |
    Where-Object { $_.Path -like "$repo\target\*" }
if ($leftover) {
    Write-Host "A dev instance is already running (pid $($leftover.Id -join ', '))." -ForegroundColor Yellow
    Write-Host 'Run dev-stop.cmd first, or close that window.' -ForegroundColor Yellow
    exit 1
}

# --- start ----------------------------------------------------------------
Set-Location $repo
Write-Host ''
Write-Host 'Starting dev preview. First run compiles Rust (~1 min); later runs are fast.' -ForegroundColor Cyan
Write-Host 'Close this window or press Ctrl+C to stop.' -ForegroundColor Cyan
Write-Host ''
pnpm run tauri dev
