/**
 * 失败档位的**线格式**词汇（与 `wb-shift-core` 的 `FailureKind::as_wire` 逐字一致）。
 *
 * 放在 `types/` 而不是 `api/` 是因为它同时被两处需要：`api/failure.ts`（还原结构化错误）
 * 与 `types/stats.ts`（`CreditExpiry` 的 `ok:false` 结果会带它）。`types/` 不依赖 `api/`，
 * 反向导入会形成环。
 */

/** 后端 `failureKind` 的四个取值（四档，见 core 的 `config/failure.rs`）。 */
export type FailureKind = "login_expired" | "gateway_html" | "upstream" | "malformed";

/**
 * 界面侧的「下一步动作」（**不是**协议字段，由前端按档位映射）。
 *
 * - `relogin`：登录态没了，重新走一次登录；
 * - `check_proxy`：像是代理 / 网关拦下来了，去设置页检查更新代理；
 * - `copy_diagnostic`：连人话都没有，把诊断摘要复制去反馈。
 */
export type RecoveryAction = "relogin" | "check_proxy" | "copy_diagnostic";
