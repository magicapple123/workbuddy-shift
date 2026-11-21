# PRD：发版后验收（Release Smoke）CI 子系统

## 1. 项目信息

| 项 | 值 |
| --- | --- |
| Language | 中文 |
| Programming Language | GitHub Actions + Shell/PowerShell/Python（不新增产品代码） |
| Project Name | `release_smoke` |
| 交付物 | 一套「发版后自动验收」流水线 + 机器可读结论 |

**原始需求复述**：在 GitHub Actions 上实现——每次在 GitHub 上发布新版时，自动完成 Windows 应用内升级、mac 与 Linux 安装升级，以及 Windows/macOS/Linux 三端与 npm、Docker 是否能正常启动并使用的验证。

**已确认前提**：
1. 真跑升级，不接受「只校验清单/签名」的弱验证；
2. 发布后（release published）触发验收，**不阻断发版**，失败要「自动修复」；
3. 「自动修复」= **有限、可审计的处置**（只允许重跑与留言），**禁止**改源码、回滚版本。

**当前版本锚点**：0.2.5（新版）/ 0.2.4（旧版）。

## 2. 产品定义

### 2.1 Product Goals（3 个正交目标）

| 目标 | 说明 | 反例（不满足） |
| --- | --- | --- |
| G1 可回答 | 一次发布后，自动产出一张「形态 × 动作」矩阵，能直接回答：三端装得上、升得动、起得来；npm 装得上、跑得起来；Docker 拉得到、跑得起来 | 只贴一堆日志，没人能一句话说「三端是否可用」 |
| G2 真实性 | 升级必须有**版本号变化**证据；启动必须有**落盘或 HTTP**证据 | 只校验 latest.json 存在、签名正确就判过 |
| G3 不阻断 · 可审计 | 与发版解耦，失败不撤回 Release；处置动作白名单化、次数受限、同版本幂等 | 失败自动改代码 / 删 tag / 重发版本 |

**成功标准（一句话）**：发版后 ≤25 分钟，Release 页面出现结论区块，其中 6 个形态（Win/mac/Linux 桌面、npm、Docker）+ 各自动作全部有明确「通过 / 失败 / 无法判定」，且每格可点到原始日志。

```
目标结论矩阵（示例）
                    安装      升级       启动并使用
  Windows            ✓        ✓ 0.2.4→0.2.5   ✓
  macOS              ✓        ✓(P1)      ✓
  Linux(deb/AppImage) ✓       ✓(P1)      ✓
  npm                ✓        —          ✓ version/serve/api
  Docker             ✓(pull)  —          ✓ healthy/api/volume
```

### 2.2 User Stories

| # | 角色 | 故事 |
| --- | --- | --- |
| US1 | 维护者 | 作为维护者，我发完版不想逐平台手点一遍，希望 Release 里直接看到「装得上/升得动/起得来」的结论，以便决定要不要发公告。 |
| US2 | 维护者 | 作为维护者，当只有一处坏（如 GHCR tag 没推上去）时，希望系统自己重跑对应环节一次，而不是让我从头排查整条流水线。 |
| US3 | 维护者 | 作为维护者，当坏得修不了（装完就打不开）时，希望有一个 issue 带平台、动作、日志链接和最小复现命令，以便快速定位。 |
| US4 | 贡献者 | 作为贡献者，我希望 PR 只跑单测和构建，不跑重型多平台 E2E，以便 PR 不被 25 分钟验收阻塞。 |
| US5 | 使用者（间接） | 作为下载新版的用户，我不要一个「装完打不开」或「升级后旧数据丢失」的版本。 |

## 3. 需求池

> 验收标准一律写死「命令 / 期望 / 超时」。所有桌面用例统一在**沙箱家目录**下跑：`WB_SHIFT_HOME=<runner 临时空目录>`，因此账号库、配置、日志全部落在 `<tmp>/.workbuddy-shift`，不碰真实用户数据（该变量是产品既有能力，无需改代码）。

### P0（Must have）

| ID | 需求 | 验收标准（可判定） |
| --- | --- | --- |
| P0-1 | 触发与范围 | `release.published`（正式版，非 draft、非 prerelease）后 ≤5 分钟，桌面三端 + npm + Docker 共 ≥5 个 job 全部开始执行；job 失败**不**改变 Release 的发布状态 |
| P0-2 | Windows 安装 | `workbuddy-shift_0.2.5_x64-setup.exe /S` 退出码 0；≤5 分钟内主程序文件存在；主程序 ProductVersion（或卸载注册表 DisplayVersion）== `0.2.5` |
| P0-3 | Windows 升级 | 先静默装 0.2.4 → 断言版本 `0.2.4` → 再升级到 0.2.5 → 断言版本 `0.2.5`；升级后重跑 P0-4。升级口径见 §4.2 与 Q1，结论中必须标注 `upgrade_mode` |
| P0-4 | Windows 启动 | `--hidden` 启动，`WB_SHIFT_HOME=<tmp>`；20s 内未退出；进程被主动结束时无「已崩溃」记录；`error.log` 无 `frontend_crash` / `frontend_unhandled` 行（详见 §4.1） |
| P0-5 | macOS 安装 + 启动 | 挂载 dmg → 拷贝到 `/Applications` → `Info.plist` 的 `CFBundleShortVersionString == 0.2.5`；20s 内未退出；`<tmp>/.workbuddy-shift/instance.lock` 存在 |
| P0-6 | Linux 安装 + 启动 | `dpkg -i workbuddy-shift_0.2.5_amd64.deb` 退出码 0，`dpkg-query -W -f='${Version}'` 以 `0.2.5` 开头；`xvfb-run` 下 `--hidden` 启动 20s 未退出 |
| P0-7 | npm 安装 + CLI + serve | `npm i -g workbuddy-shift@0.2.5 --registry=https://registry.npmjs.org` 退出码 0；`workbuddy-shift version` 输出包含 `0.2.5`；`workbuddy-shift serve --no-open --port <临时端口>` 后 30s 内：`GET /api/status` = 200 且响应内 version == `0.2.5`，`GET /` = 200 且为 HTML |
| P0-8 | Docker 拉取 + 运行 | `docker pull ghcr.io/magicapple123/workbuddy-shift:0.2.5` 与 `:latest` 均成功；`docker run` 后 90s 内 `docker inspect` 的 Health.Status == `healthy`；`GET /api/status` = 200；容器内 `id -u` == `1000`（非 root）；向挂载卷写文件成功且宿主可读 |
| P0-9 | 结论可回答 | 同一次 run 结束时：Release 正文出现/更新「发版后验收结论」区块（矩阵 + run 链接 + 每格日志链接）；任一失败时该区块首行标注 `FAIL` |
| P0-10 | 失败分级与处置 | 每个失败必须被归入 §6 的某一类，并执行该类对应动作；未归类的失败默认按「不可自动修复」处理（fail-closed） |

### P1（Should have）

| ID | 需求 | 验收标准 |
| --- | --- | --- |
| P1-1 | macOS 覆盖升级 | 已装 0.2.4 → 用 0.2.5 覆盖同一路径 → `CFBundleShortVersionString` 变 `0.2.5` 且首次启动未崩（20s 存活） |
| P1-2 | Linux 覆盖升级 + AppImage 形态 | deb 由 0.2.4 覆盖到 0.2.5，`dpkg-query` 版本变化；AppImage 用新资产覆盖同路径后 `--appimage-extract-and-run --hidden` 20s 存活，且资产哈希 == 本次 Release 资产哈希 |
| P1-3 | 首次启动落盘断言 | 沙箱下首次启动后 30s 内，`<tmp>/.workbuddy-shift/` 目录存在；若出现 `error.log`，每行均为合法 JSON 且含 `ts/kind/message/appVersion` 字段（`appVersion` 应等于 0.2.5） |
| P1-4 | 版本一致性交叉断言 | tag `v0.2.5` == `package.json` == `latest.json` 内版本 == 桌面产物版本 == `/api/status` 返回版本；任一不等即失败 |
| P1-5 | 处置幂等 | 同一 tag 重复触发 3 次，只存在 1 个 open issue（label `release-smoke`），后续以评论追加 |

### P2（Nice to have）

| ID | 需求 | 验收标准 |
| --- | --- | --- |
| P2-1 | 性能预算 | 从 published 到结论落地 ≤25 分钟；单 job ≤10 分钟；超时即判该格「超时失败」而非挂起 |
| P2-2 | 历史回填 | 手动触发时可指定任意历史 tag（如 `v0.2.4`），跑同一套验收并产出相同矩阵 |
| P2-3 | 漂移巡检 | 每周对 `latest` 各形态各跑一次；连续 2 个版本失败升级为「需人工介入」标记 |
| P2-4 | 并发策略 | 同一 tag 同时只允许 1 个验收 run（新的取消旧的）；不同 tag 互不阻塞 |

## 4. 「能正常启动并使用」的判定标准（写死）

### 4.1 桌面三端

| 断言 | 方法 | 期望 | 超时 | 必须 |
| --- | --- | --- | --- | --- |
| 沙箱隔离 | 启动前设 `WB_SHIFT_HOME=<tmp 空目录>` | 全部数据落 `<tmp>/.workbuddy-shift`，真实家目录无新增文件 | — | 是 |
| 进程存活 | 启动后轮询进程 | 20s 内进程存在且未产生退出码 | 20s | 是 |
| 单实例证据 | 首实例运行中，再以同参数启动第二个实例 | 第二实例 ≤15s 内退出且退出码 0（证明首实例已注册并在运行） | 15s | 是（Windows 必用） |
| 落盘证据 | 检查 `<tmp>/.workbuddy-shift/instance.lock` | macOS 必须存在；其余平台出现即记录 | 30s | macOS 是 |
| 负向断言 | 读 `<tmp>/.workbuddy-shift/error.log`（若存在） | 无 `kind=frontend_crash` / `frontend_unhandled`；不出现 `panic`；网络/更新检查类 `backend` 记录**允许**（沙箱无账号） | — | 是 |
| **不作为验收项** | GUI 窗口可见性、托盘图标、Dock 显示、悬浮窗交互、开机自启注册 | 托管 runner 无真实桌面，**不可断言，明确不算通过也不算失败** | — | — |
| 无法判定时 | 上述「必须」项有任一无法取得证据 | 该格判 `BLOCKED`（不算通过），走 §6 不可修复流程并注明环境限制 | — | 是 |

### 4.2 升级（唯一硬指标：版本变化）

| 平台 | 版本读法 | 判据 |
| --- | --- | --- |
| Windows | 主程序 `ProductVersion`（或注册表 DisplayVersion） | 升级前 == `0.2.4`，升级后 == `0.2.5` |
| macOS | `Info.plist` `CFBundleShortVersionString` | 同上 |
| Linux deb | `dpkg-query -W` | 同上 |
| Linux AppImage | 资产哈希 == Release 资产哈希 | 覆盖后哈希匹配即视为「已升级」 |
| npm | `workbuddy-shift version` | 安装前后均由包版本决定；改版本即视为升级成功 |

**口径约束**：结论中必须写明 `upgrade_mode=in-app | overwrite-install`。若当期只能做覆盖安装（见 Q1），必须标注 `overwrite-install`，**不得**记作应用内升级。任何升级用例结束后必须重跑对应平台的「启动」判据（升级不能把应用搞坏）。

### 4.3 npm / Docker

| 形态 | 步骤 | 期望 | 超时 |
| --- | --- | --- | --- |
| npm | 全局安装（官方源） | 退出码 0，5 分钟内完成，且 5 个平台包依赖均可解析 | 5min |
| npm | `version` 子命令 | 输出含 `0.2.5` | 30s |
| npm | `serve --no-open --port <临时端口>` | `/api/status` 200、`/` 200（HTML）、进程 30s 内不退出 | 60s |
| Docker | `pull :0.2.5` + `:latest` | 两次均成功，tag 与版本一致 | 10min |
| Docker | `run` + `inspect` | 90s 内 `healthy`；`/api/status` 200；`id -u` == 1000；挂载卷可写且宿主可读 | 2min |

## 5. 失败分级与自动处置（关键）

**允许的处置动作（全部白名单，除此之外一律不做）**：重跑既有 workflow/job；写 Release 评论/更新结论区块；开/新建或评论 issue；上传日志 artifact。

| 类别 | 判据 | 处置 | 次数上限 |
| --- | --- | --- | --- |
| A 瞬时失败 | 首次失败且日志含 timeout / DNS / 5xx / rate limit | 同 job 重试 1 次 | 1 次/job |
| B npm 平台包缺失或版本不齐 | optional dependency 404 或版本 != 0.2.5 | 触发 npm 回填流水线 1 次 → 复验 | 1 次/版本 |
| C GHCR tag 缺失 | `docker pull :0.2.5` 或 `:latest` 404 | 触发 Docker 发布流水线 1 次 → 复验 | 1 次/版本 |
| D Release 资产缺失 | 期望资产名不存在 / 数量不符 | 重跑发版构建中失败的 job 1 次 → 复验 | 1 次/版本 |
| E 安装失败 | 安装器退出码 != 0 或装完主程序不存在 | **不可自动**：开 issue + Release 评论 | — |
| F 启动即崩 | 20s 内退出 / error.log 有 crash 行 | **不可自动**：开 issue + Release 评论 | — |
| G 签名不匹配 | minisign 校验失败 / 更新清单签名与产物不符 | **不可自动**：开 issue + Release 评论（安全类，优先） | — |
| H 版本不一致 | tag / plist / package.json / `/api/status` 任一不等 | **不可自动**：开 issue + Release 评论 | — |
| I 容器不可用 | unhealthy / 以 root 运行 / 卷不可写 | **不可自动**：开 issue + Release 评论 | — |
| J 无法判定 | 环境限制导致取不到证据（如 GUI） | 记 `BLOCKED` + 评论说明限制，不开 issue | — |

**硬约束（禁止动作）**：不改源码、不改工作流定义、不删/移动 tag、不撤回 Release、不把 draft 改回、不改版本号、不 force push、不改用户数据。违反即视为流水线缺陷。

**次数与幂等**：单次 run 的总重跑次数 ≤3；每类每版本最多 1 次；同一 tag 最多 1 个 open issue（label `release-smoke`，标题含 tag），已存在则追加评论。连续 2 个版本失败 → 结论区块标注「需人工介入」。

**不可修复 issue 必含内容**：版本 + 平台 + 动作、失败断言原文、日志 artifact 链接、最小复现命令（一条可复制执行的 shell/PowerShell 片段）、最近一次该格的通过历史。

## 6. 非目标

1. PR 阶段跑全量多平台 E2E（PR 只跑单测/构建/契约检查）；
2. GUI 截图、点击、悬浮窗与托盘交互测试；
3. 自动修复代码级 Bug、自动回滚版本、自动改版本号；
4. JetBrains 插件端 / VS Code 扩展端的验收；
5. 登录真实账号后的业务功能回归（沙箱无账号，只验「能起来、能应答」）；
6. 多发行版 / 多 Windows 版本 / 多架构的兼容性矩阵（仅托管 runner 默认镜像 + x64/arm64 现有资产）。

## 7. 待确认问题（含默认取值）

| # | 问题 | 默认取值（无异议即按此执行） |
| --- | --- | --- |
| Q1 | 桌面端当前**没有**可脚本触发的「应用内更新」入口（更新由界面/托盘触发，无 `--version` 类 CLI）。Windows 的「应用内升级」如何真跑？ | 本期 P0 用**静默覆盖安装（0.2.4 装 → 0.2.5 NSIS `/S` 覆盖）**作为升级口径，断言版本 0.2.4→0.2.5 并重跑启动判据，结论标 `upgrade_mode=overwrite-install`；真·in-app（下载 + 签名校验 + install + 重启）降为 P1，需产品先补一个非交互触发入口（如 `--update-now` 或环境变量开关） |
| Q2 | macOS 在无头 runner 上安装 .app 是否需要额外处理 Gatekeeper/quarantine？ | 允许 CI 内对拷贝后的 `.app` 去掉 quarantine 属性（仅限 runner，本地安装行为不变） |
| Q3 | Linux 桌面启动依赖 WebKitGTK 等 GUI 库，`ubuntu-latest` 上装依赖是否纳入 25 分钟预算？ | 纳入。若 `apt` 安装依赖单次超过 5 分钟，允许改用预装依赖的容器镜像执行该格（仍必须在 25 分钟预算内） |
