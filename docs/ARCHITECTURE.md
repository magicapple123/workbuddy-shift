# 架构

本文只讲仓库的分层、依赖方向与目录职责，用于回答「这段逻辑应该写在哪一层」。
产品功能见 [README.md](../README.md)，构建与发布命令见 [DEVELOPMENT.md](DEVELOPMENT.md)，
内置会话悬浮窗的模块约束见 [companion.md](companion.md)，工程约定与验证入口见
[AGENTS.md](../AGENTS.md)。

## 三宿主与依赖方向

同一份 `wb-shift-core` 被三种宿主复用：宿主只做协议适配，不含业务规则。

| 宿主 | 目录 | 入口 | 调用链 |
| --- | --- | --- | --- |
| 桌面 App（Tauri 2） | `src-tauri/` | `src-tauri/src/lib.rs` 的 `run()` | 前端 `invoke` → Tauri command → core |
| WebUI（本地 HTTP） | `crates/wb-shift-server/` | `src/main.rs` 的 `serve()` | 浏览器 `fetch` → axum 路由 → core |
| CLI | 同上（同一个二进制） | `src/main.rs` 的 `main()` | 终端参数 → core |

```
src-tauri/src/commands/（按业务域拆分的 Tauri command） ─┐
                                                       ├─► wb-shift-core（纯 Rust）
wb-shift-server/src/api/handlers/（按业务域拆分的 axum handler）┘
```

`wb-shift-core` 只依赖通用库（reqwest / rusqlite / tokio 等），不依赖 tauri、axum，
也不依赖前端 `dist/`。

- `wb-shift-core` 是唯一业务层：`crates/wb-shift-core/src/lib.rs` 只导出 `modules`，
  具体能力按业务域放在 `crates/wb-shift-core/src/modules/`。
- 两个宿主只做「参数解析 + 结果/错误字符串化」，且保持同源：
  `src-tauri/src/commands/`（Tauri IPC）与 `crates/wb-shift-server/src/api/handlers/`（REST）
  按业务域一一对应，覆盖同一批能力。新增能力时三处一起改：core 实现、两个宿主包装、
  前端路由表；`node scripts/check-command-routes.mjs` 在 CI 里校验三方名字/路径一致。
- 宿主内部结构：`commands/mod.rs` 与 `api/mod.rs` 只做模块声明与 re-export，
  `commands/<域>.rs` / `api/handlers/<域>.rs` 放薄包装；router 只在
  `api/router.rs` 装配一次。命令名、HTTP 路径与返回契约保持不变。
- 请求体是宿主层的协议契约：高风险接口在 `api/dto.rs` 定义明确类型（`#[serde(rename_all =
  "camelCase")]`，字段名与 Tauri command 参数逐字一致），失败统一成 400 + `{ok:false,error}`。
  仍是 `Json<Value>` 的 handler 登记在 `scripts/api-contract-baseline.json` 并写明理由，
  `node scripts/check-api-contract.mjs` 校验「DTO ↔ Tauri 参数」并拒绝未登记的新弱类型边界。
  响应体不需要两份定义：两个宿主都直接返回 core 的 `serde_json::Value`，形状天然同源。
- 前端不共享 Rust 类型：契约在 `src/lib/types/`，双通道适配在 `src/lib/api/`。
- 长任务状态归任务所有：`POST /api/switch` 的运行标记由 `spawn_blocking` 内部守卫清除，
  请求 future 被取消（浏览器断开）不会遗留「永久运行中」，后台仍在写数据时也不会放行第二次
  切换（见 `crates/wb-shift-server/src/api/state.rs` 的 `SwitchState` / `SwitchRunGuard`）。

## 六个 crate

Workspace 成员与共享依赖版本见根 [`Cargo.toml`](../Cargo.toml)。

| crate | 产物 | 职责 | 入口 |
| --- | --- | --- | --- |
| `wb-shift-core` | 库 | 全部业务逻辑（账号 / 会话 / 统计 / 限额 / 更新等） | `src/lib.rs`、`src/modules/mod.rs` |
| `wb-shift-server` | 二进制 `workbuddy-shift` | WebUI：axum API + `rust-embed` 嵌入 `dist/`；同时是 npm 安装形态的 CLI | `src/main.rs`、`src/api/` |
| `workbuddy-shift-desktop`（`src-tauri`） | 桌面 App（lib `wb_shift_lib`） | Tauri 宿主：commands、托盘、更新、单实例锁、悬浮窗宿主边界 | `src/lib.rs` |
| `companion-core` | 库 | 悬浮窗的会话监控与状态归一化（各客户端 hook / 日志解析） | `src/lib.rs`、`src/adapters/` |
| `companion-runtime` | 二进制 `companion-runtime` | 悬浮窗本地监控服务与 hook 客户端，作为 Tauri `externalBin` 打包 | `src/main.rs` |
| `companion-desktop` | Tauri 插件库 | 悬浮栏 / 设置窗口、命中测试、位置持久化 | `src/lib.rs` |

要点：

- `wb-shift-core` 不依赖 Tauri 也不依赖 Axum；这两个重依赖只出现在宿主 crate。
- `companion-*` 三件套与 `wb-shift-core` 互不依赖，只通过 `src-tauri` 装配进来
  （`src-tauri/src/companion.rs` 给插件传资源根与运行时路径）。
- `companion-desktop` 的 `links = "tauri-plugin-companion"` 决定 Tauri 权限命名空间，
  改名时要同步 capability 与前端 `plugin:companion|` 前缀，见 [companion.md](companion.md)。
- 宿主与 companion 也已按职责目录化：`src-tauri/src/tray/`（`menu`/`icons`/`platform`/`actions`）、
  `src-tauri/src/update_service/`（`state`/`download`/`install`/`tray_bridge`）、
  `companion-desktop/src/plugin/`、`companion-desktop/src/rail_settings/autostart/`（三平台子模块）、
  `companion-core/src/adapters/{workbuddy,codeg/{events,child}}/`、`companion-core/src/custom/engine/`；
  入口文件只做装配，事件/命令/持久化契约未变。

### wb-shift-core 模块地图

| 主题 | 模块（`src/modules/` 下） |
| --- | --- |
| 账号与切换 | `account/`（`store`/`display`/`identity`）、`auth_file/`（`read`/`sync`）、`switch/`（`plan`/`execute`）、`variant.rs`（档位单一来源）、`oauth.rs`、`export_import/`（`export`/`import`）；`process/`（`mod.rs` 公共入口 + `windows/`（进程/注册表/解析）+ `macos.rs`） |
| 会话复制 / 同步 / 恢复 | `session/mod.rs`（公共入口与共享口径）、`session/paths.rs`（存储/数据根与命名空间）、`session/query.rs`（会话列表、DB 探测与会话行存活索引 `LiveSessionRows`）、`session/copy/{mod,body,db,db_spaces,plan,plan_prepare,plan_execute}.rs`、`session/exec/{mod,backup,write,write_replay}.rs`、`session/sync/{mod,preview,liveness,plan,wait}.rs`、`session/recovery.rs`、`session/run.rs`、`session/tests/`、`session_backup.rs`、`session_groups/`（`view` 列表/详情、`liveness` 插件/IDE 的组级存活汇总与清理拒绝原因、`sync` 配对、`members` 成员操作、`graph` 统一批次、`maintenance` 失效组清理） |
| 会话关联存储与判定 | `session_link.rs`（模块声明与 re-export）与其子模块 `session_link/{content,link_store,link_store/{baseline,store},operation_log,preview,sync_decision}.rs`、`session_link/tests/` |
| 其它客户端 | `codebuddy_cli/`（`state`/`settings`/`helper`/`switch`）、`codebuddy_ide/`、`codebuddy_cn_ide/`、`codebuddy_ide_session/`、`codebuddy_ide_session_sync/`、`vscode_ext/`、`vscode_session/`、`vscode_session_sync/`（`index` 工作区索引四态/定位/会话目录证据、`liveness` 会话存活判定（三处消费方共用）、`register` 复制后登记、`preview` 预览、`plan`/`exec` 同步、`backup` 回滚）、`vscode_session_link/`、`vscode_cn_inject/`、`jetbrains/`（其余全部按 `mod/status/switch/detect/...` 目录化） |
| 限额 | `limits/`（`scan` 日志扫描、`parse/{mod,workbuddy,ide_cli}` 解析分类、`ledger/{mod,cache,payload}` 台账与缓存、`payload` 输出）、`rate_limit_events/`（`state`/`parse`/`watch`）、`rate_limit_hook/`（`install`/`uninstall`/`status`） |
| 统计与自动化 | `credits/`（`http` 取数、`resources` 投影）、`credit_usage/`（`collect`/`project`）、`official_usage/`（`fetch`/`parse`/`aggregate`）、`token_stats/`（`jsonl` 解码、`ide` 数据源、`projection` 聚合、`cache` 文件缓存）、`checkin/`（`status`、`window` 时间段、`cycle` 轮次、`logs`）、`travel/`（`status`/`cycle`/`config`）、`rotate/`（`decision`/`cycle`/`logs`）、`refresh.rs` |
| 基础设施 | `config/`（`paths` 路径与环境变量、`store` 原子写/通用 JSON、`http` 请求与错误归一化 + 共享 `is_unauthorized`/`parse_number`、`configs/{mod,checkin,travel,automation}` 业务配置）、`notifications.rs`、`session_backup/`（`backup`/`lifecycle`/`cleanup`）、`error_log.rs`、`update.rs`、`switch/`、`linux_keyring.rs`（仅 Linux）、`locks.rs`（进程内锁获取 helper：中毒返回错误或显式恢复） |

> `session` / `session_link` 已拆成目录模块（文件即子边界）；对外仍从 `session::` /
> `session_link::` 的 re-export 使用，调用方不需要跟着改路径。

会话「存活」判定有两套依据，按客户端分派，互不串用：

| 客户端 | 判定依据 | 实现 |
| --- | --- | --- |
| WorkBuddy | `workbuddy.db` 的 `sessions` 行（按成员档位读库） | `session::group_any_alive` |
| VS Code 插件 / CodeBuddy IDE | 工作区索引 `index.json` 条目**或**会话目录 `history/<workspaceHash>/<conversationId>/` | `vscode_session_sync::liveness`（`LivenessJudge` / `SessionLiveness`） |

插件侧的判定优先级是「索引命中 → 目录存在 → 两者都没有时看四态」：目录比索引条目更可靠，
`index.json` 损坏 / 半截写入 / 条目被改归属时条目会缺失，会话目录却还在。目录集合与定位表
来自**同一次**遍历（`conversation_index_scan` 进每个工作区目录时顺手 `read_dir`），
**不得**为每个待判定的会话单独 `is_dir()`。链接（符号链接 / junction）指向的目录同样算证据：
普通目录项走 `DirEntry::file_type()`，链接项补一次跟随判定。判定结果按 `(spec, root, uid)`
缓存，一个请求内一个账号只扫一遍盘；预览 / 管理页 / 批量清理三处共用同一份结论
（`session_groups::liveness` 做组级汇总与拒绝原因），所以**管理页隐藏的组 ≡ 清理能删的组**。

四态只回答一个问题——「不在索引里」能不能当作会话已消失：

| 状态 | 含义 | 索引与目录都没命中时 |
| --- | --- | --- |
| `Absent` | 历史目录不存在（账号从未用过 / 数据根不对） | `Unknown`：读路径不过滤，写路径拒绝整次清理 |
| `Unavailable` | 目录在，但一份 `index.json` 都没解析成功 | 同上 |
| `Partial` | 有索引文件的工作区里只有一部分解析成功：`ids` 只是**可读分片**的并集 | 同上（`Partial` 下命中可读分片即 `Alive`） |
| `Available` | 每一份索引都解析成功 | `Gone`：读路径过滤，写路径可删组 |

分母只数**含 `index.json` 的工作区目录**：目录在却没有索引文件通常是客户端正常状态
（空工作区 / 尚未写入），把它算进分母会让「有一个空工作区」就永远 `Partial`，该客户端的
清理被永久拒绝；那种现场若会话真的存在，目录证据会兜底。

`Partial` 是关键的一格：只读分片里没命中**不能**证明会话已消失（损坏的那份里可能正装着
它），所以两侧都向「不删 / 不隐藏」的方向取舍。读路径在非 `Available` 时 **fail-open**，写路径
在非 `Available` 时 **fail-closed**（`Partial` 的落点是「找到就活、找不到就判不了」）——破坏性
操作绝不「读不到就当全死」，但也不会因为一处损坏就永久挡住全部清理（会话目录存在即可正常
处理，这是 2026-11 那一轮的收益）。定位表、四态与目录证据来自同一次目录遍历
（`conversation_index_scan`），不要新增第二份扫描逻辑。

管理页按客户端分派：`Workbuddy` 用 `sessions` 行，插件 / IDE 用上面的组合判定；插件 / IDE
的数据根解析不到时 **fail-open** 全部展示（读路径不因缺依据而隐藏）。

`list` 载荷的清理入口状态有两个键（**都始终存在**）：`deadGroupCount`（被隐藏的失效组数）
与 `pruneBlocked`（清理会被拒绝的原因，无阻塞时是 `null` 而不是省略）。两者由
`view::list_payload` **一处**装配（`list_at_with_index_root` 与用例共用），并且与批量清理
**同一趟判定**产出：

- 主表可读：`blocked` 非空 ⇔ `prune_dead_groups` 会拒绝，文案逐字相同；
- 主表损坏（`Unavailable`）：`prune` 返回 `Err`，所以 `pruneBlocked` 是同一句
  「会话关联组暂不可用：…」（`store_unavailable_reason` 是唯一来源，`detail` / `members` /
  `sync` 的错误也共用它）；
- 主表不存在（`Missing`，首次使用）：`prune` 返回 0 组而**不是**拒绝，`pruneBlocked`
  必须保持 `null`。

因此前端可以把「可点但必失败」的入口改成「禁用 + 展示原因」，不会出现前后端判定不一致。
`list_at` 只替换关联存储路径（数据根按客户端解析），`list_at_with_index_root` 连插件 / IDE
的数据根一起注入：用例因此能覆盖 `Ready` 分支从扫描结论到载荷的最后一跳（数据根解析不到
即是阻塞），而卡片正文无法注入、只用「主表里没有组」的现场跑通，全程不读真实客户端数据。

## 前端分层

`src/` 是主界面（React 19 + Vite + Tailwind + shadcn/Radix），`companion-ui/` 是悬浮窗的
独立 Vite 工程，两者不共享代码。

| 目录 | 职责 |
| --- | --- |
| `src/main.tsx`、`src/App.tsx` | 挂载、全局错误处理、侧栏布局与 5 条懒加载路由；footer/更新中心在 `components/app-shell/` |
| `src/pages/` | 页面；页面目录内按区块拆文件：`sessions/hooks/{use-session-groups,use-session-detail,use-session-actions}.ts`、`accounts/hooks/use-accounts-page.ts`、`accounts/components/`、`settings/hooks/use-config-autosave.ts` 等 |
| `src/components/` | 跨页面组件；`ui/` 是 shadcn 基础件；`account-card/` 账号卡片区块；`client-switch-account-*` 两个客户端共享切号弹窗；`app-shell/` 是侧栏 footer 与更新中心 |
| `src/hooks/` | `use-*` 轮询/可见性/更新状态/悬浮窗开关；`use-visible-interval` 是可见性轮询的唯一实现 |
| `src/lib/` | 纯逻辑与契约：`api/`（双通道）、`types/`（按域拆分）、`screenshot-demo/`（按域 fixtures + 分发）、`persisted-preference.ts`（localStorage 偏好唯一实现）、主题/格式化等 |
| `src/stores/` | zustand 状态；目前只有 `accounts.ts`（账号库、状态快照、轮询去重） |

### API 双通道

`src/lib/api/` 按业务域拆文件，统一从 `src/lib/api/index.ts` 再导出；页面只
`import * as api from "@/lib/api"`。核心适配在 `src/lib/api/core.ts`：

- 桌面 App：`invoke(cmd, args)` 调 Tauri command。
- WebUI：按 `ROUTES` 表把同一个 `cmd` 映射成 HTTP 方法与路径，`fetch` 同源服务
  （端口 / SSH 转发都跟随页面来源，不写死 `127.0.0.1:57910`）。
- 运行形态判定：`isWebui()`（无 `__TAURI_INTERNALS__`）、`isDesktop()`、`isDemoMode()`。
- 新增能力时，Tauri command 名必须与 `ROUTES` 的键一致，否则 WebUI 会报
  「webui 模式暂不支持该操作」。`node scripts/check-command-routes.mjs` 校验
  Tauri 注册、command 定义、ROUTES、axum 路由四方一致；`node scripts/check-api-contract.mjs`
  校验请求体 DTO 与 Tauri 参数一致；`node scripts/check-companion-contract.mjs`
  校验悬浮窗演示桥的 postMessage channel 两端一致；`node scripts/check-structure.mjs`
  校验长文件豁免表、core 依赖方向、旧名称与 capability 最小化。

### Demo Mode

只读演示跑的是同一份前端代码，数据由 `src/lib/screenshot-demo/`（按域 fixtures + 薄分发）
提供：

| 开关 | 作用 |
| --- | --- |
| `VITE_DEMO_MODE=1` | 只读演示：`demoModeEnabled` 生效，写操作统一抛「演示模式下不可操作」 |
| `VITE_SCREENSHOT_DEMO=1` | README 截图 / 演示构建，同样只读 |
| `VITE_PAGES_DEMO=1` | GitHub Pages 部署：改用 `HashRouter`、Vite `base=/workbuddy-shift/` |

`call()` 在演示模式下只放行 `DEMO_READ_COMMANDS` 白名单里的读命令，其余直接拒绝；
`pnpm run build:demo` 产出 Pages 用的静态站点（见 `.github/workflows/pages.yml`）。
`src/lib/demo-contract.test.ts` 校验白名单与演示分发不漂移；悬浮窗演示的 postMessage
channel 由 `scripts/check-companion-contract.mjs` 校验。

### 内置悬浮窗前端

`companion-ui/` 由 `scripts/build-companion.mjs` 驱动：嵌入包输出到
`public/companion/`（`desktop.html` + `desktop-settings.html`），运行时二进制输出到
`src-tauri/binaries/companion-runtime-<target>`。构建时机与目录细节见
[companion.md](companion.md)。

## 数据存储

工具自己的数据在 `~/.workbuddy-shift/`；开发构建把家目录整体重定向到 `~/.wb-shift-dev`，
存储根随之变成 `~/.wb-shift-dev/.workbuddy-shift/`（见下文「沙箱」）。客户端数据只读写
各自产品目录；路径常量集中在
`crates/wb-shift-core/src/modules/config/paths.rs` 与 `variant.rs`。

### 工具存储根（`~/.workbuddy-shift`）

| 文件 / 目录 | 内容 |
| --- | --- |
| `accounts.json` | 账号库（含凭据字段；原子写） |
| `backups/` | 切换前登录态备份 + 会话操作备份（见「备份与恢复」） |
| `auto_checkin_config.json`、`auto_checkin_logs.json`、`auto_travel_config.json`、`travel_cache.json` | 自动签到 / 自动旅行 |
| `auto_rotate_config.json`、`auto_rotate_logs.json`、`auto_rotate_notify.json` | CodeBuddy CLI 自动轮换 |
| `rate_limit_config.json`、`rate_limit_state.json`、`hook-events.jsonl` | 限额配置、入账状态、hook 事件追加文件 |
| `hook.sh`、`hook-backups/` | 限额 hook 脚本；安装前把客户端原配置备份到这里 |
| `credit_usage_snapshots.json`、`official_usage_cache.json` | 积分 / 官方用量缓存 |
| `workbuddy_exe.json`、`codebuddy_cn_app.json`、`codebuddy_ide_app.json` | 各客户端安装路径缓存 |
| `codebuddy_ide.json`、`codebuddy_cn_ide.json`、`vscode_ext.json`、`jetbrains.json` | 各客户端当前账号 / 切换状态 |
| `github_config.json`、`notifications.json`、`error.log` | 更新代理配置、提示存档、前端错误日志 |
| `session_links.json`、`session-links/` | WorkBuddy 关联组、基线、操作日志、预览凭据 |
| `vscode_session_links.json`、`vscode-session-links/` | VS Code 插件同构存储 |
| `codebuddy_ide_session_links.json`、`codebuddy-ide-session-links/` | CodeBuddy IDE 同构存储 |
| `locks/` | 跨进程的档位操作锁与会话操作锁 |

三个关联命名空间互不可见：文件名 / 目录名不同，避免 WorkBuddy、VS Code 插件、
CodeBuddy IDE 的关联关系互相污染（见 `session/paths.rs` 的 `LinkNamespace`）。

### 各客户端数据

| 客户端 | 读写的本机数据 |
| --- | --- |
| WorkBuddy 国内版 / 国际版 | 登录态 `…/CodeBuddyExtension/Data/Public/auth/workbuddy-desktop[-ai].info`；数据根 `~/.workbuddy` / `~/.workbuddy-ai`（会话正文 `projects/**/*.jsonl`、`workbuddy.db`、云端映射 `edge-sync-mapping*.db`、`logs/`） |
| CodeBuddy CLI | `~/.codebuddy/settings.json`、会话与日志目录；轮换状态 `~/.codebuddy-rotate/state.json` |
| CodeBuddy IDE（国内 / 国际） | 用户数据目录 `.../CodeBuddy CN` 或 `.../CodeBuddy` 下的 `User/globalStorage/state.vscdb`（Safe Storage secret），只读写登录态行 |
| VS Code（官方版） | `User/globalStorage/state.vscdb` 中 CodeBuddy 扩展的 secret；写入前要求 VS Code 退出 |
| JetBrains（IDEA / PyCharm） | 配置目录 `.../JetBrains/<产品><版本>/options/secret-storage.xml` 与已安装的 `plugins/coding-copilot*` |

WorkBuddy 的会话复制需要「正文 + `workbuddy.db` sessions 表」同时可用；数据根不具备该
结构时按不支持处理（`session::session_copy_supported_at`），不硬编码按档位放行。

### 备份与恢复

- **登录态**：切换前把客户端登录态文件复制到 `backups/workbuddy-desktop[-ai].<时间>.info`。
- **会话复制**：在操作专属目录 `backups/session-transactions/<variant>/<operationId>/`
  逐文件复制 `workbuddy.db`（含 `-wal` / `-shm`）并校验大小；备份不可信就不写业务数据。
- **会话同步**：同一目录布局下用 rusqlite 在线备份 API 生成数据库一致性快照，逐文件备份
  目标正文并记录摘要，写 `manifest.json` 记录覆盖前正文 / 目标行 / 恢复方法；备份不可信时零写入。
- **生命周期**：维护记录 `session-links/backup-lifecycle/` 标记 `allocating → protected →
  cleanupPending`；备份不完整时零写入，中断的操作由 `session/recovery.rs` 按同一份清单
  补完或回滚，可靠完成后再清理临时目录（`session_backup.rs`）。
- **写入范围**：复制 / 同步只写目标正文与 `workbuddy.db` 的 sessions 行；
  `edge-sync-mapping-*.db` 的云端映射行由客户端 edge-sync 扩展在下次启动时自行写入，
  本工具不预写（预写会让客户端判定「已迁移」从而跳过上传）。
- **VS Code / CodeBuddy IDE 会话**：复制 / 同步前把索引与正文备份到
  `backups/{vscode-sessions,codebuddy-ide-sessions}/<utc>/`。
- **hook 配置**：安装限额 hook 前把三处客户端 `settings.json` 备份到 `hook-backups/`，
  卸载时在「除本工具条目外没有其它改动」的前提下逐字节还原。
- **hook 脚本归属**：安装 / 卸载只动「本工具生成的正文」——`Current`（当前模板）与
  `Outdated`（`$HOME` 形态的最早一版，以及**骨架一致、只有事件文件路径不同**的同代模板，
  例如上一代数据目录留下的那一份）；骨架认不出的一律按「用户改过」保留。历史注册条目
  （旧 `hook.cmd`、上一代数据目录里的同名脚本）在安装 / 卸载时按**具体绝对路径**认领并替换，
  不用宽泛片段认领第三方命令（旧名称例外登记见 `AGENTS.md` 第 7 节）。

### 加密（只描述现状）

- 工具**不加密**自己的 `accounts.json`；凭据字段来自客户端，WorkBuddy 5.6 的
  `$wbEncrypted` 加密信封原样保存、由客户端自行解密，本工具不降级成空串。
- 切到 CodeBuddy 系客户端时，按 Chromium / Electron Safe Storage 的既有模型写入目标
  自己的加密存储：macOS Keychain → PBKDF2-SHA1 → AES-128-CBC `v10`；Windows Local State
  + DPAPI → AES-256-GCM `v10`；Linux Secret Service → AES-128-CBC `v11`（无密钥环时退回
  peanuts 固定密钥）。实现集中在 `vscode_cn_inject.rs`，Linux 密钥环读取见
  `linux_keyring.rs`；工具不落盘明文主密码。
- 只读写目标客户端自己的数据库 / XML 字段，不做全库解密，也不导出解密后的凭据。

### 沙箱

`WB_SHIFT_HOME` 覆盖「家目录」，账号库、客户端数据、备份、hook 事件全部随之重定向。
开发构建（`tauri dev`）在未显式设置时默认指向 `~/.wb-shift-dev`，位置可用
`WB_DEV_SANDBOX` 指定；`WB_DEV_REAL_HOME=1` 才回到真实数据。实现见
`src-tauri/src/lib.rs` 的 `sandbox_dev_home()` 与 `config/paths.rs` 的 `dev_home_dir()`，使用说明见
[`scripts/README.md`](../scripts/README.md)。

## Tauri 2 权限与安全边界

权限按**实际窗口**最小化，capability 文件在 `src-tauri/capabilities/`：

| 窗口 | capability | 权限（最小集） | 依据 |
| --- | --- | --- | --- |
| `main` | `default.json` | `core:event:default`；`core:window:allow-is-visible`；`core:window:allow-start-dragging`；`core:window:allow-internal-toggle-maximize`；`opener:default`；`dialog:allow-save` | 前端只监听事件、调 `isVisible()`、用 `data-tauri-drag-region`、`plugin-opener`/`plugin-dialog`；更新/通知由 Rust 内部完成，不需要前端 capability |
| `companion-rail`、`companion-settings` | `companion.json` | `core:event:default`；`core:window:allow-start-dragging`；`notification:allow-is-permission-granted`；`notification:allow-request-permission`；`companion:default` | companion-ui 只 listen 事件、调 `plugin:companion|` 命令、请求通知权限、拖动悬浮栏 |

- 不新增 fs / shell / process 等宽泛 capability；Rust 需要的能力（opener、autostart、单实例、更新）
  由 Rust API 直接调用，不暴露给前端。
- `app.security.csp` 是生产 CSP：`default-src 'self'`、`script-src 'self'`、允许内联样式、
  只放行 `ipc:` / `http://ipc.localhost` 连接、禁止 object/base/form 外发。
  `app.security.devCsp` 仅供 `tauri dev`（Vite HMR/React preamble），生产 CSP 不放宽。
  `dangerousDisableAssetCspModification: ["style-src"]` 只关闭 style-src 的 nonce/hash 注入
  （React 运行时设置 style 属性需要），其余指令仍由 Tauri 注入。
- companion 的窗口 label、capability identifier、插件名、invoke 前缀、事件名必须成组修改，
  见 [companion.md](companion.md) 的「运行时标识」表。
- WebUI（本地 HTTP）安全边界：只绑定 127.0.0.1；Host 校验防 DNS rebinding；Origin 校验防
  跨站 simple request（CSRF）；请求体上限 2 MiB；静态资源来自 `rust-embed` 编译期表
  （无磁盘路径、带 `X-Content-Type-Options: nosniff`）；未知 `/api/*` 返回 404 而不是
  SPA 回退。上述行为都有 `crates/wb-shift-server/src/api/tests.rs` 覆盖。

## 长文件与拆分计划

阈值（[AGENTS.md](../AGENTS.md)）：手写源码以 ~300 行为目标，超过 500 行必须拆分，或在本文登记理由与后续计划。

本轮（Round 3）已完成的拆分：

- `wb-shift-core` 的生产模块全部目录化：`session/`、`session_link/`（含 `tests/`）、`process/{windows/}`、`config/{paths,store,http,configs/}`、`limits/{scan,parse/,ledger/,payload}`、`rate_limit_events/`、`rate_limit_hook/`、`credits/`、`token_stats/`、`checkin/`、`session_groups/`、`session_backup/`、`travel/`、`account/`、`official_usage/`、`credit_usage/`、`rotate/`、`export_import/`、`auth_file/`、`switch/`、`codebuddy_cli/`、`codebuddy_ide/`、`codebuddy_cn_ide/`、`codebuddy_ide_session{,_sync}/`、`vscode_session{,_sync,_link}/`、`vscode_ext/`、`vscode_cn_inject/`、`jetbrains/`；仅 `variant.rs`、`oauth.rs`、`refresh.rs`、`notifications.rs`、`error_log.rs`、`update.rs`、`linux_keyring.rs`、`session/link` 的少量入口文件保持单文件。
- 宿主：`src-tauri/src/commands/`、`src-tauri/src/tray/`、`src-tauri/src/update_service/`、`crates/wb-shift-server/src/api/{handlers/,router,host_guard,response,state,static_assets}`。
- Companion：`companion-desktop/src/{plugin/,rail_settings/autostart/,rail_position/,session/}`、`companion-core/src/adapters/{workbuddy/,codeg/{events,child}}/`、`companion-core/src/custom/engine/`、`companion-runtime/src/serve.rs`、`companion-core/tests/lifecycle/`。
- 前端：`pages/sessions`、`pages/accounts`、`settings/auto-*`、`screenshot-demo/`、`App.tsx`、`switch-account-dialog/`、`client-switch-account-*`、`app-shell/`、`persisted-preference` 等；主界面 `src/` 已无 >500 行手写文件，`companion-ui/` 只剩 `desktop/rail-controller.ts`（见表）。
- 去重：`is_unauthorized`/`parse_number`（`config/http.rs`）、`count_items`（`config/store.rs`）、`auth_file_path` 转发 shim 删除（统一 `WbVariant::auth_file_path()`）；`preview helper` 共享因触及同步凭据路径、风险大而暂缓（见下）。

### 仍未拆分的 >500 行文件（逐项登记）

行数口径：`wc -l` 的**总行数**（含空行与注释）。核对时请用同一口径；只看非空行会得到更小的
数字（例如 `rail-controller.ts` 非空 671 行 / 总 734 行），容易误判为「文档过时」。

| 文件 | 行数 | 类别 | 理由 | 后续计划 |
| --- | ---: | --- | --- | --- |
| `crates/wb-shift-core/src/modules/session/tests/copy.rs` | 2024 | 测试 | 本轮拆分后的测试分组（原 session/tests.rs 6696 行按主题拆分），不属于生产逻辑；再按行数机械切只增加跳转成本。 | 测试文件继续显著膨胀时，按用例主题再拆；当前保留。 |
| `crates/wb-shift-core/src/modules/limits/tests.rs` | 1744 | 测试 | 限额模块测试整体随模块搬迁，测试名与基线一致。 | 同上，保留。 |
| `crates/wb-shift-core/src/modules/session/tests/recovery.rs` | 1429 | 测试 | 会话恢复主题测试分组。 | 同上，保留。 |
| `crates/wb-shift-core/src/modules/config/tests.rs` | 1063 | 测试 | 配置模块测试整体随模块搬迁。 | 同上，保留。 |
| `crates/wb-shift-core/src/modules/session/tests/sync.rs` | 1252 | 测试 | 会话同步主题测试分组。 | 同上，保留。 |
| `crates/wb-shift-core/src/modules/session/tests/exec.rs` | 839 | 测试 | 会话执行/恢复主题测试分组。 | 同上，保留。 |
| `crates/wb-shift-core/src/modules/session/tests/run.rs` | 864 | 测试 | 会话编排主题测试分组。 | 同上，保留。 |
| `companion-ui/src/desktop/rail-controller.ts` | 734 | 单一状态所有者 | 悬浮栏的唯一状态所有者（文件头已声明设计意图），拆分会让同一份布局/动画状态跨文件。 | 保持单文件；如继续增长，先抽纯函数（布局计算），状态仍留在 controller。 |
| `crates/companion-core/tests/lifecycle/codex.rs` | 726 | 测试 | Codex 生命周期集成测试分组（原 lifecycle.rs 拆出）。 | 保留；测试文件继续膨胀时按场景再拆。 |
| `crates/wb-shift-core/src/modules/process/macos.rs` | 714 | 平台代码 | macOS 进程枚举/关闭/启动与 app 路径探测集中在单文件，整模块 cfg(macos) 门控；本机 Windows 无法编译验证，先不二次拆。 | macOS CI 覆盖后按「进程控制」「app 路径解析」再拆。 |
| `crates/wb-shift-core/src/modules/variant.rs` | 650 | 单一来源 | 档位（国内/国际）规则与路径的单一来源，刻意集中；拆开会让同一规则出现多个入口。 | 保持单文件；新增档位时只在这里扩展。 |
| `crates/wb-shift-core/src/modules/session_link/tests/sync_decision.rs` | 578 | 测试 | 同步判定测试分组。 | 保留。 |
| `crates/companion-core/tests/custom.rs` | 541 | 测试 | 自定义集成集成测试，整体随上游迁移。 | 保留。 |
| `crates/wb-shift-server/src/api/tests.rs` | 552 | 测试 | api 层边界测试集中在一个文件（SPA 回退 / Host 边界 / 请求体 DTO 契约 / 切换运行标记 / 清理接口的破坏性默认），共享 `router_request`、`post_json`、`router_status` 三个 helper，按域拆文件会把它们复制到每个子文件。 | 保留；边界层测试再显著膨胀时，先把三个 helper 抽到 `api/tests/support.rs` 再按域拆。 |
| `crates/wb-shift-core/src/modules/session_groups/tests/plugin_liveness.rs` | 823 | 测试 | 插件 / IDE 的存活判定主题测试组（会话目录证据、空工作区分母、`Partial` 判不了时清理入口的 `pruneBlocked`、隐藏集合 ≡ 清理集合、客户端与档位隔离），共用 `tests/editor.rs` 的 `TempStore` 造数夹具；按主题分组而非按行数拆分。 | 继续膨胀时把 `pruneBlocked` 相关用例拆到 `tests/prune_blocked.rs`。 |
| `crates/wb-shift-core/src/modules/session_groups/view.rs` | 638 | 视图装配 | 列表/详情/卡片载荷共用一份成员快照与判定矩阵；拆开会让「同一份摘要口径」跨文件传递，先保持单文件。列表的存活过滤按客户端分派（WorkBuddy 读库、插件/IDE 走 `liveness`），与清理共用同一份组级汇总。 | 继续增长时把卡片载荷装配（`group_payload`）与共用口径（`member_snapshot`/`summary_text`）按职责再拆。 |
| `crates/wb-shift-core/src/modules/vscode_session_sync/tests/sync.rs` | 522 | 测试 | 插件侧同步/预览主题测试分组（含代表成员与存活过滤用例）。 | 保留；测试继续膨胀时按「复制登记」「预览」「同步执行」再拆。 |
| `crates/wb-shift-core/src/modules/session_groups/tests/maintenance.rs` | 1131 | 测试 | 失效组清理与列表过滤的判定/备份/拒绝语义共用一套造数夹具（临时工具存储根、`sessions` 表、插件工作区索引）；补齐插件与 IDE 的清理、索引部分损坏（`Partial`）的拒绝与放行、无失效组时不重写主表、两客户端互不串用等用例后超过阈值，属测试分组而非生产逻辑。 | 继续膨胀时按「WorkBuddy（`sessions` 表）」与「插件 / IDE（工作区索引）」两个主题拆成 `maintenance.rs` + `plugin_prune.rs`。 |
| `crates/wb-shift-core/src/modules/session_groups/tests/editor.rs` | 550 | 测试 | 管理页/客户端作用域/目标兼容性测试与共享夹具 `TempStore`（临时工具存储根、`sessions` 表、插件工作区索引造数）集中在一个文件；夹具被同模块多个 `include!` 的测试共用，按行数拆开只会把同一份造数逻辑复制多份。 | 夹具继续增长时抽到 `tests/support.rs`（与 `vscode_session_sync/tests/fixtures.rs` 同构）。 |

> 该表由 `node scripts/check-structure.mjs` **双向校验**：未登记的 >500 行文件、行数与实际不符、
> 已缩小到 500 行以内但条目未删，都会让 CI 失败。新增文件必须先拆分，或按同一格式登记理由与计划。

### 已知 lint 抑制（类级登记，2026-11）

`#[allow(unused_imports)]` 从 **180 降到 91**（2026-11-16 第一轮按模块清理，已清零 89 处）。

第一轮已清零的模块（每个都通过 `cargo check --all-targets` + 模块单测）：
`auth_file`、`account`、`travel`、`official_usage`、`rotate`、`rate_limit_hook`、
`export_import`、`credit_usage`、`switch`、`vscode_session_link`、
`codebuddy_ide_session`、`codebuddy_ide_session_sync`。

剩余 91 处按成因分三类：

| 类别 | 数量 | 为什么不能直接删 |
| --- | ---: | --- |
| 测试可见性（`use super::*` + `include!`） | 22 | 删 allow 后 lib 目标报未使用，但测试目标靠它取符号 |
| `pub(crate) use` 重导出 | 17 | 同样只被测试通过 `use super::*` 使用 |
| 兄弟模块互引 / 平台 cfg | 52 | `copy ↔ paths`、`status ↔ switch` 这类环，且部分符号只在 macOS / Linux 分支使用 |

- **理由**：模块按职责拆分后，子文件用 glob 复用兄弟模块符号；测试模块再通过
  `use super::*` 复用这些导入。部分符号只在 `cfg(target_os = ...)` 分支使用，
  在 Windows 上不可见，单一开发机无法验证删除后的跨平台影响。
- **影响**：可能掩盖真正的冗余导入（第一轮已清掉 89 处）；不影响运行时代码与产物。
- **后续计划**：按模块继续（下一批：`limits`、`session`、`config`、`vscode_session`、
  `codebuddy_cn_ide`、`codebuddy_ide`、`jetbrains`、`vscode_ext`、`session_backup`、
  `vscode_cn_inject`）。每个模块的验收标准：`cargo check --all-targets` 无 warning、
  该模块单测通过、该模块 allow 归零；清完一批下调 `check-structure.mjs` 的基线，
  归零后删除本条登记。
- **登记位置**：本条 + `scripts/check-structure.mjs` 的 `ALLOW_UNUSED_IMPORTS_BASELINE`
  （棘轮：新增即失败）。

### 下一轮候选（已识别，未在本轮执行）

- 未迁移的 `Json<Value>` handler 见 `scripts/api-contract-baseline.json`：客户端切号 8 个（共用
  `copySessions` / `syncSelections` 嵌套类型）、签到 2 个、只读统计 1 个、通知 1 个、token 刷新 1 个、
  配置保存 5 个（配置 schema 在 core）。迁移顺序建议：客户端切号 → 签到 → 只读查询；每迁完一个就从
  baseline 删掉对应条目（脚本会拒绝过期登记）。
- `preview helper` 共享：`session/sync/preview.rs` 与 `vscode_session_sync/preview.rs` 的 4 个 helper 逐字相同、`live_preview_binding` 仅差 archive 字段；需要允许调整 `session_link::preview` 可见性并清理 `session/mod.rs` 导入面后一次完成，完成后必须跑同步凭据相关测试。
- `session/tests`、`limits/tests`、`config/tests`、`companion-core/tests` 继续膨胀时按用例主题再拆。
- `process/macos.rs` 等平台文件在 macOS CI 稳定后按平台职责再拆。
## 构建与发布入口

| 入口 | 命令 | 产物 |
| --- | --- | --- |
| 前端开发 | `pnpm run dev` | Vite dev server（浏览器预览，配合 WebUI） |
| 桌面开发 | `pnpm run tauri dev`（`beforeDevCommand: pnpm run dev:desktop`） | 桌面 App + 嵌入悬浮窗资源 |
| 前端生产构建 | `pnpm run build` | `dist/`（Tauri 与 WebUI 共用） |
| 桌面打包 | `pnpm run tauri build`（`beforeBuildCommand: pnpm run build:desktop`） | 安装包；签名更新包 |
| WebUI / npm 包 | `cargo build -p wb-shift-server --release` | `workbuddy-shift` 二进制，由 `npm/` 平台分包分发 |
| 公开演示 | `pnpm run build:demo` | GitHub Pages 静态站点（`dist/`） |
| 悬浮窗资源 | `node scripts/build-companion.mjs dev\|build\|demo-dev\|demo-build\|clean` | `public/companion/`、`src-tauri/binaries/` |

Tauri 配置在 [`src-tauri/tauri.conf.json`](../src-tauri/tauri.conf.json)：`frontendDist` 指向
`../dist`，`externalBin` 打包 `companion-runtime`，更新公钥与 release 端点也在那里。

### CI

| workflow | 触发 | 内容 |
| --- | --- | --- |
| `.github/workflows/test.yml` | push / PR / 手动 | 三平台 `cargo test -p wb-shift-core` + `companion-core/runtime` 测试 + Linux/Windows clippy（Linux 含 companion-core/runtime）；macOS lint job（`pnpm install --frozen-lockfile` + `pnpm run build`、`cargo fmt --all`、clippy 含 companion 三 crate、server / desktop / companion-desktop 测试、companion-ui typecheck + build:embed、6 个仓库检查脚本 versions / command-routes / api-contract / rust-locks / structure / companion-contract）；前端 `pnpm run lint` + `pnpm run typecheck` + `pnpm test`；msrv job（`cargo +1.95 check` 四个非 Tauri crate） |
| `.github/workflows/build.yml` | `v*` tag / 手动 | 四平台桌面矩阵 + `npm-linux-arm64` job（arm64 runner 原生编译 CLI，不参与桌面发布）：前端构建、server 二进制、npm 平台分包与主包发布、桌面安装包、更新清单、GitHub Release |
| `.github/workflows/pages.yml` | push `main` / 手动 | `pnpm run build:demo` → GitHub Pages |

发版细节与签名密钥位置见 [DEVELOPMENT.md](DEVELOPMENT.md)。
