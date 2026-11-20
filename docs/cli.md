# workbuddy-shift 命令行（AI 友好子命令）

面向「主控 AI 直接调用命令行」的场景：一次拿到整个账号池的账号、积分、限额与
活跃 CLI 会话，自行决定等任务结束后切号。

```bash
workbuddy-shift inspect --summary     # 一次拿全：账号 + 积分 + 限额 + 当前账号
workbuddy-shift status --json         # 只看 CLI 当前账号 / 是否有会话在跑
workbuddy-shift accounts --json       # 账号列表（不含密钥）
workbuddy-shift limits  --json        # 各账号观察到的模型限额
workbuddy-shift credits --json        # 各账号积分（默认实时，--cached 走本地快照）
workbuddy-shift switch <id> --dry-run # 预览切号会影响哪些 CLI 进程（不写任何状态）
```

## 通用约定

| 项 | 约定 |
| --- | --- |
| stdout | **只输出 JSON**。单行紧凑格式，可直接 `JSON.parse` / `jq`；键顺序为字典序，不要依赖顺序 |
| stderr | 进度与警告（`[credits] 3/12 …`）。stdout 不含任何非 JSON 文本 |
| `schemaVersion` | 每条输出都带，当前为 `1`。字段语义变化时递增 |
| 密钥 | 输出只用白名单字段，**不含** `access_token` / `refresh_token` / `auth_raw` |
| 只读边界 | `accounts` / `limits` / `status` / `inspect` 不写本地状态；`switch` 只在非 `--dry-run` 且未被拒绝时写 |
| 时区/单位 | 所有时间戳是**毫秒**（与 `Date.now()` 一致） |

## 退出码

| 码 | 含义 |
| --- | --- |
| `0` | 成功（含 `partial: true` 的部分结果） |
| `1` | 目标不存在（`switch` / `inspect` 给的 id 在账号池与会话里都找不到） |
| `2` | 用法错误（未知子命令 / 未知选项 / 缺少参数 / 参数值非法） |
| `3` | 拒绝执行：有活跃 CodeBuddy CLI 会话时请求切号（未带 `--force`） |
| `4` | 致命错误（账号库存在但读不出 / 解析失败、切号执行失败等） |

错误时 stdout 仍是可解析的 JSON：`{"schemaVersion":1,"ok":false,"error":"…"}`，
同时 stderr 有一行 `[cli] …` 给人看。

## `accounts [--json]`

```json
{
  "schemaVersion": 1,
  "accounts": [
    {
      "id": "6a82893f-…",
      "uid": "3d3fbea0-…",
      "name": "软件学院向阳工作室",
      "variant": "cn",
      "cliCurrent": false,
      "needsRelogin": false,
      "expiresAt": 1797588951426,
      "refreshExpiresAt": 1797761750426
    }
  ]
}
```

- `cliCurrent` 来自 **CodeBuddy CLI** 的轮换状态（`~/.codebuddy*/settings.json` +
  `~/.codebuddy-rotate/state.json`），**不是** GUI 的当前账号。
- `name` 是 core 的展示名（备注 / 手机号 / 昵称 / 邮箱按用户设置优先级）。
- 账号库文件缺失时返回 `accounts: []` 且退出码 `0`（新机器就是这种形态）；
  文件存在但读不出 / 不是 JSON 数组时退出码 `4`。

## `limits [--json]`

```json
{
  "schemaVersion": 1,
  "scannedAt": 1795190259532,
  "windowDays": 2,
  "accounts": [
    { "id": "6fe6ec35-…", "limits": [{ "model": "deepseek-v4.1-flash", "resetAt": 1795251140000, "source": "workbuddy-cn" }] },
    { "id": "fe859199-…", "limits": [] }
  ]
}
```

- **`limits: []` 表示「没有观察到限额」，不是「模型可用」。** 没有记录时 AI 必须按
  `unknown` 处理，不能推定模型可用。
- `scannedAt` 是最近一次真实扫描的时刻（台账 5 分钟节流）；`windowDays` 是日志观察窗口。
  `scannedAt` 缺失或明显过期时同样按 `unknown` 处理。
- 来源 `source` 取值：`workbuddy-cn` / `workbuddy-ai` / `codebuddy-cli` /
  `codebuddy-ide` / `codebuddy-cn-ide` / `vscode-ext`。
- 不做跨账号推断，也不合并「借用的限额」（台账里的 `borrowed` 不下发）。
- 账号池里的每个账号都有一条记录（顺序与 `accounts` 一致）；台账里有、账号池里已删除的
  条目不下发。

## `credits [--json] [--cached] [--timeout <秒>]`

```json
{
  "schemaVersion": 1,
  "partial": false,
  "accounts": [
    {
      "id": "6a82893f-…",
      "source": "live",
      "ok": true,
      "remaining": 1710.79000718,
      "total": 4035.0,
      "expiringSoonRemaining": 0.0,
      "soonestExpireAt": 1796490637000,
      "observedAt": 1795190263655
    }
  ]
}
```

- **默认实时**：并发 4，整体受 `--timeout`（默认 8s，上限 120s）约束。
  超时或单账号失败**不 fail-fast**：该账号写 `{ "ok": false, "error": "…" }`，
  顶层 `partial: true`，其余账号照常返回（退出码仍是 `0`）。
- `--cached`：读本地 `credit_usage` 快照（`~/.workbuddy-shift/credit_usage_snapshots.json`
  的最近一条），`source: "cache"`，带 `observedAt`；
  `expiringSoonRemaining` / `soonestExpireAt` 恒为 `null`（快照没有到期资源）。
  没有快照的账号 `ok: false` 且顶层 `partial: true`。
- 实时查询的副作用与 GUI 查积分完全一致：会（a）在必要时刷新并落盘账号 token，
  （b）把观察值追加到本地积分快照账本。除此之外不写状态。
- `expiringSoonRemaining` 只统计「即将到期」的资源；不区分套餐类别。

## `status --json`

```json
{ "schemaVersion": 1, "cli": { "current": "fe859199-…", "running": false }, "poolSize": 12 }
```

- `cli.current`：CodeBuddy CLI 当前账号 id，取不到时为 `null`。
- `cli.running`：是否存在心跳新鲜的 CLI 会话（`~/.codebuddy/sessions/*.json`，120s 窗口）。
- 裸 `status`（不带 `--json`）保持改造前的终端文本输出，供人和旧脚本使用。

## `inspect [--summary] [--cached] [--no-credits] [--timeout <秒>]`

整池只读体检。默认就是 `--summary`：

```json
{
  "schemaVersion": 1,
  "generatedAt": 1795190285661,
  "partial": false,
  "current": "fe859199-…",
  "scannedAt": 1795190284898,
  "windowDays": 2,
  "accounts": [
    {
      "id": "6fe6ec35-…", "uid": "a3525d8c-…", "name": "林小凤", "variant": "cn",
      "cliCurrent": false, "needsRelogin": false,
      "expiresAt": 1797588951962, "refreshExpiresAt": 1797761750962,
      "credits": { "source": "live", "ok": true, "remaining": 2815.02, "total": 5005.0,
                   "expiringSoonRemaining": 0.0, "soonestExpireAt": 1796054399000,
                   "observedAt": 1795190284906 },
      "limits": [{ "model": "deepseek-v4.1-flash", "resetAt": 1795251140000, "source": "workbuddy-cn" }]
    }
  ]
}
```

- `--no-credits`：跳过网络，每个账号的 `credits` 为 `null`，`partial` 保持 `false`。
- `--cached`：积分走本地快照（语义同 `credits --cached`）。
- 12 个账号、网络正常时实测 ≈ 2.4s（验收要求 ≤ 8s）。

### `inspect <账号 id | uid | 会话 id>`

单目标排查（只读）：

- 命中账号 → `{ "kind": "account", "account": { …同上，内嵌 credits/limits… } }`
- 命中会话 id → 在账号池里反查归属账号：
  `{ "kind": "session", "session": { "id": …, "title": …, "cwd": …, "updatedAt": … }, "account": { … }, "limits": [ … ] }`
- 都不命中 → 退出码 `1`。

## `switch <id> [--dry-run] [--force]`

```bash
workbuddy-shift switch 6a82893f-… --dry-run
```

```json
{
  "schemaVersion": 1,
  "dryRun": true,
  "ok": true,
  "target": { "id": "6a82893f-…", "name": "软件学院向阳工作室" },
  "current": { "id": "fe859199-…", "name": "果妈" },
  "liveSessions": [{ "pid": 1234, "startedAt": 1795190306701 }],
  "willClose": 1,
  "running": true,
  "alreadyCurrent": false,
  "wouldRefuse": true,
  "reason": "有正在运行的 CodeBuddy CLI 会话；实际切号会被拒绝（需 --force）"
}
```

- `--dry-run` **不写任何状态**：不关进程、不写 `state.json`、不改 `settings.json`，
  退出码恒为 `0`；是否会被拒绝看 `wouldRefuse`。
- **有活跃 CLI 会话时实际切号默认拒绝**（退出码 `3`）：

  ```json
  { "schemaVersion": 1, "dryRun": false, "ok": false, "refused": "live-session",
    "target": { "id": "…", "name": "…" }, "previous": { "id": "…", "name": "…" },
    "liveSessions": [{ "pid": 1234, "startedAt": 1795190306701 }], "willClose": 1,
    "message": "有 1 个 CodeBuddy CLI 会话在运行；等任务结束后再切，或加 --force（当前会话会中断且不自动重开）" }
  ```

- 成功时输出 `previous` / `target` / `closed`（关闭的 CLI 进程数）/ `regionChanged` / `message`。
- 已经是当前账号且未带 `--force` → 直接返回 `alreadyCurrent: true`，不做任何写入。
  带 `--force` 会照常执行（配置被改坏时用来重新套用）。
- **`--force` 会关闭正在运行的 CodeBuddy CLI 进程，当前会话中断且不自动重开。**

## 已知风险

1. **没有跨进程账号锁**（本轮决策）：CLI 与 GUI/WebUI 同时写 `accounts.json` 仍是
   last-write-wins。`switch` 写 `state.json` / `settings.json` 同理。
   切号前请让主控 AI 串行化自己的写操作，不要在 GUI 正在刷新 token 时切号。
2. `switch` 的 `liveSessions` 清单是**尽力而为**的展示：core 只导出布尔版
   `has_live_session`，CLI 侧按同一 120s 心跳判据重读注册表。
   **拒绝判定以 core 为准**，两者判据一致，不会因为清单读不到就放行。
3. `credits` 实时查询会打官方接口：账号多时受总预算限制可能 `partial`，
   不建议为了提高覆盖率把并发或预算调大。
4. `limits` 依赖「本机日志 + hook 信号」：新机器、关闭限额监听、或客户端没接 hook 时
   一律是 `[]`（`unknown`），不是「可用」。

## 相关实现

| 文件 | 内容 |
| --- | --- |
| `crates/wb-shift-server/src/main.rs` | 子命令切分（`subcommand_index`）与分发 |
| `crates/wb-shift-server/src/cli/mod.rs` | 参数解析、JSON 输出、退出码、`accounts`/`limits`/`status` |
| `crates/wb-shift-server/src/cli/credits.rs` | 积分：并发 4 + 总预算 + `--cached` |
| `crates/wb-shift-server/src/cli/inspect.rs` | `inspect --summary` / `inspect <id>` |
| `crates/wb-shift-server/src/cli/switch.rs` | 会话清单、拒绝判定、`--dry-run` / `--force` |
