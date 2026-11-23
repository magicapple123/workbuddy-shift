# 开发指南

分层与目录职责见 [ARCHITECTURE.md](ARCHITECTURE.md)；本页只讲环境、常用命令、
验证与发布。

## 环境要求

- Node.js ≥ 20.19（`package.json` 的 `engines`；Vite 7 要求；`companion-ui/` 要求 ≥ 22.13，
  CI 统一用 22）
- pnpm ≥ 12，版本由根 `package.json` 的 `packageManager` 固定为 **12.21.0**；安装方式
  `corepack enable pnpm` 或 `npm i -g pnpm`。**依赖只用 pnpm 安装**，不要用 npm / yarn
  （`allowBuilds` 等配置是 pnpm 11+ 的写法，旧版本会忽略）
- Rust stable（Windows 开发机使用 `x86_64-pc-windows-gnu` 工具链；CI 的 Windows runner
  用 MSVC target）。最低版本 **1.95**（根 `Cargo.toml` 的 `rust-version`，由
  `cargo +1.95 check --workspace --all-targets` 实测）；CI 另有 msrv job 防止无意中使用更新语法。
  仓库**不新增** `rust-toolchain.toml`：贡献者与 CI 各自选择 stable/1.95，`rust-version`
  只声明下限，避免 `rust-toolchain.toml` 强制下载特定工具链盖掉 CI 的矩阵设置。
- 首次：仓库根执行 `pnpm install`，一次装完根工程与 `companion-ui`（workspace 成员）

### 包管理器：pnpm workspace（2026-11-16 决策）

根 [pnpm-workspace.yaml](../pnpm-workspace.yaml) 把根工程与 `companion-ui` 收进同一个 workspace，
锁文件只有根目录一份 `pnpm-lock.yaml`。`npm/` **不在** workspace 内：它是发布到 registry 的
发行包（主包 + 5 个平台分包），纳入 workspace 会让平台分包被 link 成工程、污染发版清单。

| 位置 | 约定 |
| --- | --- |
| 安装 | 仓库根 `pnpm install`（CI 用 `pnpm install --frozen-lockfile`）；不要在 `companion-ui/` 单独安装 |
| 脚本 | `pnpm run <script>`；子工程用 `pnpm --filter workbuddy-shift-companion-ui run <script>` 或 `pnpm -C companion-ui run <script>` |
| 参数 | pnpm 会把 `--` **原样传给脚本**（与 npm 不同）。写 `pnpm run tauri build --debug`，不要写 `pnpm run tauri -- build`，否则 Vite/Tauri 把 `--` 当成「选项结束」 |
| 依赖安装脚本 | pnpm 10+ 默认拦截依赖的 postinstall。Vite 依赖的 esbuild 需要它，已在 `pnpm-workspace.yaml` 的 `allowBuilds` 放行；新增此类依赖时用 `pnpm approve-builds` 授权，不要关闭全局检查 |
| 锁文件里的 pnpm 自身 | pnpm 12 会把 `packageManager` 固定的版本记进 `pnpm-lock.yaml` 的 `packageManagerDependencies`（并列出 `@pnpm/exe.*` 各平台条目）。这是版本可复现的一部分，**不要手删**；升级 pnpm 后跑一次 `pnpm install` 让它同步 |
| 发版 | 保留 `npm publish`：`npm/` 是独立发行包，发布语义（`files`、`optionalDependencies`）与 workspace 无关；CI 的 publish job 不需要 pnpm |

该约束由 `node scripts/check-versions.mjs` 守门：根目录出现 `package-lock.json`、`yarn.lock`、
`bun.lock(b)`，或 `companion-ui/` 出现自己的锁文件，都会直接失败（不做自动删除，先判断是
误操作还是真迁移）。`pnpm-lock.yaml` 缺失同样失败——CI 的 frozen 安装依赖它。

## 开发命令

```bash
pnpm install
pnpm run tauri dev          # 开发模式（默认进沙箱，不碰真实数据）
pnpm run dev                # 只起 Vite 前端（1420 端口；桌面开发用 tauri dev）
pnpm test                   # 前端单元测试（vitest）
pnpm run lint               # ESLint（含 react-hooks；warnings 上限 9，棘轮）
pnpm run typecheck          # tsc strict
pnpm run build              # 生产构建（tsc + vite build，产物 dist/）
pnpm run build:app          # 构建 debug .app（含前端资源补丁，仅 macOS）
pnpm run build:app:release  # 构建 release .app + 签名更新包（仅 macOS）
```

Windows 上更省事的是双击仓库根的 `dev-start.cmd` / `dev-stop.cmd`：前者会先关掉已安装的
正式版再启动 dev，避免单实例锁把新窗口顶掉；沙箱位置、环境变量与排查方式见
[scripts/README.md](../scripts/README.md)。

## 本地验证

改完代码至少跑与改动域对应的命令；CI 的 `.github/workflows/test.yml` 覆盖下列入口
（平台矩阵见 [ARCHITECTURE.md](ARCHITECTURE.md#ci)）。Windows GNU 环境下需要把
`D:\Rust\cargo\bin;D:\mingw64\bin` 加进 `PATH`。

```bash
# Rust：格式、clippy、测试
cargo fmt --all -- --check
cargo clippy -p wb-shift-core -p wb-shift-server --all-targets --no-deps -- -D warnings
cargo clippy -p workbuddy-shift-desktop --all-targets --no-deps -- -D warnings
cargo test -p wb-shift-core
cargo test -p wb-shift-server

# 内置悬浮窗（不依赖 tauri/webkit，三平台 CI 都会跑）
cargo test -p companion-core
cargo test -p companion-runtime
cargo clippy -p companion-core -p companion-runtime --all-targets --no-deps -- -D warnings
# 仅 macOS：插件层的 ObjC/窗口测试
cargo test -p companion-desktop

# 前端
pnpm test
pnpm run lint
pnpm run typecheck
pnpm run build

# 内置悬浮窗前端（workspace 成员）
pnpm --filter workbuddy-shift-companion-ui run typecheck
pnpm --filter workbuddy-shift-companion-ui run test          # 入口冒烟（happy-dom，issue #5）
pnpm --filter workbuddy-shift-companion-ui run build:embed

# 仓库级契约与静态规则（CI 同样执行）
node scripts/check-versions.mjs
node scripts/check-command-routes.mjs
node scripts/check-api-contract.mjs
node scripts/check-rust-locks.mjs
node scripts/check-structure.mjs
node scripts/check-companion-contract.mjs

# 最低 Rust 版本（CI 的 msrv job 用 1.95；本机已装该工具链时可自检）
cargo +1.95 check -p wb-shift-core -p wb-shift-server -p companion-core -p companion-runtime --all-targets
```

前置条件与说明：

- `wb-shift-server` 用 `rust-embed` 嵌入 `dist/`，`src-tauri` 要求 `dist/` 与
  `src-tauri/binaries/companion-runtime-<target>` 存在，所以先跑一次 `pnpm run build`；
  悬浮窗相关检查前再跑 `node scripts/build-companion.mjs dev`（或 `dev` 启动时自动生成）。
- CI 矩阵：`wb-shift-core` + `companion-core/runtime` 在三平台 runner 上跑；
  server / desktop / companion-desktop 测试、`cargo fmt --all`、clippy、前端 build、
  companion-ui typecheck/build:embed 在 macOS lint job 上执行；Linux 与 Windows job
  各自跑对应 cfg 的 core+server clippy（Linux 另含 companion-core/runtime）；
  `pnpm run lint` / `pnpm run typecheck` / `pnpm test` 与 companion-ui 的入口冒烟测试
  （`pnpm --filter workbuddy-shift-companion-ui run test`）在 ubuntu 的 `frontend-tests`
  job；`msrv` job 用 1.95 检查四个不依赖 Tauri/webkit 的 crate。
- `cargo fmt --all` 覆盖整个 workspace，CI 用同样的 `cargo fmt --all -- --check`。
  内置会话悬浮窗的 clippy 已纳入 CI：Linux runner 检查 `companion-core`/`companion-runtime`，
  macOS lint job 检查 `companion-core`/`companion-runtime`/`companion-desktop`；
  companion-ui 走 typecheck + build:embed。
- `cargo test -p workbuddy-shift-desktop` 在部分 Windows GNU 环境会以
  `STATUS_ENTRYPOINT_NOT_FOUND` 启动失败（测试二进制加载问题，与用例本身无关），
  该检查只在 CI 的 macOS runner 上执行；本机可用 `cargo check -p workbuddy-shift-desktop`
  做编译级验证。

## 发布新版本

**版本单一来源**是根 [`Cargo.toml`](../Cargo.toml) 的 `[workspace.package] version`。

1. `sh scripts/bump-version.sh <新版本>` 统一同步 6 个 crate、`package.json`、
   `tauri.conf.json`、`npm/package.json`、平台包与 optionalDependencies、
   `companion-ui/package.json`，并精确更新 `Cargo.lock`；脚本末尾会用
   `node scripts/check-versions.mjs` 复核。
2. tag 版本（`v<ver>`）必须与上述版本一致；`.github/workflows/build.yml` 在
   `select-matrix` 里先做这项校验，不一致直接失败。
3. 安装包名由 `productName` 决定：macOS 是 `WorkBuddy Shift.app`（DMG 仍由
   `scripts/make-dmg.sh` 生成 `workbuddy-shift_<ver>_<arch>.dmg`）；Windows 原生
   NSIS 产物是 `<productName>_<ver>_x64-setup.exe`，CI 用 `find` 定位真实文件并
   复制/改名为 Release 统一的 `workbuddy-shift_<ver>_x64-setup.exe[.sig]`。不要再按
   旧 slug 写死路径。
4. 签名密钥（自动更新用）存放于 `~/.workbuddy-shift/workbuddy-shift-updater.key`，
   构建脚本通过 `TAURI_SIGNING_PRIVATE_KEY` 注入；CI 用仓库 secrets。
5. 本机 macOS 打包：`pnpm run build:app(:release)`（`scripts/fix-app.sh` 用 glob 定位
   真实 `.app`，不再写死名字）。
6. 手动生成更新清单（本地/离线排查用）：
   macOS `UPDATE_OS=macos UPDATE_ARCH=aarch64 sh scripts/gen-update-json.sh`；
   Windows `UPDATE_OS=windows UPDATE_ARCH=x86_64 sh scripts/gen-update-json.sh`；
   最后 `python3 scripts/merge-update-manifests.py <产物目录>` 合并 `latest.json`。
   CI 的 tag 发版流程会自动完成这些步骤并上传 GitHub Release。
   清单里的 `notes` 是**应用内卡片摘要（纯文本，≤600 字符）**，不是 Release 说明全文；
   `gen-update-json.sh` 会优先取 `.md` 里的 `<!-- card:begin -->` … `<!-- card:end -->`
   区块，没有标记时按行清洗整份正文（丢表格 / 标题 / 引用 / 强调标记）。完整说明仍由
   Release 页承载。规则细节见 `scripts/README.md` 的「更新清单的 `notes` 口径」。
   写下一版发布说明时记得补这段摘要区块。
7. tag 发版会额外把 npm 包镜像一份到 GitHub Packages（scoped 名
   `@<owner>/workbuddy-shift*`，由 `scripts/publish-github-packages.sh` 从 npmjs 的
   tarball 重打包后发布，备用渠道、版本号与 npmjs 同号）；workflow 加入之前的历史版本用
   `.github/workflows/github-packages.yml` 手动回填（输入 version，例如 `0.2.5`）。
   - 发布命令不带 `--access`：GitHub Packages 的 npm registry 不认 npmjs 那套
     `--access` 语义，可见性由包设置 / 与仓库的关联决定，带上去可能直接报参数错误。
   - **首次发布后必须确认可见性**：仓库 →「Packages」→ 对应包 → *Package settings*
     检查 Visibility。`package.json` 的 `repository` 字段把包关联到公开仓库后**通常**会
     继承仓库的可见性，但不保证——若显示 Private，手动改成 Public（或确认已继承公开仓库）。
     否则未登录的安装方即使自带 token 也可能拿不到包（表现为 404 / 无权限）。

> 注意：`createUpdaterArtifacts` 只会为当前版本生成签名安装包；CI 每次构建前会清掉
> `target/**/release/bundle`，避免 cargo cache 把旧安装包带进 Release。

### 预发布版本（RC）

- tag 用 `vX.Y.Z-rc.N`（例：`v0.2.1-rc.1`）。版本比较按 semver：`0.2.1-rc.1 < 0.2.1`，
  因此 RC 用户会在正式版发布后正常收到升级提示，正式版用户不会看到 RC。
- 客户端用 `semver` 解析版本（`crates/wb-shift-core/src/modules/update.rs::parse_version`）；
  非法版本号会明确报「无法比较版本号」，不会退化成「取数字前缀」。
- RC 与正式版共用同一签名密钥；不能用「禁止 `-` 的 tag」来规避预发布，那会让 RC 永远收不到正式版。

### 资产命名与发布原子性（第三轮第 4/18 条）

- 人用安装包带版本号：Windows 只有 `workbuddy-shift_<ver>_x64-setup.exe`（+ `.sig`），
  macOS 是 `workbuddy-shift_<ver>_{aarch64,x86_64}.dmg`，Linux 是 `.deb` / `.AppImage`。
  Windows 不再额外生成稳定名副本；updater 清单直接引用带版本号的文件。
- 清单里的资产 URL 必须固定到 tag：`https://github.com/<owner>/<repo>/releases/download/v<ver>/<asset>`，
  不能用 `releases/latest/download`（检查与下载之间发布新版本会下错包、验签失败）。
- 发布 job 先创建 **draft** 并上传全部资产，最后 `gh release edit --draft=false --latest`；
  上传中断时 `latest` 不会被污染。
- 发布后自动跑 `python3 scripts/check-release-assets.py "$GITHUB_REF_NAME"`：
  校验 8 份清单、Windows 去重、同 sha 重复资产（macOS 清单别名除外）、
  清单 version/平台键/签名 key id、发行说明的「下载哪个？」与「其余文件是什么？」。
  本地 dry-run 同样可用（需 `GH_TOKEN`）。
- 发行说明模板见 [`docs/release-notes-v0.2.1.md`](release-notes-v0.2.1.md)；
  资源说明表在 [`docs/release-assets.md`](release-assets.md)，CI 会自动追加（若正文已含则跳过）。
- **历史 Release 不回改**：`v0.2.0` 仍保留 `workbuddy-shift-windows-x86_64-setup.exe`，
  删掉会让 0.2.0 客户端自动更新 404；去重只对下一个版本生效。

### npm 版（webui）发布

1. 编译 server 二进制并上传 GitHub Release（`.github/workflows/build.yml` 自动执行）
2. `cd npm && npm publish`（包名 `workbuddy-shift`；postinstall 从同平台的 npm 分包
   `workbuddy-shift-<platform>-<arch>` 复制二进制，不走网络下载）
3. 发布 5 个平台：`darwin-arm64`、`darwin-x64`、`win32-x64`、`linux-x64`、`linux-arm64`。
   `linux-arm64` 只提供 CLI / WebUI 二进制（`build.yml` 的 `npm-linux-arm64` job 在 arm64
   runner 上原生编译，且不参与桌面发布），**桌面安装包仍只有 x64**；新增/下线平台要同时改
   `build.yml`（矩阵或独立 job）、`npm/package.json` optionalDependencies、
   `npm/scripts/install.js` 与 `npm/bin/workbuddy-shift.js` 两个映射表、以及
   `npm/platform/<platform-tag>/`。

## 目录结构

```
src-tauri/
  src/
    commands/        # Tauri command 薄包装，按业务域拆分（mod.rs 只做声明与 re-export）
    tray/            # 托盘图标与菜单、点击行为、轻量模式（menu/icons/platform/actions）
    update_service/  # 检查 / 下载 / 安装更新（state/download/install/tray_bridge）
    companion.rs     # 内嵌悬浮窗的宿主配置（资源根、运行时路径、默认关闭）
    instance_lock.rs # macOS 单实例补锁
crates/
  wb-shift-core/     # 核心逻辑：账号 / 认证 / 切换 / 会话 / 关联 / 积分 / Token 统计 / 限额 / 更新
    src/modules/session/       # 会话复制、同步、执行备份、恢复、流程编排
    src/modules/session_link/  # 关联存储、内容版本、操作日志、预览凭据、同步判定
    src/modules/locks.rs       # 进程内锁获取 helper（中毒返回错误 / 显式恢复）
  wb-shift-server/   # HTTP server + CLI：axum API（api/handlers/ 按域、api/dto.rs 请求体契约）
  companion-core/    # 内置会话悬浮窗：会话监控与状态归一化（含 locks.rs）
  companion-runtime/ # 内置会话悬浮窗：本地监控运行时（打包为 externalBin）
  companion-desktop/ # 内置会话悬浮窗：Tauri 插件（悬浮栏 / 设置窗口）
companion-ui/        # 内置会话悬浮窗前端：Vite 工程（嵌入页 + 公开演示页）
src/                 # 前端（React）
  pages/             # 5 个页面，各自的目录内按区块拆文件（settings/、credit-stats/ …）
  components/        # 跨页面组件；ui/ 是 shadcn 基础件，account-card/ 是账号卡片区块
  hooks/             # use-* 钩子（轮询、可见性、更新状态等）
  lib/               # api/（按业务域拆分的双通道适配）、types/（按域拆分的契约）、
                     # format.ts 等纯函数
  stores/            # zustand 状态
docs/                # ARCHITECTURE.md / DEVELOPMENT.md / companion.md
npm/                 # npm 包：package.json + bin + scripts/install.js
scripts/             # 构建、打包、图标生成、仓库契约检查（check-*.mjs）
  api-contract-baseline.json # 未迁移 Json<Value> handler 的登记与理由
  diagnostics/       # 一次性的会话诊断 / 修复脚本（不参与构建）
eslint.config.js     # 前端 lint（typescript-eslint + react-hooks）
pnpm-workspace.yaml  # pnpm workspace：根工程 + companion-ui，以及依赖构建放行（allowBuilds）
AGENTS.md            # 工程约定与验证入口（人 + AI agent 共用）
```

## 隐私注意事项

- 仓库不提交本地数据（accounts.json、认证文件、密钥、token 由 `.gitignore` 排除）
- 发布前用 `git grep` 扫描 token 模式（`ghp_`/`npm_`/`gho_` 等）

## CLI 子命令与退出码

`workbuddy-shift` 除 `serve` / `status` / `version` / `help` 外，还有一组面向 AI 的
JSON 子命令（`accounts` / `limits` / `credits` / `inspect` / `switch`），
定义与 JSON 契约见 [`docs/cli.md`](./cli.md)，实现见 `crates/wb-shift-server/src/cli/`。

工程侧只需要记住两条：

- **stdout 只输出 JSON**，进度与警告走 stderr；新增子命令必须遵守。
- 退出码：`0` 成功（含 `partial`）/ `1` 目标不存在 / `2` 用法错误 /
  `3` 拒绝执行（有活跃 CLI 会话时切号）/ `4` 致命错误。
