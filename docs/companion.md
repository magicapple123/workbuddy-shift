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

## 监听来源

| 来源 ID | 名称 | 事件通道 | 写用户文件 |
| --- | --- | --- | --- |
| `codex` | Codex | `~/.codex/hooks.json`（命令 hooks） | 安装/修复 hooks 时写 |
| `workbuddy` | WorkBuddy | `settings.json` hooks + 运行日志兜底（沙箱审批不发 hook） | 安装/修复 hooks 时写 |
| `codebuddy-ide` | CodeBuddy | IDE 的 `settings.json` hooks | 安装/修复 hooks 时写 |
| `codeg` | Codeg | 本地 Webhook 回调 | 不写用户文件 |
| `dsh` | DSH（DeepSeek Harness） | **只读**轮询 `$DSH_HOME/sessions/<project>/<session-dir>/session*.jsonl.zstd` | **不写任何文件** |

DSH 的接入类型是 `kind: "log"`（另外两种是 `hooks` / `webhook`）：接入页不装 hook，
`install` / `uninstall` 只切换「自动接入」开关。状态判定读目录——`sessions` 可读即
「已连接（只读）」，目录不存在即「暂不可用」，存在但没有日志则提示「暂无会话记录」。

DSH 适配器的取舍（都有实测依据，改之前先看 `crates/companion-core/src/adapters/dsh/`）：

- 会话日志是**多帧** zstd；按字节偏移增量解码，尾部半帧不推进 offset，下一轮重试。
- 只接**当前活跃**会话：启用监听后的首扫只登记偏移，不导入历史会话；例外是 `mtime`
  在 2 分钟内（说明刚打开或还在跑）的会话会被完整解析一次。
- header 带 `parentSession` 的**子代理会话不建档**，与 Codex 子会话隐藏策略一致。
- 格式版本白名单 v1–v4，未知版本明确报错「DSH 会话格式已更新（vN），请升级」，
  不猜字段。
- **没有「等待确认」态**：v4 日志里不存在审批/提问事件（`approval/policy` 与
  `permission/preset` 是启动配置，不是请求），因此 DSH 只有「进行中 / 已完成 / 空闲」。
- DSH 桌面端没有会话 URL scheme，卡片不提供「进入对话」。

## 权限审批（「需要授权」）

悬浮栏区分两种「要人处理」的等待，**只做检测与跳转，不做允许/拒绝决策**——审批仍在客户端
自己的弹窗里完成（远程审批需要客户端 hook 支持返回决策并把 `async` 改成阻塞式，未立项）。

| 环节 | 行为 |
| --- | --- |
| 事件 | WorkBuddy `PermissionRequest` 与确认类 `Notification` → `wait.kind="permission"`；提问工具 → `wait.kind="question"`。Codeg / Codex 未打标时由 `hub.rs#pending_kind` 推断：带 `questions` 或提问工具名的是提问，其余（含沙箱审批日志兜底）按授权处理 |
| 防误报 | 授权类 `wait` 带 `graceMs`（`hub::PERMISSION_GRACE_MS` = 2500）。hub 存 `graceUntil`，`snapshot()` 到点才投影；期间被 `resolve` 掉的请求既不出卡片也不发通知（覆盖客户端的自动批准）。提问不延迟 |
| 展示 | `presentation.ts`：授权 → 状态「需要授权」、正文「工具 · 原文」、动作「去处理」；提问 → 维持「待确认」+ 问题正文。卡片用琥珀色描边 + 盾牌图标（`rail.css` 的 `[data-kind=permission]`） |
| 去重与清理 | 同一 `callId` 重复上报只保留一条；`PostToolUse`/`PostToolUseFailure` 按 id 结算，缺 `tool_use_id` 的授权请求（占位 `perm:<ts>`）按工具名兜底结算；`Stop`/`Interrupt`/`SessionEnd` 清空该会话的 `pending` |
| 通知 | 复用运行时桥：`companion-runtime` 把 `wait` 事件交给 `companion-desktop` 弹系统通知，去重键含 `sessionId + roundId + callId`。悬浮窗 WebView 的 capability **不含** `notification:allow-notify`，因此不从页面里重复发通知；通知不可用（未开启「桌面通知」或系统拒绝）时降级为浮窗高亮（`monitor/permission-attention.ts` 给 `#desktop-rail` 加 `permission-attention`，2.4 秒后自移除），不抛错 |
| 已知缺口 | 托盘通知标题由 `companion-desktop` 统一写成「需要确认」，尚未按 `kind` 分成「需要授权」；沙箱审批日志兜底（`adapters/workbuddy/log_watch.rs`）没有 `kind` 字段，靠推断归为授权 |

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

## 入口模块初始化顺序（改动 `desktop/rail.tsx` 前必读）

两个入口页面（`desktop.html` → `src/desktop/rail.tsx`、`desktop-settings.html` →
`src/desktop/settings.tsx`）都在**模块顶层**直接挂载 React。这类文件有一条硬约束：

> **顶层副作用（`void boot(...)`）必须写在模块最末尾，位于它引用到的所有顶层
> `let` / `const` / `class` 绑定之后。**

原因是 TDZ 会伪装成「什么都没有发生」：`boot()` 是 `async`，但它在第一个 `await`
之前会同步执行 `productionOptions()`，而后者要写 `let stopAttention`。调用写在声明之前
就会抛 `ReferenceError: Cannot access '…' before initialization`；因为异常发生在 async
函数里，`void boot(...)` 把它吞成 **unhandled promise rejection**——控制台只有一条
「Uncaught (in promise)」，脚本其他部分全部正常，`<main id="desktop-rail">` 永远没有
子节点。悬浮窗是 `transparent(true)`，所以用户看到的是「完全没有悬浮栏」而不是报错窗口
（GitHub issue #5：0.2.4 – 0.3.0 四个正式版一直如此——`productionOptions()` 与
`let stopAttention` 是 2026-11-21 的 round5-09（`6b56558`）才引入的，0.2.0 – 0.2.3 的
`boot()` 是 `createRailController()` 空参调用，碰不到那个绑定，所以不受影响；
演示构建走 `createDemoRailOptions()` 不经过 `productionOptions`，所以网页演示和
设置页都正常，只有真实悬浮栏空白）。

门禁有两道，**覆盖范围不同**（都必要，但不能互相替代）：

1. **写法——两个入口文件都覆盖**：`eslint.config.js` 对这两个入口文件开启
   `@typescript-eslint/no-use-before-define`（`functions/classes/variables/enums/typedefs`
   全开）。不放开到全仓库——历史代码里「先用后声明」的写法很多，全量开启会成片报错，
   反而逼人加 disable。`pnpm run lint` 在 CI 的 `frontend-tests` job 里执行。
2. **行为——目前只覆盖 `rail.tsx`**：`companion-ui/src/desktop/rail.smoke.test.tsx`
   （`pnpm --filter workbuddy-shift-companion-ui run test`，happy-dom，
   配置见 `companion-ui/vitest.config.ts`）。它**真实求值**入口模块——不 mock
   `boot` / `productionOptions`——先挂 `process.on('unhandledRejection')`，然后断言
   `#desktop-rail` 的 `childElementCount > 0` 且捕获列表为空。判据是「症状」而
   不是「某一种写法」，所以 ESLint 规则将来被放宽或出现新的顶层顺序陷阱时它仍然有效。
   happy-dom 21 没有 `EventSource`，测试里给了一个只含 `onmessage` / `onerror` /
   `close` 的 stub（非 Tauri 环境下生产 transport 就是 `new EventSource('/events')`）；
   stub 同时记录构造过的地址，用来证明跑的是 `productionOptions()` 那条生产装配路径。
   （`settings.tsx` 目前只有写法门禁：它同步挂载、没有 async boot，行为门禁暂不覆盖）

**`settings.tsx` 为什么没有行为门禁**：它目前是纯**同步**挂载
（`if (container) createRoot(container).render(<SettingsForm />)`，全程没有 `async`
函数），这类代码抛错会当场冒到控制台，不存在「异常被 async 吞成静默 rejection」的形态，
行为测试能证明的东西已经被 ESLint 那条写法规则覆盖。**若哪天给它加了 async 挂载
（例如先 `await` 配置再渲染），必须同时补一份和 rail 同形态的冒烟测试**——那正是
issue #5 的复发条件。

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
