# 常见问题（FAQ）

## Token 统计：「缓存写入」为什么永远是 0？

**结论：这个 0 是真实值，不是统计缺陷。**

Token 统计页「Token 与调用趋势」里的 `缓存写入` 系列（以及导出里的「缓存写」列）在 WorkBuddy
数据源上恒为 0（0.0%），近 30 天合计 9.9B Token 时同样如此。原因是上游数据里根本没有这个量：

| 证据 | 数据 |
| --- | --- |
| 数据源 | `~/.workbuddy/projects/**/*.jsonl`，774 个文件 / 2.28 GB |
| `usage` 对象只有 4 个键 | `input_tokens` / `output_tokens` / `total_tokens` / `cache_read_input_tokens` —— **没有写入字段** |
| 写入字段只出现在 `providerData.rawUsage` | `prompt_cache_write_tokens`、`cache_creation_input_tokens`、`cache_write_tokens` |
| 全量扫描结果 | 183,613 条记录，`prompt_cache_write_tokens` 与 `cache_creation_input_tokens` 的最大值**均为 0** |
| 解码器口径正确 | `crates/wb-shift-core/src/modules/token_stats/projection.rs` 的 `CACHE_WRITE_KEYS` 覆盖上述四个别名；单测 `cache_write_uses_explicit_aliases_but_never_cache_miss` 钉死「不得把 `prompt_cache_miss_tokens` 当写入」 |
| 旁证 | 同批数据 `cache_read_input_tokens` 正常（1.3B、命中率 98.5%），说明上报链路是通的 |

DeepSeek 系的上下文缓存是**自动命中、按命中/未命中计价**，没有 Anthropic 式的「cache write」
计费项；WorkBuddy 的 `rawUsage` 保留这些字段只为 schema 兼容，在 DeepSeek 模型上从不填值。

### 为什么不隐藏这一列？

- 用户已明确选择**保持现状**：继续显示「缓存写入」系列与 0 值，不做自动隐藏、不加说明 tooltip。
- 若将来接入会上报缓存写入的模型（例如 Anthropic 系），该系列会自动出现非 0 值，**无需改代码**；
  届时再讨论「恒 0 时隐藏」才有意义。

### 相关维护规则

1. **不得**因为恒 0 删除 Rust 侧的写入解析（`CACHE_WRITE_KEYS` / `usage_fields` / `usage_from`）。
2. **不得**用 `prompt_cache_miss_tokens` 冒充写入量：它表示「新计算的（未命中缓存的）输入」。
3. **导出列必须与图表系列一一对得上**：图上有哪个 token 分类，导出就有哪一列。
   注意映射不是字面同名——导出「输入」列 = 图表「新增输入 + 缓存读取」（`input` 已含 `cacheRead`）；
   「总 Token」「请求数」是合计/计数列，不对应单个系列。任一侧增删分类都必须同时改另一侧
   （`src/pages/token-stats/trend-chart.tsx` 的 `TOKEN_SERIES` ↔ `src/pages/token-stats/export.ts`
   的 `TOTALS_HEADERS` / `totalsCells`）。
4. 计算口径 `total = input + output + cacheWrite` 不变：`cacheWrite` 恒 0 时对结果无影响。

### 想自证链路是通的？

构造一条 `prompt_cache_write_tokens > 0` 的 jsonl 再跑一次统计，界面与导出的「缓存写入」会显示
非 0 —— 说明解析与导出链路可用，只是源数据没有这个值。

### 注意范围

本结论只针对 WorkBuddy 数据源。本机没有 CodeBuddy CLI 数据（`~/.codebuddy/projects` 不存在），
IDE 的 `cacheWriteInputTokens` 路径未参与实测；若那份数据里有非 0 值，属于正常情况。
