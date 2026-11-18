# 内置会话悬浮窗模块

桌面版的会话悬浮窗是 WorkBuddy Shift 的一等模块：代码直接位于本仓库，随主产品一起构建、
发布，不再有独立的源码快照或导出 / 同步流水线。

## 模块位置

| 路径 | 内容 |
| --- | --- |
| `crates/companion-core/` | 会话监控与状态归一化：各客户端 hooks / 本地文件的解析与快照 |
| `crates/companion-runtime/` | 本地监控运行时，产物为 `companion-runtime` 可执行文件（`serve` / `hook` / `custom-hook`），随应用作为 externalBin 打包 |
| `crates/companion-desktop/` | Tauri 插件：悬浮栏与设置窗口、点击穿透、位置持久化 |
| `companion-ui/` | 悬浮栏与设置页的 Vite 前端工程：嵌入用的两个页面 + 公开演示页 |

代码源自 agent-companion（MIT License，Copyright (c) 2026 agent-companion）。
许可文本：`crates/companion-core/LICENSE`；`companion-ui/LICENSE`、
`crates/companion-runtime/LICENSE`、`crates/companion-desktop/LICENSE` 为同一份副本。
现在的代码以本仓库为准：没有上游同步流程，直接修改以上路径即可。

## 构建

统一由 `scripts/build-companion.mjs` 处理：

| 命令 | 作用 |
| --- | --- |
| `node scripts/build-companion.mjs dev` | 产物过期时才重建嵌入包与运行时（`dev:desktop` 使用） |
| `node scripts/build-companion.mjs build` | 强制重建嵌入包与运行时（`build:desktop` 使用） |
| `node scripts/build-companion.mjs demo-dev` | 演示包过期时才构建到 `public/companion-demo/`（`dev:demo` 使用） |
| `node scripts/build-companion.mjs demo-build` | 先 clean 再构建演示包（`build:demo` 使用） |
| `node scripts/build-companion.mjs clean` | 删除 `public/companion-demo/`（生产构建前调用） |

- 嵌入包输出到 `public/companion/`（已 gitignore）；根目录 `vite build` 会把它带进
  `dist/companion/`，Tauri 端以 `assets: "companion/"` 加载其中的两个 HTML 页面。
- 运行时用 `cargo build --release --locked -p companion-runtime --target <triple>` 编译，
  产物复制到 `src-tauri/binaries/companion-runtime-<triple>[.exe]`，由
  `bundle.externalBin` 打进安装包。
- target 取 `AGENT_COMPANION_TARGET`，未设置时用 `rustc -vV` 的 host；发布工作流会按平台注入。
- 时间戳判断让 dev 启动不再重复全量构建，`--force` 可强制重来。
- CI：`companion-core` / `companion-runtime` 测试在三平台矩阵执行；macOS lint job
  额外跑 `companion-desktop` 测试与 `pnpm --filter workbuddy-shift-companion-ui run typecheck/build:embed`；
  `node scripts/check-companion-contract.mjs` 校验演示桥 channel。

## 受控未启用的功能

自定义接入（custom integration）的后端 RPC 与运行时路径保留并有用例覆盖，但设置页的
`CustomIntegrationManager` UI 入口当前被注释停用（见 `SettingsForm.tsx` 的说明）。
恢复入口前先确认前后端契约与演示构建都覆盖到；不要把它当成死代码删除。

## 用户目录

| 路径 | 内容 |
| --- | --- |
| `~/.workbuddy-shift-companion/bin/companion-runtime-v1[.exe]` | 稳定的 hook 入口：每次启动把当前运行时复制到这里，供各客户端 hooks 调用 |
| `~/.workbuddy-shift-companion/settings.json` | 监听开关与来源配置 |
| `~/.workbuddy-shift-companion/integrations.json` | 各来源的接入状态 |
| `~/.workbuddy-shift-companion/custom-integrations.json` | 自定义来源模板 |
| `~/.workbuddy-shift-companion/codex-recovery-v1.json` | Codex 离线事件暂存 |
| `~/.workbuddy-shift-companion/runtime-v1.json` / `.lock` | 运行时端点与单实例锁 |
| `~/.workbuddy-shift-companion/desktop-notifications.json` | 桌面通知去重所需的「已读」状态（运行时写入） |

旧版本使用 `~/.agent-studio/`。**不兼容旧安装**：合并 hooks 时会把指向
`.agent-studio/bin/agent-studio-runtime` 的旧条目清掉，避免同一事件被触发两次；旧目录
与旧二进制本身不删除，留在原处。开发调试可用 `COMPANION_HOME` 覆盖用户目录。

## 运行时标识（改名时必须成组修改）

| 项 | 值 |
| --- | --- |
| 窗口 label | `companion-rail`、`companion-settings` |
| capability identifier | `companion`（权限命名空间 `companion:*`） |
| 插件名 / invoke 前缀 | `companion` / `plugin:companion|<command>` |
| 事件 | `companion:enabled`、`companion-pointer`、`companion-window-active`、`companion-rail-settings`；插件推送 `monitor-state`、`monitor-settings`、`monitor-connection`、`monitor-warning` |
| 应用配置文件 | `companion-enabled.json`、`companion-rail-position.json` |
| 插件配置字段 | `assets`、`runtime`、`manage_autostart`、`default_enabled`、`show_rail` |
| 嵌入资源根 | `public/companion/`（运行时 URL 前缀 `companion/`） |
| 运行时二进制 | `companion-runtime`（打包名 `companion-runtime-<triple>`） |
| 目标环境变量 | `AGENT_COMPANION_TARGET` |
| Linux 自启文件 | `agent-companion.desktop`（固定 slug，改名会让已有自启项变孤儿） |
| 演示 postMessage channel | `workbuddy-shift-demo`（主界面与 companion-ui 演示桥必须一致） |

`crates/companion-desktop/Cargo.toml` 的 `links = "tauri-plugin-companion"` 决定 Tauri 的
权限命名空间（通过 `DEP_<links>` 机制派生）。它必须与 capability 里的 `companion:default`
以及前端 `plugin:companion|` 前缀一致；改其中一处就要同步另外两处。

安装后的应用仍然只有 WorkBuddy Shift 一个托盘；内置组件不会创建自己的托盘或开机启动项
（宿主传入 `manage_autostart: false`）。打包进去的是固定版本的页面与运行时，运行时不会
下载可执行代码。

## 公开演示中的悬浮栏

`npm run build:demo` 会额外构建组件的网页演示版本，让访客不安装也能看到悬浮栏：

- `demo-dev` / `demo-build` 用 `--mode demo` 构建到 `public/companion-demo/`（已 gitignore），
  前者在产物过期时才构建，后者先 clean 再构建。
- 生产构建（`npm run build`）会先执行 `clean`，陈旧的演示产物不会经由 `public/` 混进正式包。
- `src/components/companion-demo-dialog.tsx` 用 iframe 渲染 `companion-demo/desktop.html`，
  `src/lib/companion-demo-script.ts` 里的虚构快照通过 postMessage 驱动它。iframe 里是演示
  构建：没有宿主、没有托盘、没有本地数据，会话跳转与关闭命令只会回报 `blocked`。
- 两端 channel 必须一致：主界面 `COMPANION_DEMO_CHANNEL` 与 companion-ui
  `DEMO_CHANNEL` 都是 `workbuddy-shift-demo`；改一侧会让演示永久停在「连接中」，
  `node scripts/check-companion-contract.mjs` 在 CI 里拦这类分叉。
- 第一帧快照要在 frame 的 `ready` 消息时发送——iframe 的 `load` 事件太早，演示桥是动态
  import、在 `load` 之后才注册监听，这时发过去的快照会被丢掉。
