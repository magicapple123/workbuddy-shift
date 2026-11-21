<#
.SYNOPSIS
release-smoke 的 **PowerShell 侧薄封装**（被 `win/driver.ps1` dot-source）。

.DESCRIPTION
与 `common.sh` 同构、同口径：**结果 JSON 的字段名 / status 与 category 白名单只在
`common.py` 一份**，本文件一律转发它的 CLI（`cell` / `set` / `finish` / `sha256`）。

设计 §10.1 把这层定为「进程 / 版本 / 单实例 / 环境判定」的公共实现，因此这里提供：

| 分组 | 函数 |
|---|---|
| 结果契约 | `Add-Cell` / `Set-ResultField` / `Write-Evidence` / `Invoke-Guarded` |
| 进程 | `Get-ProcessAlive` / `Wait-ProcessExit` / `Stop-AllAppProcesses` |
| 版本 | `Read-InstalledVersion`（转发 `win/read-version.ps1` 的双读）/ `Compare-VersionNumber`（按数字段的语义化比较） |
| 环境判定 | `Test-SessionZero` / `Test-WebView2Installed` / `Get-SmokeEnvironment` |
| 资产 | `Get-ReleaseDigest` / `Invoke-GhDownload` |
| 启动四元证据 | `Invoke-StartupEvidence`（20s 存活 / 单实例退出 / 落盘 / error.log） |

依赖 driver 在 dot-source 之前定义这些变量（dot-source 共享 script 作用域）：

| 变量 | 含义 |
|---|---|
| `$Py` | python 解释器路径（`Resolve-SmokePython` 的返回值） |
| `$CommonPy` | `scripts/smoke/common.py` 的绝对路径（结果契约唯一真源） |
| `$ResultPath` | 本次结果 JSON 的绝对路径 |
| `$EvidenceDir` | 证据目录（日志 / sha256 文本 / 版本读取原始输出） |
| `$Repo` | `owner/repo`（可为空，空则用 gh 的默认仓库） |

为什么 dot-source 而不是 Import-Module：driver 需要「恒 exit 0」，模块作用域会让
`Add-Cell` 里对 `$script:FailCount` 的累加落到模块里，driver 读不到失败计数。
#>

# 本文件所在目录（scripts/smoke）。用独立变量名，避免和 driver 自己的 $PSScriptRoot 混淆。
$SmokeLibDir = $PSScriptRoot

# --- 工具链 -----------------------------------------------------------------

function Resolve-SmokePython {
    # windows-latest 上 python3 与 python 都在，但不赌：按可用性挑第一个，都没有返回空。
    foreach ($candidate in @('python3', 'python')) {
        $command = Get-Command $candidate -ErrorAction SilentlyContinue
        if ($command) { return $command.Source }
    }
    return ''
}

# --- 结果契约（一律转发 common.py，不自己拼 JSON） --------------------------

function Add-Cell {
    param(
        [Parameter(Mandatory = $true)][string]$Action,
        [Parameter(Mandatory = $true)][string]$Status,
        [Parameter(Mandatory = $true)][string]$Assertion,
        [string]$Category = '',
        [string]$Detail = '',
        [string[]]$Evidence = @()
    )
    $cellArgs = @(
        $CommonPy, 'cell',
        '--file', $ResultPath,
        '--action', $Action,
        '--status', $Status,
        '--assertion', $Assertion
    )
    if ($Category) { $cellArgs += @('--category', $Category) }
    if ($Detail) { $cellArgs += @('--detail', $Detail) }
    foreach ($item in $Evidence) {
        if (-not [string]::IsNullOrWhiteSpace($item)) { $cellArgs += @('--evidence', $item) }
    }
    try {
        & $Py @cellArgs | Write-Output
    }
    catch {
        Write-Output "::warning::写格子失败（$Action/$Status）：$($_.Exception.Message)"
    }
    if ($Status -eq 'fail') { $script:FailCount++ }
}

function Set-ResultField {
    param([string]$Key, [string]$Value)
    # 注意：splat 必须用变量（`@var`）；`@(...)` 会被当成「一个数组参数」传给 native 命令。
    $setArgs = @($CommonPy, 'set', '--file', $ResultPath, '--set', "$Key=$Value")
    try { & $Py @setArgs | Write-Output }
    catch { Write-Output "::warning::回填 $Key 失败：$($_.Exception.Message)" }
}

function Write-Evidence {
    param([string]$Name, [string]$Text)
    $path = Join-Path $EvidenceDir $Name
    try { Set-Content -LiteralPath $path -Value $Text -Encoding utf8 } catch { }
    return $path
}

# 每个步骤都过这一层：Body 抛异常时落一格 fail，绝不冒泡（driver 必须恒 exit 0）。
function Invoke-Guarded {
    param(
        [Parameter(Mandatory = $true)][string]$Action,
        [Parameter(Mandatory = $true)][string]$Assertion,
        [string]$Category = 'E',
        [Parameter(Mandatory = $true)][scriptblock]$Body
    )
    $script:StepNo++
    Write-Output "::group::[$($script:StepNo)] $Action — $Assertion"
    try {
        & $Body
    }
    catch {
        Add-Cell -Action $Action -Status 'fail' -Assertion $Assertion -Category $Category `
            -Detail "driver 异常：$($_.Exception.Message)"
    }
    finally {
        Write-Output '::endgroup::'
    }
}

# --- 进程 -------------------------------------------------------------------

function Get-ProcessAlive {
    param([int]$ProcessId)
    return [bool](Get-Process -Id $ProcessId -ErrorAction SilentlyContinue)
}

function Wait-ProcessExit {
    # 等进程退出；返回 $true = 已退出，$false = 超时仍在运行（调用方决定是否强杀）。
    # 不用 `Wait-Process` 的返回值做判定：它对「进程已消失」抛的是 terminating error，
    # 而且退出码本身在 NSIS 场景不可靠（设计 §5.1 的坑 4）。
    param(
        [Parameter(Mandatory = $true)][int]$ProcessId,
        [Parameter(Mandatory = $true)][int]$TimeoutSec,
        [int]$PollMs = 1000
    )
    $deadline = (Get-Date).AddSeconds($TimeoutSec)
    while ((Get-Date) -lt $deadline) {
        if (-not (Get-ProcessAlive -ProcessId $ProcessId)) { return $true }
        Start-Sleep -Milliseconds $PollMs
    }
    return -not (Get-ProcessAlive -ProcessId $ProcessId)
}

function Stop-AllAppProcesses {
    # 清场清单必须包含实测的进程名（Tauri 用 crate 名，不是 productName）。
    $stopped = @()
    foreach ($name in @('workbuddy-shift-desktop', 'workbuddy-shift', 'WorkBuddy Shift')) {
        $procs = @(Get-Process -Name $name -ErrorAction SilentlyContinue)
        foreach ($proc in $procs) {
            try { Stop-Process -Id $proc.Id -Force -ErrorAction Stop; $stopped += $proc.Id } catch { }
        }
    }
    return $stopped
}

# --- 版本 -------------------------------------------------------------------

function Compare-VersionNumber {
    # 语义化版本比较：返回 -1（左小）/ 0（相等）/ 1（左大）。
    #
    # **必须按 `.` 拆成数字再比**：字符串口径下 `'0.2.10' -lt '0.2.6'` 成立（`'1' -lt '6'`），
    # 会把 0.2.10 误判成「早于 0.2.6」。预发布后缀（`-rc.1`）不参与本判定：钩子类的
    # 「从哪个版本起有某个能力」只看主版本号序列（`0.2.6-rc.1` 与 `0.2.6` 视为同号）。
    param(
        [Parameter(Mandatory = $true)][string]$Left,
        [Parameter(Mandatory = $true)][string]$Right
    )
    $leftParts = @(ConvertTo-VersionParts $Left)
    $rightParts = @(ConvertTo-VersionParts $Right)
    $length = [Math]::Max($leftParts.Count, $rightParts.Count)
    for ($index = 0; $index -lt $length; $index++) {
        $x = 0
        if ($index -lt $leftParts.Count) { $x = $leftParts[$index] }
        $y = 0
        if ($index -lt $rightParts.Count) { $y = $rightParts[$index] }
        if ($x -lt $y) { return -1 }
        if ($x -gt $y) { return 1 }
    }
    return 0
}

function ConvertTo-VersionParts {
    # `v0.2.10-rc.1` → @(0, 2, 10)。非数字段归 0（不抛异常：CI driver 不能因为脏版本号崩掉）。
    param([string]$Version)
    $clean = ($Version.Trim() -replace '^[vV]', '').Trim()
    $core = ($clean -split '-')[0]
    $parts = @()
    foreach ($segment in ($core -split '\.')) {
        $digits = ($segment -replace '[^0-9]', '')
        if ([string]::IsNullOrEmpty($digits)) {
            $parts += 0
        }
        else {
            $parts += [int]$digits
        }
    }
    return $parts
}

function Get-UpgradeSkipReason {
    # 「应用内升级能不能自验」的唯一判定点（纯函数：不碰文件、不起进程，因此可单测）。
    #
    # `--smoke-self-update` 钩子自 `-SmokeHookSince`（0.2.6）才有：上一版低于它时，旧二进制
    # 收到这个参数只会当普通启动（不写日志、不升级），driver 只能干等安装超时——那是**时序
    # 约束**，不是产品缺陷。返回空串 = 照常跑升级；返回非空 = 升级链路的三个格写这个理由。
    param(
        [string]$PrevVersion = '',
        [string]$SmokeHookSince = '0.2.6'
    )
    if ([string]::IsNullOrWhiteSpace($PrevVersion)) {
        return '没有上一版，升级格不适用'
    }
    if ((Compare-VersionNumber $PrevVersion $SmokeHookSince) -ge 0) {
        return ''
    }
    # 「真正生效的版本」= 钩子版本的下一个补丁号：那一版的「上一版」才带钩子。
    $parts = @(ConvertTo-VersionParts $SmokeHookSince)
    $effective = $SmokeHookSince
    if ($parts.Count -ge 3) {
        $effective = "$($parts[0]).$($parts[1]).$($parts[2] + 1)"
    }
    return ("上一版 v$PrevVersion 不含 --smoke-self-update（钩子自 $SmokeHookSince 起才有），" +
        "本版无法自验应用内升级；从 $effective 起生效（届时上一版 $SmokeHookSince 带钩子）")
}

function Read-InstalledVersion {
    param(
        [string]$ProductName = 'WorkBuddy Shift',
        [string]$ScriptPath = ''
    )
    if ([string]::IsNullOrWhiteSpace($ScriptPath)) {
        $ScriptPath = Join-Path $SmokeLibDir 'win\read-version.ps1'
    }
    if (-not (Test-Path -LiteralPath $ScriptPath)) {
        return $null
    }
    $text = & $ScriptPath -ProductName $ProductName 2>$null
    $joined = ($text | Out-String).Trim()
    if ([string]::IsNullOrWhiteSpace($joined)) { return $null }
    try { return ($joined | ConvertFrom-Json) } catch { return $null }
}

# --- 环境判定（设计 §6.2 判 BLOCKED 的三条证据） ----------------------------

function Test-SessionZero {
    # Session 0 是服务会话：没有交互桌面，WebView2 起不了（托管 runner 的典型情况）。
    param([int]$ProcessId = 0)
    try {
        if ($ProcessId -le 0) { $ProcessId = $PID }
        return ((Get-Process -Id $ProcessId -ErrorAction Stop).SessionId -eq 0)
    }
    catch {
        return $false
    }
}

function Test-WebView2Installed {
    # WebView2 Evergreen Runtime 的两个常见落点（机器级 / 用户级）。
    foreach ($root in @(
            "${env:ProgramFiles(x86)}\Microsoft\EdgeWebView\Application",
            "$env:ProgramFiles\Microsoft\EdgeWebView\Application",
            "$env:LOCALAPPDATA\Microsoft\EdgeWebView\Application")) {
        if ($root -and (Test-Path -LiteralPath $root)) { return $true }
    }
    return $false
}

function Get-SmokeEnvironment {
    # 打包一次环境快照：结论里解释「为什么这一格取不到证据」时要看它。
    return [pscustomobject]@{
        SessionId   = $(try { (Get-Process -Id $PID).SessionId } catch { $null })
        SessionZero = (Test-SessionZero)
        WebView2    = (Test-WebView2Installed)
        User        = $env:USERNAME
        OsCaption   = $(try { (Get-CimInstance Win32_OperatingSystem).Caption } catch { '' })
    }
}

# --- Release 资产 -----------------------------------------------------------

function Get-ReleaseDigest {
    # $Tag 的 $FileName 资产的 digest（"sha256:..."）；拿不到返回空串（哈希比对在 W1 里是可选项）。
    param([string]$Tag, [string]$FileName)
    $ghArgs = @('release', 'view', $Tag, '--json', 'assets')
    if ($Repo) { $ghArgs += @('--repo', $Repo) }
    $json = & gh @ghArgs 2>$null
    if (-not $json) { return '' }
    try {
        $assets = ($json | Out-String | ConvertFrom-Json).assets
    }
    catch { return '' }
    $hit = @($assets | Where-Object { $_.name -eq $FileName }) | Select-Object -First 1
    if (-not $hit) { return '' }
    return [string]$hit.digest
}

function Invoke-GhDownload {
    # 返回 gh 的退出码（不抛异常；输出走 Host 流，避免混进返回值）。
    param([string]$Tag, [string]$Pattern, [string]$Dir)
    New-Item -ItemType Directory -Force -Path $Dir | Out-Null
    $ghArgs = @('release', 'download', $Tag, '--pattern', $Pattern, '--dir', $Dir, '--clobber')
    if ($Repo) { $ghArgs += @('--repo', $Repo) }
    $output = & gh @ghArgs 2>&1
    $code = $LASTEXITCODE
    Write-Host ($output | Out-String)
    return $code
}

function Save-ReleaseAsset {
    # 下载一个 Release 资产 + 校验 sha256，返回一个结构化结果（**不写格子**，由调用方决定记什么）。
    #
    # `Ok=false` 表示没拿到文件（调用方记 fail/D）；`Mismatch=true` 表示哈希与 Release digest
    # 不一致（fail/D）。Release 没提供 digest 时只记录不算失败——设计 W1 把哈希比对标为可选，
    # 因为旧 Release 的资产可能没有 digest 字段。
    param(
        [Parameter(Mandatory = $true)][string]$Tag,
        [Parameter(Mandatory = $true)][string]$Version,
        [Parameter(Mandatory = $true)][string]$Dir,
        [string]$Pattern = '*_x64-setup.exe',
        [string]$Extension = '.exe',
        [string]$EvidenceName = ''
    )
    $asset = [pscustomobject]@{
        Path = ''; Name = ''; Length = 0; Digest = ''; Local = ''
        Ok = $false; Mismatch = $false; Error = ''; Evidence = ''
    }
    $code = Invoke-GhDownload -Tag $Tag -Pattern $Pattern -Dir $Dir
    $files = @(Get-ChildItem -LiteralPath $Dir -File -ErrorAction SilentlyContinue |
        Where-Object { $_.Extension -eq $Extension })
    if ($code -ne 0 -or $files.Count -eq 0) {
        $asset.Error = "gh exit=$code，下载到的 $Extension 文件数=$($files.Count)"
        return $asset
    }
    $file = $files[0]
    $asset.Path = $file.FullName
    $asset.Name = $file.Name
    $asset.Length = $file.Length
    $asset.Digest = Get-ReleaseDigest -Tag $Tag -FileName $file.Name
    $shaArgs = @($CommonPy, 'sha256', '--file', $file.FullName)
    $asset.Local = (& $Py @shaArgs | Out-String).Trim()
    if ($asset.Digest -and $asset.Digest -ne "sha256:$($asset.Local)") {
        $asset.Mismatch = $true
        $asset.Error = "local=$($asset.Local) release=$($asset.Digest)"
    }
    $asset.Ok = $true
    if ([string]::IsNullOrWhiteSpace($EvidenceName)) { $EvidenceName = "sha256-$Version.txt" }
    $asset.Evidence = Write-Evidence -Name $EvidenceName -Text ("file=$($asset.Name)`nlocal=$($asset.Local)`nrelease=$($asset.Digest)")
    return $asset
}

# --- 启动四元证据（三格） ---------------------------------------------------
#
# 桌面 App 不提供 HTTP 端口，所以「起来了并且用上了」的代理证据就是这四条（设计 §6.1）：
# E1 进程 20s 存活、E2 第二实例 ≤15s 退出、E3 `$WB_SHIFT_HOME/.workbuddy-shift` 落盘、
# E4 `error.log` 无 crash 行且 appVersion 正确。缺一条都不能判 pass。
#
# BLOCKED(J) 的判定是**三条同时成立**（设计 §6.2）：20s 内退出 + 沙箱目录完全未创建 +
# 命中环境特征（Session 0 或 WebView2 缺失）。少一条就是产品问题（F），不能拿环境当挡箭牌。
function Invoke-StartupEvidence {
    param(
        [string]$Exe = '',
        [string]$SandboxRoot = '',
        [string]$Version = '',
        [int]$AliveSec = 20,
        [int]$SingleInstanceSec = 15
    )
    # 用哈希表保存跨「三格」的中间状态：scriptblock 由 `& $Body` 在**子作用域**里执行，
    # 普通变量赋值传不回来（用 `$script:` 直指脚本作用域虽然能成立，但会污染 driver 的
    # 脚本级命名空间）。哈希表是引用类型，成员赋值能透出来。
    $state = @{ Pid = 0; Sandbox = '' }

    Invoke-Guarded -Action 'startup' -Assertion "启动后 $AliveSec s 内存活" -Category 'F' -Body {
        if (-not $Exe) {
            Add-Cell -Action 'startup' -Status 'blocked' -Assertion '启动并保持存活' -Detail '读不到已安装主程序'
            return
        }
        # 清场：NSIS 自动拉起的进程会占着单实例锁，让新实例立刻退出（设计 R10）。
        $killed = Stop-AllAppProcesses
        if ($killed.Count -gt 0) { Write-Output "启动前清场，结束 pid=$($killed -join ',')" }

        $sandbox = Join-Path $SandboxRoot 'sandbox-startup'
        New-Item -ItemType Directory -Force -Path $sandbox | Out-Null
        # 新沙箱不带自更新开关：自更新环境变量若留着，新进程会再跑一遍升级。
        Remove-Item Env:WB_SHIFT_SMOKE_SELF_UPDATE -ErrorAction SilentlyContinue
        Remove-Item Env:WB_SHIFT_SMOKE_LOG -ErrorAction SilentlyContinue
        $env:WB_SHIFT_HOME = $sandbox

        $proc = Start-Process -FilePath $Exe -ArgumentList '--hidden' -PassThru
        Write-Output "启动 pid=$($proc.Id) sandbox=$sandbox"
        Start-Sleep -Seconds $AliveSec
        $alive = Get-ProcessAlive -ProcessId $proc.Id
        $store = Join-Path $sandbox '.workbuddy-shift'
        $storeExists = Test-Path -LiteralPath $store

        if ($alive) {
            $state.Pid = $proc.Id
            $state.Sandbox = $sandbox
            Add-Cell -Action 'startup' -Status 'pass' -Assertion "$AliveSec s 内进程存活（pid=$($proc.Id)）" `
                -Detail "store=$store（存在=$storeExists）"
            return
        }

        $envInfo = Get-SmokeEnvironment
        # 注意：`+` 必须留在行尾。PowerShell 在 `( … )` 里遇到**完整表达式**后的换行会当成
        # 语句结束（"缺少右括号"），只有行尾是运算符时才继续表达式——上一轮就是写成
        # 行首 `+` 导致整个文件解析失败、Windows driver 的 dot-source 直接抛异常。
        $detail = ("$AliveSec s 内已退出 · sessionId=$($envInfo.SessionId)（SessionZero=$($envInfo.SessionZero)）" +
            " · WebView2=$($envInfo.WebView2) · store 存在=$storeExists")
        if (-not $storeExists -and ($envInfo.SessionZero -or -not $envInfo.WebView2)) {
            Add-Cell -Action 'startup' -Status 'blocked' -Assertion '启动并保持存活' -Detail $detail
            return
        }
        Add-Cell -Action 'startup' -Status 'fail' -Assertion "$AliveSec s 内进程存活" -Category 'F' -Detail $detail
    }

    Invoke-Guarded -Action 'startup' -Assertion "第二实例在 $SingleInstanceSec s 内退出（单实例证据）" -Category 'F' -Body {
        if (-not $Exe -or -not $state.Pid) {
            Add-Cell -Action 'startup' -Status 'skipped' -Assertion '单实例：第二个实例退出' -Detail '首实例没起来，无从验证'
            return
        }
        $second = Start-Process -FilePath $Exe -ArgumentList '--hidden' -PassThru
        $exited = Wait-ProcessExit -ProcessId $second.Id -TimeoutSec $SingleInstanceSec -PollMs 500
        if (-not $exited) {
            try { Stop-Process -Id $second.Id -Force -ErrorAction Stop } catch { }
        }
        # 坑 4：Windows 退出码不可依赖（被安装器/单实例插件改写），因此只断言「退出」，不读码。
        if (-not $exited) {
            Add-Cell -Action 'startup' -Status 'fail' -Assertion "第二实例在 $SingleInstanceSec s 内退出" -Category 'F' `
                -Detail "第二实例 pid=$($second.Id) 超时仍在运行，已强杀"
            return
        }
        Add-Cell -Action 'startup' -Status 'pass' -Assertion "第二实例在 $SingleInstanceSec s 内退出（单实例生效）" `
            -Detail "首实例 pid=$($state.Pid) · 退出码不可依赖，未纳入判定"
    }

    Invoke-Guarded -Action 'startup' -Assertion '沙箱落盘且 error.log 无 crash 行' -Category 'F' -Body {
        if (-not $state.Sandbox) {
            Add-Cell -Action 'startup' -Status 'skipped' -Assertion '沙箱落盘 / error.log 干净' -Detail '首实例没起来，无从验证'
            return
        }
        $store = Join-Path $state.Sandbox '.workbuddy-shift'
        if (-not (Test-Path -LiteralPath $store)) {
            Add-Cell -Action 'startup' -Status 'fail' -Assertion '<沙箱>/.workbuddy-shift 存在' -Category 'F' -Detail "store 不存在：$store"
            return
        }
        $errorLog = Join-Path $store 'error.log'
        if (-not (Test-Path -LiteralPath $errorLog)) {
            Add-Cell -Action 'startup' -Status 'pass' -Assertion '沙箱落盘 + error.log 无 crash 行（本次未产生 error.log）' `
                -Detail "store=$store" -Evidence @($store)
            return
        }
        $bad = @()
        $lineNo = 0
        foreach ($line in (Get-Content -LiteralPath $errorLog -ErrorAction SilentlyContinue)) {
            $lineNo++
            if ([string]::IsNullOrWhiteSpace($line)) { continue }
            try {
                $entry = $line | ConvertFrom-Json
            }
            catch {
                $bad += "第 ${lineNo} 行不是合法 JSON"
                continue
            }
            $kind = [string]$entry.kind
            if ($kind -eq 'frontend_crash' -or $kind -eq 'frontend_unhandled' -or $kind -eq 'panic') {
                $bad += "第 ${lineNo} 行 kind=$kind message=$([string]$entry.message)"
            }
            if ($entry.appVersion -and ([string]$entry.appVersion -ne $Version)) {
                $bad += "第 ${lineNo} 行 appVersion=$([string]$entry.appVersion)（期望 $Version）"
            }
        }
        $evidence = Write-Evidence -Name 'error-log-startup.txt' -Text ((Get-Content -LiteralPath $errorLog -Raw -ErrorAction SilentlyContinue) + '')
        if ($bad.Count -gt 0) {
            Add-Cell -Action 'startup' -Status 'fail' -Assertion 'error.log 无 crash 行且 appVersion 正确' -Category 'F' `
                -Detail ("行数=$lineNo · " + ($bad -join '; ')) -Evidence @($errorLog, $evidence)
            return
        }
        Add-Cell -Action 'startup' -Status 'pass' -Assertion "error.log 无 crash 行（$lineNo 行，appVersion=$Version）" `
            -Detail "store=$store" -Evidence @($errorLog, $evidence)
    }
}
