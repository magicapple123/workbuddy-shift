<#
.SYNOPSIS
Windows 桌面形态的发版后验收驱动（设计 §5.1 的 W1–W12）。

.DESCRIPTION
真跑一次「应用内升级」：装 0.2.4 → 用 `--hidden --smoke-self-update` 启动它 →
它自己走 `check → download → minisign 校验 → update.install` → NSIS 静默升级到 0.2.5 →
driver 用「装完后的版本号」作证，并用 `assert-update-log.py` 断言过程日志的阶段序列。

硬约定（设计 §2.2 / §9.2）：

- **本脚本恒 exit 0**。每个断言通过 `common.py cell` 写进
  `artifacts/release-smoke/windows.json`，判定交给 workflow 的 Gate 步骤
  （`render-conclusion.py --gate`）。任何异常都必须落成 `fail` 格子，**不能让脚本崩**——
  崩了矩阵就缺格，缺格比失败更难查。
- 结果 JSON 的字段名/白名单只在 `scripts/smoke/common.py` 一份，本文件不自己拼 JSON。
- 沙箱：每次启动都换一个新的 `WB_SHIFT_HOME=<临时空目录>`，不碰真实用户数据。
- 退出码**不作通过判定**：NSIS 会在 `Update::install` 内部结束进程，观察到的退出码不可靠
  （只有失败归类时才看它）。

四个已知坑（上一轮踩过的，勿回退）
----------------------------------
1. 断言日志要**按 pid 分组取第一个进程**：NSIS 装完可能自动拉起新版本进程，它也会写日志；
2. `url` 是**发布页**不是安装包直链，`download_done` 的 detail 只有 `percent: 100`；
3. `WB_SHIFT_SMOKE_LOG` 必须是**绝对路径**（相对路径会被 App 的工作目录解释成别处）；
4. Windows 退出码不可依赖（见上）。

一个时序约束（不是缺陷，勿「修」成 fail）
------------------------------------------
`--smoke-self-update` 钩子自 **0.2.6** 才有。上一版低于它时，旧二进制只会当普通启动
（不写日志、不升级），driver 只能干等安装超时（实测 9 分钟）。此时升级 / 自更新日志 /
升级后版本三格标 `skipped`、**不启动升级进程**；启动三格照常跑。**从 0.2.7 起**（上一版
0.2.6 带钩子）才真正有效。

.PARAMETER Version
目标版本（0.2.5，可带 v 前缀）。

.PARAMETER PrevVersion
上一版版本（0.2.4）。为空时升级格记 `skipped`（首次发版）。

.PARAMETER Out
结果 JSON 路径，默认 `<repo>/artifacts/release-smoke/windows.json`。

.PARAMETER Repo
`owner/repo`，默认读 `$env:GH_REPO` / `$env:GITHUB_REPOSITORY`。

.EXAMPLE
pwsh -NoProfile -File scripts/smoke/win/driver.ps1 -Version 0.2.5 -PrevVersion 0.2.4
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$Version,
    [string]$PrevVersion = '',
    [string]$Out = '',
    [string]$Repo = '',
    [int]$InstallTimeoutSec = 480,
    [int]$StartupAliveSec = 20,
    [int]$SingleInstanceTimeoutSec = 15
)

# 任何一条断言失败都不许中断脚本：判定在 Gate，记录在本脚本。
$ErrorActionPreference = 'Continue'
$ProgressPreference = 'SilentlyContinue'

$VersionNorm = ($Version.Trim() -replace '^[vV]', '').Trim()
$PrevNorm = ''
if (-not [string]::IsNullOrWhiteSpace($PrevVersion)) {
    $PrevNorm = ($PrevVersion.Trim() -replace '^[vV]', '').Trim()
}

# 首个包含 `--smoke-self-update` 钩子的版本（设计 §3.1 的 `update_service/smoke.rs` 随 0.2.6
# 才进来）。上一版低于它 → 旧二进制不认这个参数，应用内升级**无法自验**（见 §3 的闸门）。
# 语义化比较用 common.ps1 的 Compare-VersionNumber（按 `.` 拆数字，不能字符串比大小）。
$SmokeHookSince = '0.2.6'

$script:FailCount = 0
$script:StepNo = 0
# 跨步骤共享的状态先显式置空：PowerShell 不会因为「引用未定义变量」报错，
# 但预置能让「这一步到底有没有跑过」一眼可见。
$script:Installed = $null
$script:PrevExe = $null
$script:CurrExe = $null
$script:InstallOnly = $false
$script:StartupPid = 0
$script:StartupSandbox = ''
$script:UpdateLog = ''

# --- 路径与工具 -------------------------------------------------------------
$RepoRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..\..\..')).Path
$CommonPy = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..\common.py')).Path
$AssertLog = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..\assert-update-log.py')).Path

$ResultPath = $Out
if ([string]::IsNullOrWhiteSpace($ResultPath)) {
    $ResultPath = Join-Path $RepoRoot 'artifacts\release-smoke\windows.json'
}
if (-not [System.IO.Path]::IsPathRooted($ResultPath)) {
    $ResultPath = Join-Path (Get-Location).Path $ResultPath
}
$ResultParent = Split-Path -Parent $ResultPath
if ($ResultParent -and -not (Test-Path -LiteralPath $ResultParent)) {
    New-Item -ItemType Directory -Force -Path $ResultParent | Out-Null
}

$WorkBase = $env:RUNNER_TEMP
if ([string]::IsNullOrWhiteSpace($WorkBase)) { $WorkBase = [System.IO.Path]::GetTempPath() }
$WorkDir = Join-Path $WorkBase ('release-smoke-win-' + ([guid]::NewGuid().ToString('N').Substring(0, 8)))
New-Item -ItemType Directory -Force -Path $WorkDir | Out-Null
# 证据落在**结果 JSON 旁边**（`artifacts/release-smoke/evidence/windows/`）：workflow 上传的是
# 仓库内的 `path: artifacts/`，放 RUNNER_TEMP 的日志从来没进过 artifact——多个 detail 写着
# 「见 xxx.log」，artifact 里却只有一份 JSON（run 44941972411 的现场）。
$EvidenceDir = Join-Path (Split-Path -Parent $ResultPath) 'evidence\windows'
New-Item -ItemType Directory -Force -Path $EvidenceDir | Out-Null

if ([string]::IsNullOrWhiteSpace($Repo)) {
    if ($env:GH_REPO) { $Repo = $env:GH_REPO }
    elseif ($env:GITHUB_REPOSITORY) { $Repo = $env:GITHUB_REPOSITORY }
}

# --- 公共库：scripts/smoke/common.ps1（与 common.sh 同构，dot-source 共享 script 作用域） ---
# 设计 §10.1 把这层定位为「进程 / 版本 / 单实例 / 环境判定」的公共实现；
# 本 driver 只保留 Windows 的**平台流程**（W1–W12），不重复实现公共函数。
. (Join-Path $PSScriptRoot '..\common.ps1')

$Py = Resolve-SmokePython
if (-not $Py) {
    Write-Output '::error::缺少 python，结果契约无法写出（流水线缺陷，不是产品问题）'
    exit 0
}

# 让结果 JSON 的 env.sessionId 落到文件里（D7）：Windows 判 BLOCKED(J) 需要「SessionId == 0」的证据。
$env:SMOKE_SESSION_ID = "$(try { (Get-Process -Id $PID).SessionId } catch { '' })"

# ===========================================================================
# 0) 建结果（driver 第一步；之后每步只 append）
# ===========================================================================
$emitArgs = @($CommonPy, 'emit', '--form', 'windows', '--version', $VersionNorm, '--out', $ResultPath, '--upgrade-mode', 'in-app')
if ($PrevNorm) { $emitArgs += @('--prev-version', $PrevNorm) }
& $Py @emitArgs | Write-Output

Write-Output "::group::环境"
Write-Output ("repo={0} version={1} prev={2} workdir={3}" -f $Repo, $VersionNorm, $(if ($PrevNorm) { $PrevNorm } else { '—' }), $WorkDir)
Write-Output ((Get-CimInstance Win32_OperatingSystem | Select-Object Caption, Version, OSArchitecture | Format-List | Out-String))
Write-Output ("sessionId={0}（0 = Session 0，桌面 App 可能起不来）" -f (Get-Process -Id $PID).SessionId)
Write-Output '::endgroup::'

# ===========================================================================
# 1) 取资产：旧版 + 新版安装包（gh release download），校验 sha256（可选）
# ===========================================================================
$DownloadDir = Join-Path $WorkDir 'assets'
$PrevExe = $null
$CurrExe = $null

Invoke-Guarded -Action 'install' -Assertion "gh release download v$PrevNorm 的 *_x64-setup.exe 成功" -Category 'D' -Body {
    if (-not $PrevNorm) {
        Add-Cell -Action 'install' -Status 'skipped' -Assertion '取旧版安装包' -Detail '没有上一版（首次发版）'
        return
    }
    $asset = Save-ReleaseAsset -Tag "v$PrevNorm" -Version $PrevNorm `
        -Dir (Join-Path $DownloadDir "prev-$PrevNorm") -EvidenceName "sha256-prev-$PrevNorm.txt"
    if (-not $asset.Ok) {
        Add-Cell -Action 'install' -Status 'fail' -Assertion "gh release download v$PrevNorm 的 x64-setup.exe" `
            -Category 'D' -Detail $asset.Error
        return
    }
    $script:PrevExe = $asset.Path
    if ($asset.Mismatch) {
        Add-Cell -Action 'install' -Status 'fail' -Assertion "上一版安装包 sha256 与 Release digest 一致" `
            -Category 'D' -Detail $asset.Error -Evidence @($asset.Evidence)
        return
    }
    $note = if ($asset.Digest) { 'sha256 与 Release digest 一致' } else { 'Release 未提供 digest，跳过哈希比对（设计 W1：可选）' }
    Add-Cell -Action 'install' -Status 'pass' -Assertion "取到旧版安装包 $($asset.Name)（$note）" `
        -Detail "size=$($asset.Length)" -Evidence @($asset.Evidence)
}
$PrevExe = $script:PrevExe

Invoke-Guarded -Action 'install' -Assertion "gh release download v$VersionNorm 的 *_x64-setup.exe 成功" -Category 'D' -Body {
    $script:CurrExe = $null
    $asset = Save-ReleaseAsset -Tag "v$VersionNorm" -Version $VersionNorm `
        -Dir (Join-Path $DownloadDir "curr-$VersionNorm") -EvidenceName "sha256-$VersionNorm.txt"
    if (-not $asset.Ok) {
        Add-Cell -Action 'install' -Status 'fail' -Assertion "gh release download v$VersionNorm 的 x64-setup.exe" `
            -Category 'D' -Detail $asset.Error
        return
    }
    $script:CurrExe = $asset.Path
    if ($asset.Mismatch) {
        Add-Cell -Action 'install' -Status 'fail' -Assertion "$VersionNorm 安装包 sha256 与 Release digest 一致" `
            -Category 'D' -Detail $asset.Error -Evidence @($asset.Evidence)
        return
    }
    $note = if ($asset.Digest) { 'sha256 与 Release digest 一致' } else { 'Release 未提供 digest，跳过哈希比对（可选）' }
    Add-Cell -Action 'install' -Status 'pass' -Assertion "取到 $VersionNorm 安装包 $($asset.Name)（$note）" `
        -Detail "size=$($asset.Length)" -Evidence @($asset.Evidence)
}
$CurrExe = $script:CurrExe

# ===========================================================================
# 2) 装旧版（/S 静默），双读断言版本 = 上一版
# ===========================================================================
$Installed = $null

Invoke-Guarded -Action 'install' -Assertion "0.2.4 静默安装成功且版本双读一致" -Category 'E' -Body {
    if (-not $PrevExe) {
        Add-Cell -Action 'install' -Status 'skipped' -Assertion '安装上一版' -Detail '没有上一版安装包'
        return
    }
    Write-Output "安装：$PrevExe /S"
    $proc = Start-Process -FilePath $PrevExe -ArgumentList '/S' -PassThru -Wait
    $code = $proc.ExitCode
    $evidence = Write-Evidence -Name 'install-prev.txt' -Text "exe=$PrevExe`nexit=$code"

    # 装完版本不会立刻写进注册表，轮询到稳定为止（最多 300s）。
    $deadline = (Get-Date).AddSeconds(300)
    $seen = $null
    while ((Get-Date) -lt $deadline) {
        $seen = Read-InstalledVersion
        if ($seen -and $seen.match -and $seen.displayVersion -eq $PrevNorm) { break }
        Start-Sleep -Seconds 3
    }
    if (-not $seen) {
        Add-Cell -Action 'install' -Status 'fail' -Assertion '安装后能读到卸载注册表项与主程序' `
            -Category 'E' -Detail "安装器 exit=$code，read-version 无输出" -Evidence @($evidence)
        return
    }
    $script:Installed = $seen
    $detail = "installer exit=$code · registry DisplayVersion=$($seen.displayVersion) · exe ProductVersion=$($seen.productVersion) · exe=$($seen.exePath)"
    if (-not $seen.match) {
        Add-Cell -Action 'install' -Status 'fail' `
            -Assertion "注册表 DisplayVersion 与 exe ProductVersion 双读一致（期望 $PrevNorm）" `
            -Category 'H' -Detail $detail -Evidence @($evidence)
        return
    }
    if ($seen.displayVersion -ne $PrevNorm) {
        Add-Cell -Action 'install' -Status 'fail' -Assertion "安装后版本 == $PrevNorm" `
            -Category 'H' -Detail $detail -Evidence @($evidence)
        return
    }
    Add-Cell -Action 'install' -Status 'pass' -Assertion "0.2.$PrevNorm 安装完成，双读版本一致（$($seen.displayVersion)）" `
        -Detail $detail -Evidence @($evidence)
}
if ($script:Installed) { $Installed = $script:Installed }

# 没有上一版时：直接装当前版本，好让启动证据仍有对象。
Invoke-Guarded -Action 'install' -Assertion "无上一版：改为静默安装 $VersionNorm 供启动验收" -Category 'E' -Body {
    if ($PrevNorm) { return }
    if (-not $CurrExe) {
        Add-Cell -Action 'install' -Status 'fail' -Assertion "安装 $VersionNorm" -Category 'E' -Detail '当前版本安装包缺失'
        return
    }
    $proc = Start-Process -FilePath $CurrExe -ArgumentList '/S' -PassThru -Wait
    $code = $proc.ExitCode
    $deadline = (Get-Date).AddSeconds(300)
    $seen = $null
    while ((Get-Date) -lt $deadline) {
        $seen = Read-InstalledVersion
        if ($seen -and $seen.displayVersion -eq $VersionNorm) { break }
        Start-Sleep -Seconds 3
    }
    if (-not $seen -or $seen.displayVersion -ne $VersionNorm) {
        Add-Cell -Action 'install' -Status 'fail' -Assertion "安装 $VersionNorm 后版本 == $VersionNorm" `
            -Category 'E' -Detail "installer exit=$code · 读到 version=$(if ($seen) { $seen.displayVersion } else { '无' })"
        return
    }
    $script:Installed = $seen
    Add-Cell -Action 'install' -Status 'pass' -Assertion "静默安装 $VersionNorm 完成" -Detail "installer exit=$code"
    $script:InstallOnly = $true
}
if ($script:Installed) { $Installed = $script:Installed }

# ===========================================================================
# 3) 触发应用内升级（双闸门：精确参数 + 环境变量），等安装器接管
# ===========================================================================
$UpdateLog = ''
$RelaunchObserved = $false
$InstallTimedOut = $false

# R2：历史回填（对老 tag 重跑验收）时 preflight 输出 `SMOKE_IS_LATEST=false`。
# 桌面更新器把检查端点写死在 `releases/latest`，**无法定向升级到 vX**：此时升级链路的
# 三个格（upgrade / 自更新日志 / 装完版本）全部标 skipped，理由写进 detail——判 fail 会
# 让每次回填都开一张假 issue。空串 = 未知（preflight 没装 gh），按 true 处理、照旧跑。
$script:UpgradeSkip = ''
if ($env:SMOKE_IS_LATEST -eq 'false') {
    $script:UpgradeSkip = "历史回填：更新器端点固定 releases/latest，无法定向升级到 v$VersionNorm"
}
elseif ($PrevNorm) {
    # 第二个闸门是**时序约束**（run 44941972411 现场）：`--smoke-self-update` 钩子是随本版
    # 才进来的，上一版二进制里没有它 → 收到这个参数只会当普通启动（隐藏窗口跑着，不写日志、
    # 不升级），driver 只能干等安装超时（实测 9 分钟才强杀）。判定收在
    # `Get-UpgradeSkipReason`（纯函数，可单测）：上一版 < $SmokeHookSince 时三个格标 skipped，
    # 且**根本不启动升级进程**；启动三格照跑（它们不依赖钩子）。
    # `$PrevNorm` 为空（首次发版）时故意不进这里：那条路径由升级格自己的「没有上一版」分支
    # 处理，文案更贴合，也不必为它多打一行 notice。
    $script:UpgradeSkip = Get-UpgradeSkipReason -PrevVersion $PrevNorm -SmokeHookSince $SmokeHookSince
    if ($script:UpgradeSkip) {
        Write-Output "::notice::升级链路的三个格标 skipped：$($script:UpgradeSkip)"
    }
}

Invoke-Guarded -Action 'upgrade' -Assertion "从 $PrevNorm 走应用内更新升到 $VersionNorm" -Category 'E' -Body {
    if ($script:UpgradeSkip) {
        Add-Cell -Action 'upgrade' -Status 'skipped' -Assertion "应用内升级 $PrevNorm → $VersionNorm" `
            -Detail $script:UpgradeSkip
        return
    }
    if ($script:InstallOnly -or -not $PrevNorm) {
        $script:UpgradeSkip = '没有上一版，升级格不适用'
        Add-Cell -Action 'upgrade' -Status 'skipped' -Assertion "应用内升级 $PrevNorm → $VersionNorm" `
            -Detail $script:UpgradeSkip
        return
    }
    if (-not $Installed -or -not $Installed.exePath) {
        Add-Cell -Action 'upgrade' -Status 'blocked' -Assertion '应用内升级' `
            -Detail '前置条件缺失：读不到已安装主程序（安装格已失败）'
        return
    }

    $AppExe = [string]$Installed.exePath
    $sandbox = Join-Path $WorkDir 'sandbox-upgrade'
    New-Item -ItemType Directory -Force -Path $sandbox | Out-Null
    # 坑 3：WB_SHIFT_SMOKE_LOG 必须是绝对路径。
    $script:UpdateLog = Join-Path $sandbox 'smoke-self-update.jsonl'

    $env:WB_SHIFT_HOME = $sandbox
    $env:WB_SHIFT_SMOKE_SELF_UPDATE = '1'
    $env:WB_SHIFT_SMOKE_LOG = $script:UpdateLog

    $appProc = Start-Process -FilePath $AppExe -ArgumentList '--hidden', '--smoke-self-update' -PassThru
    Write-Output "已启动旧版本 pid=$($appProc.Id) exe=$AppExe log=$($script:UpdateLog)"

    # 等它自己被安装器结束（正常路径）；超时则强杀并如实记 E（安装未完成）。
    # 不读退出码：NSIS 会在 Update::install 内部 exit，码不可靠（坑 4）。
    $installExited = Wait-ProcessExit -ProcessId $appProc.Id -TimeoutSec $InstallTimeoutSec -PollMs 2000
    if (-not $installExited) {
        $script:InstallTimedOut = $true
        try { Stop-Process -Id $appProc.Id -Force -ErrorAction Stop } catch { }
        Write-Output "::warning::自更新进程 ${InstallTimeoutSec}s 未退出，已强杀"
    }

    # bonus 证据：NSIS 装完可能自动拉起新版本进程（坑 1）。
    $exeName = [System.IO.Path]::GetFileNameWithoutExtension($AppExe)
    $deadline = (Get-Date).AddSeconds(60)
    while ((Get-Date) -lt $deadline) {
        $others = @(Get-Process -Name $exeName -ErrorAction SilentlyContinue | Where-Object { $_.Id -ne $appProc.Id })
        if ($others.Count -gt 0) {
            $script:RelaunchObserved = $true
            Write-Output "观察到安装器自动拉起的新进程 pid=$($others[0].Id)"
            break
        }
        Start-Sleep -Seconds 2
    }
    if ($script:RelaunchObserved) { Set-ResultField -Key 'relaunchObserved' -Value 'true' }
    Write-Output "日志是否存在：$(Test-Path -LiteralPath $script:UpdateLog)；relaunchObserved=$($script:RelaunchObserved)"
}
$UpdateLog = $script:UpdateLog
$RelaunchObserved = $script:RelaunchObserved
$InstallTimedOut = $script:InstallTimedOut

# ===========================================================================
# 4) 断言自更新日志的阶段序列（唯一硬证据）
# ===========================================================================
Invoke-Guarded -Action 'upgrade' -Assertion '自更新日志阶段序列齐全、无 error 事件' -Category 'F' -Body {
    if ($script:UpgradeSkip) {
        Add-Cell -Action 'upgrade' -Status 'skipped' -Assertion '断言自更新日志' -Detail $script:UpgradeSkip
        return
    }
    if (-not $UpdateLog -or -not (Test-Path -LiteralPath $UpdateLog)) {
        Add-Cell -Action 'upgrade' -Status 'fail' -Assertion '自更新日志落盘' -Category 'F' `
            -Detail "日志不存在：$UpdateLog（闸门不满足或 WB_SHIFT_SMOKE_LOG 不可写）"
        return
    }
    # 证据必须落在 artifact 里：日志与断言报告原本写在 RUNNER_TEMP（workflow 上传不到），
    # 拷一份进 `evidence/windows/` 再引用——否则 detail 里写的「见 xxx」是查不到的。
    $logEvidence = Write-Evidence -Name 'smoke-self-update.jsonl' `
        -Text ((Get-Content -LiteralPath $UpdateLog -Raw -ErrorAction SilentlyContinue) + '')
    $reportPath = Join-Path $WorkDir 'update-log.json'
    $assertArgs = @($AssertLog, $UpdateLog, '--target', $VersionNorm, '--json', '--out', $reportPath)
    if ($PrevNorm) { $assertArgs += @('--expect-current', $PrevNorm) }
    # 坑 1：NSIS 会在 install_start 之后接管进程，install_ok / relaunch / done 常常写不到——
    # 用显式豁免降级成 NOTE（不是静默通过），失败仍靠「装完版本变化」兜住。
    foreach ($waived in @('install_ok', 'relaunch', 'done')) { $assertArgs += @('--allow-missing', $waived) }

    $output = & $Py @assertArgs 2>&1
    Write-Output ($output | Out-String)
    $report = $null
    if (Test-Path -LiteralPath $reportPath) {
        try { $report = (Get-Content -LiteralPath $reportPath -Raw | ConvertFrom-Json) } catch { $report = $null }
    }
    if (-not $report) {
        Add-Cell -Action 'upgrade' -Status 'fail' -Assertion '断言自更新日志阶段序列' -Category 'F' `
            -Detail 'assert-update-log.py 没产出报告'
        return
    }
    $phases = ($report.phases -join '→')
    $reportEvidence = Write-Evidence -Name 'update-log.json' `
        -Text ((Get-Content -LiteralPath $reportPath -Raw -ErrorAction SilentlyContinue) + '')
    if ($report.ok) {
        $extra = ''
        if ($report.notes) { $extra = ' · note=' + ($report.notes -join '; ') }
        Add-Cell -Action 'upgrade' -Status 'pass' -Assertion '自更新日志含 start→check→check_ok→download_start→download_done→install_start 且无 error' `
            -Detail ("阶段序列=$phases · 进程数=$($report.processCount) · relaunch_observed=$($script:RelaunchObserved)$extra") `
            -Evidence @($logEvidence, $reportEvidence)
        return
    }
    $category = 'F'
    if ($report.category) { $category = [string]$report.category }
    if ($script:InstallTimedOut -and $category -eq 'F') { $category = 'E' }
    Add-Cell -Action 'upgrade' -Status 'fail' -Assertion '自更新日志阶段序列齐全、无 error 事件' `
        -Category $category -Detail ("阶段序列=$phases · " + ($report.problems -join '; ')) `
        -Evidence @($logEvidence, $reportEvidence)
}

# ===========================================================================
# 5) 装完版本 = 目标版本（升级的最终判据）
# ===========================================================================
Invoke-Guarded -Action 'version' -Assertion "升级后版本 == $VersionNorm（双读一致）" -Category 'H' -Body {
    if ($script:InstallOnly -or $script:UpgradeSkip) {
        Add-Cell -Action 'version' -Status 'skipped' -Assertion "升级后版本 == $VersionNorm" `
            -Detail $(if ($script:UpgradeSkip) { $script:UpgradeSkip } else { '没有上一版，未做升级' })
        return
    }
    $deadline = (Get-Date).AddSeconds(180)
    $seen = $null
    while ((Get-Date) -lt $deadline) {
        $seen = Read-InstalledVersion
        if ($seen -and $seen.displayVersion -eq $VersionNorm -and $seen.match) { break }
        Start-Sleep -Seconds 3
    }
    if (-not $seen) {
        Add-Cell -Action 'version' -Status 'fail' -Assertion "升级后能读到版本（期望 $VersionNorm）" -Category 'H' -Detail 'read-version 无输出'
        return
    }
    $script:Installed = $seen
    $detail = "registry DisplayVersion=$($seen.displayVersion) · exe ProductVersion=$($seen.productVersion) · match=$($seen.match) · exe=$($seen.exePath)"
    if (-not $seen.match) {
        Add-Cell -Action 'version' -Status 'fail' -Assertion '注册表与 exe 版本双读一致' -Category 'H' -Detail $detail
        return
    }
    if ($seen.displayVersion -ne $VersionNorm) {
        Add-Cell -Action 'version' -Status 'fail' -Assertion "升级后版本 == $VersionNorm" -Category 'H' -Detail $detail
        return
    }
    Set-ResultField -Key 'productVersion' -Value $seen.displayVersion
    Add-Cell -Action 'version' -Status 'pass' -Assertion "升级后版本 == $VersionNorm（注册表与 exe 双读一致）" -Detail $detail
}
if ($script:Installed) { $Installed = $script:Installed }

# ===========================================================================
# 6) 升级后启动的四元证据：20s 存活 / 单实例退出 / 落盘 / error.log 无 crash
#    （三格的实现都在 scripts/smoke/common.ps1 的 Invoke-StartupEvidence 里）
# ===========================================================================
$StartupExe = ''
if ($Installed -and $Installed.exePath) { $StartupExe = [string]$Installed.exePath }

# 历史回填（升级格 skipped）时，启动证据的对象仍是**上一版**：期望版本号必须跟着变，
# 否则 error.log 的 `appVersion == $VersionNorm` 断言会把「装的其实是 0.2.4」误判成 H/F。
$StartupVersion = $VersionNorm
if ($script:UpgradeSkip -and $PrevNorm) { $StartupVersion = $PrevNorm }

try {
    Invoke-StartupEvidence -Exe $StartupExe -SandboxRoot $WorkDir -Version $StartupVersion `
        -AliveSec $StartupAliveSec -SingleInstanceSec $SingleInstanceTimeoutSec
}
catch {
    Add-Cell -Action 'startup' -Status 'fail' -Assertion '启动四元证据' -Category 'F' `
        -Detail "driver 异常：$($_.Exception.Message)"
}

# ===========================================================================
# 7) 收尾：清场（含 NSIS 自动拉起的新进程）+ 回填 durationMs
# ===========================================================================
try {
    $killed = Stop-AllAppProcesses
    foreach ($name in @('*setup*', 'Un_A*')) {
        foreach ($proc in @(Get-Process -Name $name -ErrorAction SilentlyContinue)) {
            try { Stop-Process -Id $proc.Id -Force -ErrorAction Stop } catch { }
        }
    }
    Write-Output "收尾清场：结束 pid=$($killed -join ',')"
}
catch {
    Write-Output "::warning::清场异常：$($_.Exception.Message)"
}

try {
    $finishArgs = @($CommonPy, 'finish', '--file', $ResultPath)
    & $Py @finishArgs | Write-Output
}
catch {
    Write-Output "::warning::finish 失败：$($_.Exception.Message)"
}

Write-Output ("[smoke] windows driver 结束：失败格子 $($script:FailCount) 个；结果 $ResultPath；证据 $EvidenceDir")
# 恒 exit 0：判定在 Gate 步骤（设计 §2.2）。
exit 0
