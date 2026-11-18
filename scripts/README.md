# 开发脚本

Windows 下的一键启动 / 关闭器，用于本地预览改动（改主题色、调 UI 时最方便）。

## 用法

双击仓库根目录的：

| 文件 | 作用 |
|---|---|
| `dev-start.cmd` | 关闭已安装的正式版 → 启动 `pnpm run tauri dev`（**沙箱数据**，桌面窗口） |
| `dev-stop.cmd` | 只结束本仓库的 dev 进程 |

调主题、量间距这类只看界面的活儿也走这个窗口：数据在沙箱里，随便点不会影响真实账号，
而且比浏览器预览多了真实的窗口尺寸、字体渲染和交互反馈。

启动后**不要关那个命令行窗口**，它是 dev 服务的控制台；关窗口或 `Ctrl+C` 即停止。
首次启动会编译 Rust，约 1 分钟；之后改前端代码是热更新，秒级生效。

**启动前那步「准备」不会再白跑。** `beforeDevCommand` 要把内置会话悬浮窗的嵌入包和
运行时构建到 `public/companion` 与 `src-tauri/binaries/`，`scripts/build-companion.mjs` 会
比对 mtime：输入没变就直接跳过（实测 **0.2 秒以内**；依赖由仓库根的 `pnpm install` 一次装好，
启动时只按需跑 `vite build` 与 release 编译运行时；改依赖后需重新 `pnpm install`）。改了 `companion-ui/` 或
`crates/companion-*` 下的源码会自动重建；想强制全量重来：

```cmd
node scripts\build-companion.mjs dev --force
```

> 早先还有一个 `dev-browser.cmd`：用 demo 数据在浏览器里看界面，为的是不碰真实数据。
> 桌面版有了沙箱隔离之后这个前提不成立了，而它看不到真实状态、还要单独构建 demo 资源，
> 所以已移除。真要临时看一眼纯静态界面，跑 `pnpm run dev:demo`（首次会构建 demo 资源，较慢）。

## 为什么启动前要关正式版

`tauri-plugin-single-instance` 在 dev 与 release 构建里都注册，按 app identifier 加锁。
不关的话，dev 实例启动时只会把已有窗口拉到前台然后退出，看起来像"启动没反应"。

## 它做了什么（以及不做什么）

`scripts/dev-start.ps1`：

1. 从脚本自身位置推断仓库根目录（**不写死路径**，克隆到哪都能跑）
2. 检查 `cargo` / `pnpm`，缺失时按常见安装位置兜底找（`%USERPROFILE%\.cargo\bin`、`D:\Rust\cargo\bin`、`D:\mingw64\bin` 等）
3. 结束"不在本仓库 `target\` 目录下"的同名进程（即已安装的正式版）
4. 前台运行 `pnpm run tauri dev`

`scripts/dev-stop.ps1` 只结束两类进程：

- `<repo>\target\` 下的 `workbuddy-shift-desktop.exe`
- 命令行里带本仓库路径的 `node.exe` / `cargo.exe` / `rustc.exe`

**不会**按进程名无差别杀，所以不会误伤你正在用的正式版或别的终端。

## 开发构建默认进沙箱（兜底）

不走上面任何一个启动器、也不设任何环境变量，直接跑：

```cmd
pnpm run tauri dev
```

宿主会在启动的**最早期**把数据目录重定向到家目录下的 `~\.wb-shift-dev`（Windows 下
即 `C:\Users\<你>\.wb-shift-dev`），并在控制台打印一行 `[dev] sandbox home: <路径>`。
账号库、会话副本、备份、限额 hook 事件全部落在那里，**不会**读写真实的
`~\.workbuddy-shift` 与 `~\.workbuddy`。第一次启动是空的，看起来就像全新安装。

想换位置（家目录在小分区，或想放到 D 盘）就设 `WB_DEV_SANDBOX`：

```cmd
set WB_DEV_SANDBOX=D:\wb-shift-dev && pnpm run tauri dev
```

（相对路径会相对真实家目录解析；只有盘符 / 根开头的绝对路径才原样使用。）

现在**只有一个 dev 数据目录**，与启动方式无关：

| 启动方式 | 数据目录 | 内容 |
|---|---|---|
| `dev-start.cmd` | `D:\wb-shift-dev` | 启动器播种好的账号 / 会话副本 |
| 直接 `pnpm run tauri dev` | `~\.wb-shift-dev` | 空目录，首次启动等于全新安装 |

启动器只导出 `WB_DEV_SANDBOX=D:\wb-shift-dev`，真正写进 `WB_SHIFT_HOME` 的是宿主的兜底
逻辑——所以两条路走的是同一段代码，区别只在"指向哪个目录"和"有没有播种"。想两边看到
同一套数据，让它们指向同一个目录即可（启动器已经这么做了）。

确认当前窗口用的是哪个目录，把鼠标停到侧边栏标题旁的琥珀色「沙箱」角标上——悬停提示里
有实际路径与真实家目录。

已经设了 `WB_SHIFT_HOME` 时，兜底逻辑不会覆盖它：环境变量优先级最高。`tauri build` 与
`tauri build --debug` 的打包产物不受影响，兜底只对 `tauri dev` 生效。

`WB_DEV_REAL_HOME=1` 会显式要求使用真实数据（启动器支持，兜底也认这个开关），用于专门
排查"真实环境下才复现"的问题。

两个相关的环境变量：

- `WB_DEV_SANDBOX` —— 兜底沙箱的**位置**。把它指到启动器用过的目录（`set
  WB_DEV_SANDBOX=D:\wb-shift-dev`）就直接复用那份已经播种好的数据，两个入口看到同一套
  账号。
- `WB_DEV_REAL_HOME=1` —— 明知故犯跑真实数据（原有的逃生开关）。设了它兜底彻底让路，
  启动器的警告与宿主日志 `[dev] WB_DEV_REAL_HOME=1: using the real home directory`
  都会提示。用完记得 unset，别带着它去切账号。

## 其它平台

macOS / Linux 直接 `pnpm run tauri dev` 即可；单实例锁在 macOS 上由
`tauri-plugin-single-instance` + `src-tauri/src/instance_lock.rs` 共同兜底，
同样需要先退出已安装的正式版。

## 相关脚本

一次性的会话诊断/修复脚本集中在 `scripts/diagnostics/`；构建、打包、图标生成仍留在
`scripts/` 根下。

### 构建 / 发布 / 仓库检查

| 脚本 | 用途 |
|---|---|
| `check-versions.mjs` | 版本单一来源校验（Cargo workspace / package.json / tauri.conf / npm 平台包 / companion-ui / Cargo.lock），CI 与 `bump-version.sh` 末尾都会跑 |
| `check-command-routes.mjs` | Tauri 注册 / command 定义 / 前端 ROUTES / axum 路由四方一致性检查 |
| `check-api-contract.mjs` | 请求体契约：POST handler 必须用 DTO；DTO 字段与 Tauri command 参数逐字一致；未迁移的 `Json<Value>` 必须登记在 `api-contract-baseline.json` |
| `check-rust-locks.mjs` | 生产 Rust 禁止 `lock().unwrap()` / `lock().expect(`（锁中毒必须返回错误或显式恢复） |
| `check-structure.mjs` | 结构约束：>500 行文件必须与 ARCHITECTURE 豁免表一致、core 不得依赖 tauri/axum、旧产品名不得回流、capability 不含 fs/shell/process、`allow(unused_imports)` 不超过基线 |
| `check-companion-contract.mjs` | 悬浮窗公开演示 postMessage channel 两端一致检查 |
| `bump-version.sh` | 统一 bump 版本（单一版本源；同步 6 crate、前端/npm/tauri 清单与 Cargo.lock） |
| `build-companion.mjs` | 内置悬浮窗嵌入包 / 运行时 / 演示包构建（见 docs/companion.md） |
| `gen-update-json.sh` / `merge-update-manifests.py` | 生成 / 合并 Tauri updater 清单（CI 发版流程使用） |
| `make-dmg.sh` / `dmgbuild-settings.py` | macOS DMG 打包（CI 无 Finder 环境使用 dmgbuild） |
| `fix-app.sh` | 本机 macOS 构建后把 dist 补进真实 `.app`（glob 定位，不写死名字） |
| `gen-tray-icon.py` / `gen-tray-template.py` / `gen-windows-rounded-icon.py` / `compare-icons.py` | 托盘图标 / Windows 圆角图标 / 图标对比工具（手动运行，不参与构建） |
| `with-env.mjs` | 跨平台环境变量前缀执行（pnpm scripts 使用） |

### 会话诊断 / 修复（一次性，手动运行）

| 脚本 | 用途 |
|---|---|
| `diagnostics/fix-incomplete-turns.py` | 修复"复制过来的会话显示思考中"（正文末条 `incomplete`） |
| `diagnostics/repair-session-space.py` | 回填跨账号复制导致的 `space_id` 错位 |
| `diagnostics/check-body-health.py` / `diagnostics/check-record-count.py` | 会话正文健康度体检（只读） |
| `diagnostics/list-orphan-sessions.py` | 列出"库里有记录、本地无正文"的孤儿会话（只读） |
