# 发版后验收（release-smoke）

`release: published` 之后自动跑的一条独立 workflow（`.github/workflows/release-smoke.yml`），
回答一个问题：**刚发出去的这一版，用户真的装得上、升得动、起得来吗？**

它和发版流程**完全解耦**：

- 独立 workflow 文件、独立 run id，不挂在 `build.yml` / `test.yml` 里；
- 失败不改 Release 发布状态、不改源码、不改 tag（失败只体现在结论区块与 issue 上）；
- 不为 PR 提供门禁（没有 `pull_request` 触发器）。

设计与任务分解见 [`delivery/design-release-smoke.md`](../delivery/design-release-smoke.md)。

## 跑什么

| job | runner | 超时 | 内容 |
|---|---|---|---|
| `preflight` | ubuntu-latest | 3 min | tag↔版本一致性、Release 是正式版、必需资产齐全、上一版 tag、`isLatest`、GHCR manifests |
| `desktop-windows` | windows-latest | 15 min | 0.2.4 → **真·应用内升级**到 0.2.5（`--smoke-self-update`）→ 读注册表版本 → 启动四元证据 |
| `desktop-macos` | macos-latest | 12 min | dmg 覆盖 `/Applications`（`pkg-upgrade`）→ 版本 → 启动 + 单实例 + 落盘 |
| `desktop-linux` | ubuntu-latest | 15 min | `dpkg -i` 升级（`pkg-upgrade`）+ AppImage 同路径替换（`appimage-replace`，sha256 断言）→ `xvfb` 下启动 |
| `npm-cli` | ubuntu-latest | 8 min | 平台分包齐全 → `npm i -g` 升级（`npm-upgrade`）→ `version` / `serve` / `/api/status` |
| `container-docker` | ubuntu-latest | 12 min | `ghcr.io` 拉 `:<ver>` 与 `:latest`（digest 相同）→ `healthy` / `/api/status` / `uid=1000` / 卷可写 |
| `triage` | ubuntu-latest | 6 min | 归类 A–J → 次数受限的一次性处置 → 结论写回 Release 正文 → 开 issue |

墙上时间预算 ≈ 24 分钟（`preflight` + 最慢的 `desktop-windows` + `triage`），压住 25 分钟红线。

**落地状态**：全部 job 已接线。`triage` 的 `if` 是 `always()`（任何验收 job 红/被跳过都要出结论），
但它的业务步骤要求 `preflight` 通过——预检自己红掉时（版本漂移 / 资产缺失）没有可比的结果，
硬跑只会写出一堆「形态缺失」的假 FAIL；这种情况看 `preflight` 的 step summary 即可。

## 怎么读结论

### 结论在哪

1. **Release 正文末尾的标记区块**（对外，永久保留）：

   ```
   <!-- release-smoke:begin -->
   **结论：PASS**

   release-smoke · `v0.2.5` · 开始 2026-02-14T03:01:00Z · [run](链接)

   | 形态 | 安装 | 升级 | 启动 | 版本 | serve | api | pull | health | nonroot | volume |
   | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
   | `windows` `in-app` | ✅ | ✅ 0.2.4→0.2.5 | ✅ | ✅ | — | — | — | — | — | — |
   | `linux` `pkg-upgrade` | ✅ / ✅ `appimage-replace` | ✅ / ✅ `appimage-replace` | ⛔ J | ✅ | … |
   …

   **不一致 / 无法判定**：
   - `linux/startup` ⛔ J：cannot open display

   **版本一致性交叉断言**：… ✅

   **本次处置**：`A` rerun-job macos（已抑制 1 项）
   <!-- release-smoke:end -->
   ```

   **区块首行恒为 `**结论：PASS|FAIL|BLOCKED**`**（PRD P0-9）。重复运行是**整块替换**，
   不会堆叠（`split-note.py --strip` + 重拼，见 §7.2）。

2. **每次 run 的 `triage` job summary**：同一份矩阵 + 处置计划表 + 执行报告（同样内容的
   artifact `smoke-triage` 里也有 `triage-plan.json` / `dispose.json` / `conclusion.md`）。

### 行与列

- **行 = 形态**（`windows` / `macos` / `linux` / `npm` / `docker`），行首的 `` `in-app` ``
  这类是**该形态的主线升级口径**（`windows=in-app`、`macos`/`linux=pkg-upgrade`、
  `npm=npm-upgrade`、`docker=null`）；
- **列 = 动作**（安装 / 升级 / 启动 / 版本 / serve / api / pull / health / nonroot / volume）；
- **一格里有两条**（`✅ / ✅ `appimage-replace``）= 同一列有**两条升级路径**：Linux 的
  deb 主线 + AppImage 覆盖。括号里的口径是**格子级覆盖值**（与行首主线不同才写）；
- **`—` = 该形态没有这个动作**（不是失败，也不是跳过）。

### 格子状态

| 状态 | 符号 | 含义 | 要不要管 |
|---|---|---|---|
| `pass` | ✅ | 全部必须证据成立 | 不用 |
| `fail` | ❌ | 存在反证，格内跟类别字母（A–J） | 要（已开 issue / 已尝试处置） |
| `blocked` | ⛔ | **取不到证据**（托管 runner 无桌面会话等环境限制） | 人工看一眼原因；**不算通过**，但也不判产品失败 |
| `skipped` | ⏭ | 不适用（没有上一版时的升级格、macOS x86_64 这类非目标、历史回填的升级格） | 不用 |

`⛔` 不是「烂尾」：桌面 App 是 Tauri 壳，托管 runner 没有真实桌面会话，`xvfb` 下
WebKitGTK 偶发起不来属于环境限制。设计里给了 `blocked` 的**充分条件**（进程 20s 内退出
**且** store 目录完全未创建 **且** 命中环境特征），三条不全就判 `fail`（启动即崩），
不允许「猜」。所以 **全部格子都是 ⛔ 时结论是 `BLOCKED`，不是 `PASS`**。

## 失败类别（A–J）

驱动脚本直接写 `category`，推不出来时 `classify.py` 按关键词兜底，**兜不出来就 fail-closed
走 issue**（宁可多一张，不可静默吞掉一个真失败）。

| 类别 | 判据 | 处置 | 次数上限 |
|---|---|---|---|
| **A** 瞬时失败 | 首次失败 + 日志命中 `timeout` / `ECONNRESET` / `5xx` / `rate limit` 等 | 重跑该 job（`gh run rerun <run> --failed`） | 1 次/job |
| **B** npm 平台包缺失/版本不齐 | 404 或版本 ≠ tag 版本 | 重跑 `build.yml` 里失败的 `publish*` job 后复验 | 1 次/版本 |
| **C** GHCR tag 缺失 | `manifest unknown` / 404 | `gh workflow run docker.yml --ref <tag> -f push_image=true` 后复验 | 1 次/版本 |
| **D** Release 资产缺失 | 资产名不存在 / digest 不符 | 重跑 `build.yml` 的 `release` job | 1 次/版本 |
| **E** 安装失败 | 安装器退出码 ≠ 0 / 装完主程序缺失 | 不可自动 → issue + 写回区块 | — |
| **F** 启动即崩 | 20s 内退出且非 J / `error.log` 有 crash 行 | 同上 | — |
| **G** 签名不匹配 | minisign 校验失败 | 同上，**priority=high**（标题 `[security]` 前缀 + `security` label） | — |
| **H** 版本不一致 | tag / plist / dpkg / package.json / latest.json / `/api/status` 任一不等 | 同上 | — |
| **I** 容器不可用 | unhealthy / `uid != 1000` / 卷不可写 | 同上 | — |
| **J** 无法判定 | `blocked` 的三条件 | 只记 ⛔ + 原因，**不开 issue** | — |

### 处置闸门（防风暴）

| 闸门 | 行为 |
|---|---|
| 单 run 总处置 ≤ 3 | 按优先级 **A > C > D > B** 排序，累计到 3 即停；其余进「已抑制」 |
| 每类每版本 ≤ 1 | 同类别在一次发版里只处置一次（`--used-categories` + 本次已选类别） |
| `run_attempt >= 2` | **一律不重跑 / 不派发**（重跑过一次还没好，就是真问题） |
| `dry_run` | `workflow_dispatch.dry_run=true` → `dispose.py --dry-run`：**一条 `gh` 命令都不发**（连定位 build job 的两条只读 GET 也跳过，命令里的 job id 印成 `<待解析>`），且 `issue.sh` 那一步被 `if` 跳过（不开 issue） |
| 白名单动作 | 只允许 `gh run rerun` / `gh workflow run docker.yml` / `gh api .../jobs/{id}/rerun`；计划里出现别的动作（`release-delete` / `npm-publish` …）会进 `dispose.json` 的 `rejected[]` 并打 `::warning::`，不会被静默忽略 |
| 明确不做 | 改源码、删 tag、撤回 Release、`git push`、`npm publish` |

处置记录写在 `dispose.json` 与结论区块的「**本次处置**」行里（dry_run 会显式标注
「未执行」）。处置**没发出去**（gh 403 / 定位不到 build job）→ 记 `unresolved`，
由 `issue.sh` **升级成 issue**：等于没处置，就必须有人看（`unresolved` 自带与
`classify.py` 同源的 `issueTitle`，标题为空时 `issue.sh` 也会兜底生成，不会静默丢弃）。

`category=null` 的计划项是**流水线缺陷**（结果文件存在但契约非法：缺 `category`、类型错、
JSON 坏）——重跑同一份脚本只会再产出一份坏文件，所以它是 `issue` + `severity=high`，
不是「重跑一次」。而文件根本没产出的形态（驱动 job 崩了 / artifact 没上传）才归 A 类重跑。

### 演练（`dry_run=true`）到底做了什么

| 动作 | dry_run 下 | 为什么 |
|---|---|---|
| 5 个验收 job + `preflight` | **照跑** | 演练要看到真实结果，否则等于「什么都不验」 |
| `dispose.py` | `--dry-run`：0 条 gh 调用（含只读的 job 定位），命令里 job id 印成 `<待解析>` | 演练必须零副作用 |
| 写回 Release 正文的结论区块 | **照写** | 幂等（整块替换）且可删；演练看不到区块就等于没演练 |
| `issue.sh`（开 issue / 追加评论） | **跳过**（步骤 `if` 带 `inputs.dry_run != true`） | 演练不该在仓库里留下 issue |

也就是说：演练唯一动到的仓库状态是「Release 正文里的那个标记区块」。

### issue

- 标题：`[release-smoke] <tag> 验收失败：<形态> / <动作>`（G 类加 `[security] ` 前缀）；
- label：`release-smoke`（G 类另加 `security`）；
- 去重：**同标题的 open issue 只追加评论，不新开** → 同一个 tag 连跑多次失败最终只有 1 张；
- 必含：版本 / 平台 / 动作、**失败断言原文**、详情、判定理由、证据、日志链接、最小复现命令。

## 手动触发 / 演练

```bash
# 演练：只跑验收，不执行任何自动处置（历史 tag 也能跑）
gh workflow run release-smoke.yml -f tag=v0.2.5 -f dry_run=true

# 看这次 run
gh run list --workflow=release-smoke.yml
gh run view <run-id> --log
```

`dry_run` 只关掉 **triage 里的自动处置与开 issue**，**不关验收 job**——否则演练永远看不到真结果。
演练下写回结论区块仍会发生（幂等、可删，明细见上表「演练到底做了什么」）。

本地只想看预检结果（不联网、不碰 Release）：

```bash
python3 scripts/smoke/preflight.py --tag v0.2.5 --network off --out /tmp/preflight.json
```

本地自检（改了哪个脚本就跑哪个，全离线）：

```bash
python3 scripts/smoke/common.py selftest            # 结果契约
python3 scripts/smoke/preflight.py --selftest       # 预检口径
python3 scripts/smoke/render-conclusion.py --selftest  # Gate + 矩阵
python3 scripts/smoke/assert-update-log.py --selftest  # 自更新日志断言
python3 scripts/smoke/classify.py --selftest        # A–J 归类 + 兜底
python3 scripts/smoke/dispose.py --selftest         # 处置闸门（不跑 gh）
python3 scripts/smoke/split-note.py --selftest      # 区块剥离 / 幂等
```

## 结果在哪

- 每个验收 job 上传 `smoke-<form>` artifact（`artifacts/release-smoke/<form>.json` + 日志），保留 30 天；
- `preflight` 上传 `smoke-preflight`（`preflight.json`）；
- `triage` 上传 `smoke-triage`（`triage-plan.json` / `dispose.json` / `conclusion.md`）；
- 每份结果 JSON 的 `logUrl` 指回该 run 的 attempt 页面——结论区块里每格都能点回原始日志。

## 为什么桌面格没有 HTTP 断言

桌面 App 是 Tauri 桌面壳，**不监听端口**。所以「能应答」的代理证据是：单实例锁注册成功 +
store 目录已落盘 + 进程持续存活 + `error.log` 无 crash 行（四元证据）。这一点在结论里会明说，
避免读者误以为漏了断言。

## 已知限制

- **历史回填**（对老 tag 重跑验收）：桌面更新器把检查端点写死在 `releases/latest`，
  `preflight` 的 `isLatest=false` 时 **Windows 升级格标 ⏭ skipped**（`SMOKE_IS_LATEST=false`），
  而不是判失败——它确实**无法定向升级到 vX**。`isLatest` 拿不到时（离线 / 无 `gh`）
  输出 `null` 并按 `true` 处理（照旧跑）。同理，历史回填时 Windows 启动证据的期望版本
  跟着降为上一版，避免 `error.log` 的 `appVersion` 断言误判。
- 只跑托管 runner 默认镜像：`macos-latest`（arm64）一格，**x86_64 dmg / Linux arm64 不入矩阵**
  （非目标，结论里标 `skipped`）；
- macOS 不做 `spctl` / `codesign` 断言：本期无公证无签名，断言必然失败，用「启动后存活」
  间接覆盖 Gatekeeper；
- Linux 无桌面会话，`xvfb` 下 WebKitGTK 依赖 dbus/GPU 兜底，偶发
  `Failed to create WebKitWebProcess` → 按 §6.2 判 `blocked`；
- 不做 GUI 自动化（无截图 / 点击 / 托盘 / Dock 断言）、不做自动修复代码 / 回滚 / 改版本号；
- `preflight` 自己红掉（版本漂移 / 资产缺失）时，`triage` 的业务步骤不执行（没有可比结果），
  **不会**自动去重跑 `build.yml`——这种情况按 `preflight` 的 summary 人工处理。

## 人工复核入口（按顺序看）

1. **Release 正文的区块首行**：`PASS` 就结束；`FAIL` 看 ❌ 的格子和类别；
2. **❌ 格的类别**：E–I 看同 tag 的 `release-smoke` issue（有断言原文与最小复现命令）；
   A–D 看「**本次处置**」行——处置成功的话，重跑结束后区块会被下一次 triage 覆盖；
3. **⛔ 格**：读「不一致 / 无法判定」里的原因，判断是不是环境限制（环境限制 → 不用管）；
4. **还看不出来**：`gh run view <run-id> --log`，或下载 `smoke-triage` artifact 里的
   `triage-plan.json`（每项都有 reason）与 `dispose.json`（每条 gh 命令与 exit）。
