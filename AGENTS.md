# AGENTS.md — WorkBuddy Shift 工程约定（人 + AI agent 的唯一入口）

本文件是仓库的**强制入口**。任何人工或 AI agent 在改代码、改配置、改脚本、改文档之前，
都必须先读本文件，再按第 0 节读对应领域的详细文档。分层细节见 `docs/ARCHITECTURE.md`，
构建/发布见 `docs/DEVELOPMENT.md`，悬浮窗见 `docs/companion.md`，脚本索引见 `scripts/README.md`。

这里写**约束与索引**，不写实现细节；能自动验证的规则必须落到第 10 节的门禁里，
而不是只靠提示词或口头约定。

## 0. 开工前检查清单（按顺序）

1. **读规范**：本文件 → 按改动域读 `docs/ARCHITECTURE.md`（分层/模块职责）、
   `docs/DEVELOPMENT.md`（命令/CI/发版）、`docs/companion.md`（悬浮窗运行时标识）、
   `scripts/README.md`（脚本副作用）。
2. **只读看现状**：`git status`、相关 `git diff`。工作树可能包含未提交的进行中改动：
   **不得** `reset` / `checkout` / `clean` / 覆盖或回退无关文件。
3. **判定层级**：这段逻辑属于 `wb-shift-core`、两个宿主（Tauri command / axum handler）、
   前端 `src/` 还是 companion 三件套？依赖只能单向（第 2 节）。
4. **确认契约**：命令名、HTTP 方法/路径、请求体字段、响应形状、数据目录、环境变量、
   Tauri capability 是否会被改动？改稳定契约必须先写迁移方案与兼容测试（第 3 节）。
5. **改代码**：最小范围 patch；保留现有注释与行为；不做无关重构、不按行数机械拆文件。
6. **收尾**：更新受影响的文档（第 12 节）；跑与改动域对应的检查（第 11 节）；
   新增例外按第 9 节登记；汇报**未执行**的检查和原因，不能把未跑说成通过。

## 1. 产品与仓库边界

- 产品名是 **WorkBuddy Shift**；WorkBuddy / CodeBuddy / CodeBuddy IDE / CodeBuddy CLI / VS Code 插件 /
  JetBrains 插件是**第三方客户端**，不是本产品，不要改名。派生来源 workbuddy-switch 的名称与版权
  只允许出现在 NOTICE、LICENSE、README 的归属声明里（见第 7 节）。
- 单仓库六个 crate + 两个前端工程：见 `docs/ARCHITECTURE.md` 的“三宿主与依赖方向”。
- 运行时标识是**兼容契约**，不得随品牌统一改名：crate 名、npm 包名、CLI 命令、数据目录
  （`~/.workbuddy-shift`、`~/.workbuddy-shift-companion`、`~/.wb-shift-dev`）、环境变量
  （`WB_SHIFT_HOME`、`WB_DEV_SANDBOX`、`WB_DEV_REAL_HOME`、`AGENT_COMPANION_TARGET`）、
  Tauri identifier、窗口 label、capability identifier、事件名、`plugin:companion|` 前缀、
  companion-ui 的 `astra.*` localStorage key、Linux 自启文件 `agent-companion.desktop`。
  需要改时先写迁移方案与兼容测试。

## 2. 依赖方向（单向，不得反向）

```
winget/浏览器 → src-tauri (commands) ─┐
browser      → wb-shift-server (api)  ├→ wb-shift-core（纯 Rust：不依赖 tauri / axum / 前端 dist）
                                        │
companion-core → companion-runtime → companion-desktop → src-tauri（仅装配）
```

- 新能力顺序：core 实现（含单测）→ 两个宿主薄包装（Tauri command 与 HTTP handler）→ 前端
  `src/lib/api/` + `src/lib/types/` + ROUTES。三处名称必须同源（command 名 = ROUTES key）。
- `wb-shift-core` 不得出现 `tauri::*` / `axum::*`；宿主只做参数解析、边界校验、错误字符串化。
- `companion-*` 与 `wb-shift-core` 互不依赖，只通过 `src-tauri` 装配。需要共用逻辑时先论证
  是否值得新增依赖；目前只共享“约定”，不共享代码（例如 `locks` helper 在每个组件内各有一份）。
- 前端契约在 `src/lib/types/`；**不要** import 生成代码、跨工程源码或 Rust 类型定义。

## 3. 稳定契约（改动前必须有迁移方案 + 兼容测试）

| 契约 | 位置 | 说明 |
| --- | --- | --- |
| Tauri command 名 | `src-tauri/src/commands/` | 同时是 `ROUTES` 的 key（`src/lib/api/core.ts`） |
| HTTP 方法/路径 | `crates/wb-shift-server/src/api/router.rs` | 与 `ROUTES` 一一对应 |
| 请求体字段 | `crates/wb-shift-server/src/api/dto.rs` | camelCase，必须与 Tauri command 参数一致 |
| 错误信封 | `{ ok: false, error }`（400/409/500） | 前端 `call()` 依赖 `error` 字段展示 |
| 持久化文件名/JSON 键 | `docs/ARCHITECTURE.md` 的“数据存储” | 改文件名 = 用户数据迁移问题 |
| Tauri 运行时标识 | `docs/companion.md` 的“运行时标识”表 | 成组修改，改一处 = 同步全部 |
| npm 平台包映射 | `npm/package.json`、`npm/platform/<tag>/package.json`、`npm/scripts/install.js`、`npm/bin/workbuddy-shift.js` | 当前 5 个平台；新增/下线要四处同步 + 发版 job（`linux-arm64` 只有 CLI/WebUI，不产出桌面安装包）。另有 GitHub Packages scoped 镜像（`@<owner>/workbuddy-shift*`，备用渠道），由 `scripts/publish-github-packages.sh` 从 npmjs 的 tarball 重打包生成：改包名或平台时，`install.js` 的候选包列表、脚本里的平台清单与 scope 两处都要同步 |
| Tauri JS ↔ Rust 版本 | 根 `package.json` 的 `@tauri-apps/*` 与 `Cargo.lock` | 必须同 major/minor，且写成完整版本（不用 `^`）；`tauri build` 会硬校验，升级时两侧一起改 |
| 包管理器 | `pnpm-workspace.yaml` + 根 `pnpm-lock.yaml` + `companion-ui/package.json` | 只用 **pnpm**（版本由 `packageManager` 固定）。安装只在仓库根执行；参数不带 `--`（pnpm 会原样传给脚本）。规则见 `docs/DEVELOPMENT.md` 的「包管理器」节 |

- 新增 HTTP 能力必须使用明确 DTO；仍用 `Json<Value>` 的 handler 登记在
  `scripts/api-contract-baseline.json`（含理由），`node scripts/check-api-contract.mjs` 会拒绝
  未登记的新 handler，也会报告“已迁移却仍登记”的过期条目。
- 前端契约改动同步改 `src/lib/types/` 与对应 Vitest；不要只改一侧。

## 4. Rust 约定

- **错误传播**：生产路径禁止无理由 `unwrap()/expect()/panic!`；返回 `Result`，在宿主边界转成
  稳定、可处理的错误文案。
- **锁中毒**：生产代码不得 `lock().unwrap()` / `lock().expect(`（`scripts/check-rust-locks.mjs` 门禁）。
  - 业务状态用 `wb_shift_core::modules::locks::guard(..)`（companion 各 crate 有同名模块），
    中毒 → 返回错误；
  - `Mutex<()>` 串行化守卫、可重建缓存、下一次写入会整体覆盖的句柄可用 `locks::recover(..)`，
    **必须在调用点注释为什么可以恢复**；评审看到无理由的 `recover` 应拒绝。
- **crate 级禁令**：`wb-shift-server` 已在 `src/main.rs` 用
  `#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]` 把宿主层的
  “零 panic”变成编译门禁；其它 crate 的历史 `Option::unwrap` 尚未清零，新增代码不得扩大债务。
- **共享可变状态**：只允许有文档说明的缓存/锁；跨进程用 fs2 文件锁并遵守既有锁顺序
  （见 `session_link` 注释）。任务级运行标记用 `AtomicBool` 抢占，清理由**任务自身**的 Drop
  负责，不依赖 HTTP 请求 future 是否存活（见 `api/state.rs` 的 `SwitchRunGuard`）。
- **文件规模**：目标 ~300 行，>500 行必须拆分或在 `docs/ARCHITECTURE.md` 的豁免表登记理由与
  后续计划。阈值用于触发**职责审查**，不是拆文件指标；强内聚状态所有者（如
  `rail-controller.ts`）保持单文件并登记。
- **拆分边界**：以职责为界；公开 API、序列化格式、持久化文件名、SQL 行必须保持不变。
- **共享 lint**：根 `Cargo.toml` 的 `[workspace.lints.clippy]`（`dbg_macro`/`todo`/`await_holding_lock`）
  由各 crate `[lints] workspace = true` 继承；新增 lint 先确认六个成员都能通过。

## 5. Tauri 2 权限边界

- 权限按窗口最小化：主窗口见 `src-tauri/capabilities/default.json`（目前只有 event/window 少量
  allow + opener + dialog save），悬浮窗见 `src-tauri/capabilities/companion.json`。
- 不新增 fs/shell/process 等宽泛 capability；若确实需要，先说明用途、scope 与运行路径，并同步文档
  与手动验证。CSP 在 `src-tauri/tauri.conf.json` 的 `app.security.csp`；开发专用放宽只允许放
  `devCsp`，生产 CSP 不得放宽。
- companion 的 capability identifier、窗口 label、插件名、invoke 前缀必须成组一致（`docs/companion.md`
  的“运行时标识”表）；改一处 = 同步全部。

## 6. React / TypeScript 约定

- TypeScript strict + `noUnusedLocals/noUnusedParameters`；禁止 `any`、`@ts-ignore`、
  `@ts-expect-error` 掩盖真实问题、禁止关闭检查。
- ESLint：`pnpm run lint`（`eslint src companion-ui/src --max-warnings=10`）。`react-hooks` 的
  `rules-of-hooks` 为 error，`exhaustive-deps` 为 warn；warnings 上限是**棘轮**（当前 10 条历史
  债），新增即失败，修掉后把上限调低，不要用文件级 disable 压下去。
- 按业务域组织：页面在 `src/pages/<domain>/`，跨页组件在 `src/components/`，纯逻辑在 `src/lib/`；
  不新增无限膨胀的 utils/helpers/common。共享逻辑只在“调用方与变化原因相同”时抽取。
- 状态单一可信来源：zustand store 或局部 state；不要保存可推导状态；effect 只用于外部系统同步
  （订阅、网络、DOM）；保留 loading / error / empty 与必要的 aria 语义。
- API 双通道：页面只 import `@/lib/api`；桌面走 invoke、WebUI 按 `ROUTES` 走 fetch；新增能力三处
  （core、两个宿主、前端 ROUTES/types/DTO）一起改。

## 7. 旧名称与死代码政策

- 旧产品名 `workbuddy-switch` / `WorkBuddy Switch` / `wb-switch` 只能出现在 NOTICE/LICENSE/README
  归属声明；代码、UI 文案、配置、测试数据里不得再作为产品名出现。
- 死代码删除前必须证明“无调用、无动态引用、无脚本/配置/序列化路径、无运行时依赖”，并先备份；
  Windows 下删除一律进系统回收站（禁止 Remove-Item/del/rd/git clean）。
- 已知并允许的旧名称例外：`crates/companion-core/src/adapters/mod.rs` 对 `.agent-studio` 旧 hook 的
  清理（防双触发）、`companion-desktop/src/rail_settings.rs` 的 `agent-companion.desktop` 自启 slug
  （防孤儿项）、`process.rs` 中旧 exe 名负向测试、companion-ui 的 `astra.*` 用户偏好 key、
  `crates/wb-shift-core/src/modules/rate_limit_hook/mod.rs` 的 `LEGACY_STORE_DIRS`（上一代数据目录名，
  只用于**识别并清理**它遗留的 `hook.sh` 与注册条目：不认出就会双触发、事件写进已废弃目录，
  hook 永远装不上——2026-11-17 实证。只读不删旧目录，旧目录在用户机上消失后连同
  `scripts/check-structure.mjs` 的 `LEGACY_NAME_EXCEPTIONS` 登记一起删除）。

## 8. 数据与安全

- 工具数据在 `~/.workbuddy-shift`，开发构建默认沙箱 `~/.wb-shift-dev`（`WB_SHIFT_HOME` 覆盖）。
  测试/冒烟必须用 `WB_SHIFT_HOME=<TEMP 沙箱>`，不读写真实账号、客户端数据、日志、备份。
- 不提交/读取账号、token、密钥、证书、本地日志、私有目录；发布前用 `git grep` 扫 token 模式。
- WebUI **默认**仅监听 `127.0.0.1`，保留 Host/Origin 防护、请求体上限与静态资源 nosniff；
  不要为了“方便”放开监听地址或 CORS。Docker 形态只能通过 `WB_SHIFT_LISTEN`（地址）与
  `WB_SHIFT_ALLOWED_HOSTS`（Host 白名单）**显式**放开，且必须同步更新 `docs/docker.md` 的
  安全说明与 `docker-compose.yml` 注释；**禁止把放开做成默认值**（代码默认值、镜像默认值、
  compose 默认端口，三者都必须是最严的那一档）。Host / Origin 校验本身没有关闭开关。
- 不要在测试、脚本或文档里写真实账号、路径、token 示例；用 `example.com` 与临时目录。

## 9. 例外与豁免（新增必须登记）

新增任何“绕过规则”的写法（`allow`、`#[cfg(test)]` 之外的特例、平台差异、暂缓拆分、
未迁移的弱类型边界）都必须同时写清四件事：

1. **理由**：为什么现有规则在这里不适用；
2. **影响**：最坏情况下会破坏什么、影响哪些平台/调用方；
3. **后续计划**：什么时候、以什么条件移除；
4. **登记位置**：代码注释 + 本文件或 `docs/ARCHITECTURE.md` / `scripts/api-contract-baseline.json`。

不允许用“宽泛 allow + 不写理由”让 CI 变绿；CI 失败先修真实问题，不得关闭检查、扩大 allowlist、
忽略错误或跳过测试。确需临时豁免时优先缩小到单行、单函数，并留下上述四项。

### 已登记例外：Docker 形态放开监听与 Host 白名单

1. **理由**：容器里的进程必须绑 `0.0.0.0` 才能被端口映射进来，从容器外访问时浏览器带的
   `Host` 也不是回环地址。原本“只绑 127.0.0.1”的硬规则在这一个部署形态里物理上不成立。
   命令行仍不提供改地址的开关，放开只能来自显式环境变量（`WB_SHIFT_LISTEN` /
   `WB_SHIFT_ALLOWED_HOSTS`），实现见 `crates/wb-shift-server/src/listen.rs` 与
   `crates/wb-shift-server/src/api/host_guard.rs`。
2. **影响**：填错 `WB_SHIFT_ALLOWED_HOSTS` 会让更多 `Host` 通过校验；配置了该变量时，
   额外接受 `https://<同一个 Host>` 的 Origin（反代终止 TLS 的场景）。**Host / Origin 校验
   没有关闭开关**，跨站 Origin 仍被拒绝，所以最坏情况是“同一个域名下的页面都能访问”
   （本服务没有登录鉴权，等于把账号库交给该域名）。默认值（代码 / 镜像 / compose 三层）
   全部保持最严：回环监听 + 仅回环 Host。
3. **后续计划**：不计划移除（Docker 是长期形态）。引入登录鉴权后，可评估把白名单收敛为
   “必须同时配置共享密钥”。新增任何“默认放开”的改动必须先改本节。
4. **登记位置**：`crates/wb-shift-server/src/listen.rs`、`crates/wb-shift-server/src/api/host_guard.rs`
   的文件头与本节；用户侧说明在 [`docs/docker.md`](docs/docker.md) 的「安全须知」。

## 10. 规则 → 门禁对照（CI 实际执行的）

| 规则 | 门禁 | 位置 |
| --- | --- | --- |
| 命令名/路径/handler 四方一致 | `node scripts/check-command-routes.mjs` | test.yml · lint job |
| 请求体 DTO ↔ Tauri 参数一致、弱类型边界已登记 | `node scripts/check-api-contract.mjs` | test.yml · lint job |
| 生产代码不得 `lock().unwrap()` | `node scripts/check-rust-locks.mjs` | test.yml · lint job |
| 宿主层零 `unwrap/expect` | crate 级 clippy deny + `-D warnings` | 本机 + CI clippy |
| 共享 clippy lint | `[workspace.lints]` + 各 crate 继承 | `cargo clippy` |
| 前端 hooks/未使用变量/类型问题 | `pnpm run lint`（ESLint，warnings ≤ 10） | test.yml · frontend-tests |
| 前端类型 | `pnpm run typecheck`（tsc strict） | 本机 + CI |
| 版本单一来源 | `node scripts/check-versions.mjs` | 本机 + CI |
| Tauri JS 包与 Rust crate 版本配对 | 同上脚本（`checkTauriPairing`） | 本机 + CI |
| 只用 pnpm（`pnpm-lock.yaml` 存在、无其它锁文件） | 同上脚本的锁文件检测 | 本机 + CI |
| 悬浮窗演示桥一致 | `node scripts/check-companion-contract.mjs` | 本机 + CI |
| 结构约束（长文件豁免表、core 依赖方向、旧名称、capability 最小化、lint 抑制棘轮） | `node scripts/check-structure.mjs` | test.yml · lint job |
| 发版后验收结果契约（结果 JSON 字段 / `status` 词表 / `upgradeMode` 词表 / 禁用口径） | `python3 scripts/smoke/common.py selftest`（离线），Gate 用 `scripts/smoke/render-conclusion.py --gate` | release-smoke.yml · 各验收 job 的 Gate + triage |
| 最低 Rust 版本 1.95 | `cargo +1.95 check -p wb-shift-core -p wb-shift-server -p companion-core -p companion-runtime --all-targets` | test.yml · msrv job |
| 格式 | `cargo fmt --all -- --check` | 本机 + CI |

## 11. 验证与发布

- 本机验证命令（Windows GNU 环境需要 PATH 含 `D:\Rust\cargo\bin;D:\mingw64\bin`）：
  `cargo fmt --all -- --check`、三个 clippy 目标、
  `cargo test -p wb-shift-core`（本机 Windows GNU：891 通过；平台 cfg 会增减）、
  `cargo test -p wb-shift-server`、companion 两个 test、`pnpm test`、`pnpm run typecheck`、
  `pnpm run lint`、`pnpm run build`、`pnpm --filter workbuddy-shift-companion-ui run typecheck/build:embed`、
  `node scripts/check-versions.mjs`、`node scripts/check-command-routes.mjs`、
  `node scripts/check-api-contract.mjs`、`node scripts/check-rust-locks.mjs`、
  `node scripts/check-structure.mjs`、`node scripts/check-companion-contract.mjs`。
- 工具链：`Cargo.toml` 的 `[workspace.package] rust-version = 1.95` 是**实测**最低版本
  （`cargo +1.95 check --workspace --all-targets`，本机 Windows GNU）；CI 用 stable 构建，
  另有 msrv job 防止无意中使用更新语法。Node 要求见 `package.json` 的 `engines`；
  pnpm 版本由 `packageManager` 固定（12.21.0），安装只在仓库根执行。
- 已知限制（Windows GNU 测试产物）：`cargo test -p workbuddy-shift-desktop` 在部分 Windows GNU
  环境下启动即 `STATUS_ENTRYPOINT_NOT_FOUND`。**根因**是 windows-gnu 的测试产物没有链接
  comctl32 v6 的应用清单，而 `muda` 静态导入了 `TaskDialogIndirect`——该符号只在
  comctl32 **v6**（SideBySide 清单激活）里存在，`System32` 下的 v5 没有它，于是加载期就失败。
  **本机绕过**（仅用于验证，不入库）：给 test 产物嵌一个 comctl32 v6 清单，或临时把导入名
  补丁成 `TaskDialogIndirect@…` 之外的可用符号。**CI 覆盖**：同一批测试在 macOS 上正常执行，
  本轮已在上述绕过下跑通 58 个 desktop lib 测试（含 `smoke/tests.rs` 的闸门矩阵、日志序列化、
  退出码映射）。companion-desktop 的 macOS/ObjC 测试本地不可跑，同理交给 CI。

- 版本单一来源：根 `Cargo.toml` 的 `[workspace.package] version`；用 `scripts/bump-version.sh`
  统一 bump，用 `scripts/check-versions.mjs` 校验；tag 版本必须与之一致。
- 发布产物名由 `productName` 决定（macOS `<productName>.app`、Windows
  `<productName>_<version>_<arch>-setup.exe`）；不要按旧 slug 写死路径。
- **发版后验收**（`release-smoke.yml`，与发版解耦、不为 PR 提供门禁）：`release: published`
  之后自动跑，结论写在 Release 正文的 `<!-- release-smoke:begin -->` 区块（首行
  `**结论：PASS|FAIL|BLOCKED**`）+ 失败时开 `release-smoke` label 的 issue；某格 ⛔
  （`blocked`，托管 runner 无桌面会话等环境限制）**既不算通过也不算失败**，要看区块里的
  原因说明；失败类别 A–J 的含义见 `docs/release-smoke.md`。重跑用
  `gh workflow run release-smoke.yml -f tag=vX.Y.Z -f dry_run=true`（`dry_run` 时零处置）。

- 不提交、不推送、不建 PR/Release/Tag、不 npm publish，除非用户明确要求；CI 覆盖矩阵见
  `docs/DEVELOPMENT.md`。

## 12. 文档地图

| 文档 | 内容 |
| --- | --- |
| `docs/ARCHITECTURE.md` | 分层/模块职责、依赖方向、API 双通道、数据存储、权限边界、长文件豁免表 |
| `docs/DEVELOPMENT.md` | 环境、开发命令、验证矩阵、CI 门禁、发版与 npm 流程 |
| `docs/release-smoke.md` | 发版后验收：跑什么、怎么读结论区块与 ⛔、失败类别 A–J 与处置闸门、演练办法 |
| `docs/docker.md` | Docker 形态：镜像构建、数据卷、局域网/反代访问、UID/GID、已知限制与安全须知 |
| `docs/companion.md` | 内置悬浮窗的模块、构建、运行时标识、用户目录、演示桥 |
| `scripts/README.md` | 一键启动/停止、构建/打包/图标/诊断脚本索引 |
| `scripts/api-contract-baseline.json` | 未迁移 `Json<Value>` handler 的登记与理由 |
| `NOTICE` / `LICENSE` | 归属与许可；不得删除或改写版权行 |
