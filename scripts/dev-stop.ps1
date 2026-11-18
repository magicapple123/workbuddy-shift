# WorkBuddy Shift - stop dev preview
#
# Kills only what belongs to this repo:
#   - the dev app (workbuddy-shift-desktop.exe under <repo>\target\...)
#   - node / cargo / rustc processes whose command line references the repo
# It never touches the installed release build or unrelated terminals.

$ErrorActionPreference = 'SilentlyContinue'
$repo = Split-Path -Parent $PSScriptRoot
$appName = 'workbuddy-shift-desktop'   # Tauri binary name, see src-tauri/Cargo.toml
$killed = 0

# 1. Dev app binaries (debug or release builds inside the repo target dir).
Get-Process -Name $appName -ErrorAction SilentlyContinue |
    Where-Object { $_.Path -like "$repo\target\*" } |
    ForEach-Object {
        Write-Host "closing dev app pid $($_.Id)"
        Stop-Process -Id $_.Id -Force
        $killed++
    }

# 2. Toolchain / bundler processes tied to this repo.
#
# Deliberately narrow: only node processes that are actually running the dev
# server (vite / tauri). Matching on the repo path alone is too broad - it also
# catches editor language servers, test runners and unrelated npm tasks whose
# command line happens to mention this directory.
$devMarkers = @('vite', '@tauri-apps', 'tauri')
$procs = Get-CimInstance Win32_Process -ErrorAction SilentlyContinue | Where-Object {
    $cmd = $_.CommandLine
    if (-not $cmd -or $cmd -notlike "*$repo*") { return $false }
    if ($_.Name -in @('cargo.exe', 'rustc.exe', 'tauri.exe')) { return $true }
    if ($_.Name -eq 'node.exe') {
        foreach ($marker in $devMarkers) {
            if ($cmd -like "*$marker*") { return $true }
        }
    }
    return $false
}

foreach ($p in $procs) {
    Write-Host "closing $($p.Name) pid $($p.ProcessId)"
    Stop-Process -Id $p.ProcessId -Force
    $killed++
}

if ($killed -eq 0) {
    Write-Host "Nothing to stop - no dev preview is running." -ForegroundColor Yellow
} else {
    Write-Host "Stopped $killed process(es)." -ForegroundColor Green
}

Write-Host ''
# Pause only when launched from the desktop .cmd (it sets WB_DEV_STOP_PAUSE);
# when called from other tools the script must not block.
if ($env:WB_DEV_STOP_PAUSE -eq '1') {
    Write-Host "Press any key to close this window."
    try { $null = $Host.UI.RawUI.ReadKey('NoEcho,IncludeKeyDown') } catch { }
}
