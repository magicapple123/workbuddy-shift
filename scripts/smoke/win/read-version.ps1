<#
.SYNOPSIS
读「已安装的 WorkBuddy Shift」的版本与主程序路径，输出一行 JSON。

.DESCRIPTION
Windows 上「装完的版本」有两个来源，本脚本**双读**并给出是否一致：

1. 卸载注册表 `DisplayVersion`（NSIS 写入，用户看到的「应用和功能」里的版本）；
2. `InstallLocation` 目录下主 exe 的 `VersionInfo.ProductVersion`（二进制自己带的版本）。

两者不一致时 `match=false`——driver 据此记 fail（H 类：版本不一致）。**不取平均值、不猜**。

注册表扫描三处（Tauri NSIS 默认 `currentUser` 装法写 HKCU；机器级安装写 HKLM 两处）：
`HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall`、
`HKLM:\Software\Microsoft\Windows\CurrentVersion\Uninstall`、
`HKLM:\Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall`。

.PARAMETER ProductName
卸载项的 `DisplayName`（Tauri 的 `productName`，本产品是 `WorkBuddy Shift`）。

.PARAMETER ExeName
优先匹配的主程序文件名；匹配不到时退回「安装目录里最大的 exe」。

.OUTPUTS
单行 JSON：`{found, displayName, displayVersion, installLocation, uninstallKey, exePath,
productVersion, fileVersion, match, registryHits, notes, error}`。
**永远 exit 0**：判定交给调用方（driver 记格子），helper 不制造「脚本崩了」的噪声。
#>
[CmdletBinding()]
param(
    [string]$ProductName = 'WorkBuddy Shift',
    # 实测的安装后主程序名（Tauri 用 crate 名而不是 productName）；匹配不到时按体积取最大的 exe。
    [string]$ExeName = 'workbuddy-shift-desktop.exe'
)

$ErrorActionPreference = 'Continue'

function Normalize-Version {
    param([string]$Value)
    if ([string]::IsNullOrWhiteSpace($Value)) { return '' }
    return ($Value.Trim() -replace '^[vV]', '').Trim()
}

function Normalize-Path {
    # NSIS 会把 InstallLocation 写成带引号的字符串（实测：`"D:\WorkBuddy Shift"`），
    # 直接 Test-Path 会失败，于是一条「已安装」的机器被读成「找不到主程序」。
    param([string]$Value)
    if ([string]::IsNullOrWhiteSpace($Value)) { return '' }
    return $Value.Trim().Trim('"').Trim("'").Trim()
}

function Get-UninstallRoots {
    return @(
        'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall',
        'HKLM:\Software\Microsoft\Windows\CurrentVersion\Uninstall',
        'HKLM:\Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall'
    )
}

# 1) 卸载注册表：找出所有 DisplayName 匹配的项（可能残留旧版本的多条）。
$registryHits = @()
foreach ($root in Get-UninstallRoots) {
    if (-not (Test-Path -LiteralPath $root)) { continue }
    $entries = @(Get-ChildItem -LiteralPath $root -ErrorAction SilentlyContinue)
    foreach ($entry in $entries) {
        $props = Get-ItemProperty -LiteralPath $entry.PSPath -ErrorAction SilentlyContinue
        if (-not $props) { continue }
        $displayName = [string]$props.DisplayName
        if ([string]::IsNullOrWhiteSpace($displayName)) { continue }
        if ($displayName -notlike "$ProductName*") { continue }
        $registryHits += [pscustomobject]@{
            displayName     = $displayName
            displayVersion  = Normalize-Version ([string]$props.DisplayVersion)
            installLocation = [string]$props.InstallLocation
            uninstallKey    = [string]$entry.PSPath
        }
    }
}

# 精确同名优先，其次按 displayVersion 降序（残留旧项不会盖住当前安装）。
$preferred = @($registryHits | Where-Object { $_.displayName -eq $ProductName })
if ($preferred.Count -eq 0) { $preferred = @($registryHits) }
$chosen = $preferred | Sort-Object -Property displayVersion -Descending | Select-Object -First 1

$installLocation = ''
if ($chosen) { $installLocation = Normalize-Path ([string]$chosen.installLocation) }

$notes = New-Object System.Collections.ArrayList
if ($registryHits.Count -eq 0) {
    [void]$notes.Add('注册表里没有 DisplayName 以 "' + $ProductName + '" 开头的卸载项')
}
elseif ($registryHits.Count -gt 1) {
    [void]$notes.Add('注册表命中 ' + $registryHits.Count + ' 条同名卸载项（可能是残留），取 displayVersion 最大的那条')
}

# 2) 主程序 exe：优先注册表的 InstallLocation，其次两个常见默认目录。
$searchDirs = New-Object System.Collections.ArrayList
if ($installLocation) { [void]$searchDirs.Add($installLocation) }
if ($env:LOCALAPPDATA) { [void]$searchDirs.Add((Join-Path $env:LOCALAPPDATA $ProductName)) }
if ($env:ProgramFiles) { [void]$searchDirs.Add((Join-Path $env:ProgramFiles $ProductName)) }

$exePath = ''
foreach ($dir in $searchDirs) {
    if ([string]::IsNullOrWhiteSpace($dir) -or -not (Test-Path -LiteralPath $dir)) { continue }
    $candidates = @(
        Get-ChildItem -LiteralPath $dir -Filter '*.exe' -File -ErrorAction SilentlyContinue |
            Where-Object { $_.Name -notmatch '^(unins|uninstall|updater)' }
    )
    if ($candidates.Count -eq 0) { continue }
    $exeHit = $candidates | Where-Object { $_.Name -ieq $ExeName } | Select-Object -First 1
    if (-not $exeHit) { $exeHit = $candidates | Sort-Object -Property Length -Descending | Select-Object -First 1 }
    $exePath = $exeHit.FullName
    if ($dir -ne $installLocation) {
        [void]$notes.Add('注册表 InstallLocation 为空或不可用，退回目录扫描：' + $dir)
    }
    break
}
if (-not $exePath) {
    [void]$notes.Add('安装目录里没找到主程序 exe（尝试过：' + ($searchDirs -join ' | ') + '）')
}

# 3) 读 exe 的版本资源。
$productVersion = ''
$fileVersion = ''
$errorText = ''
if ($exePath) {
    try {
        $info = (Get-Item -LiteralPath $exePath -ErrorAction Stop).VersionInfo
        $productVersion = Normalize-Version ([string]$info.ProductVersion)
        $fileVersion = Normalize-Version ([string]$info.FileVersion)
    }
    catch {
        $errorText = "读 exe 版本资源失败：$($_.Exception.Message)"
    }
}

$displayVersion = ''
$displayName = ''
$uninstallKey = ''
if ($chosen) {
    $displayVersion = [string]$chosen.displayVersion
    $displayName = [string]$chosen.displayName
    $uninstallKey = [string]$chosen.uninstallKey
}

$match = ($displayVersion -ne '') -and ($productVersion -ne '') -and ($displayVersion -eq $productVersion)
if ($displayVersion -ne '' -and $productVersion -ne '' -and -not $match) {
    [void]$notes.Add("注册表 DisplayVersion=$displayVersion 与 exe ProductVersion=$productVersion 不一致")
}

$result = [pscustomobject]@{
    found           = [bool]$chosen
    displayName     = $displayName
    displayVersion  = $displayVersion
    installLocation = $installLocation
    uninstallKey    = $uninstallKey
    exePath         = $exePath
    productVersion  = $productVersion
    fileVersion     = $fileVersion
    match           = [bool]$match
    registryHits    = $registryHits.Count
    notes           = @($notes)
    error           = $errorText
}

Write-Output ($result | ConvertTo-Json -Compress -Depth 4)
exit 0
