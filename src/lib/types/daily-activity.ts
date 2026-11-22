/**
 * 国际版每日活跃会话（issue #214）的结果契约。
 *
 * 与 core 的 `AccountRunResult`
 * （`crates/wb-shift-core/src/modules/daily_activity/protocol/mod.rs`）逐字段一致：
 * 后端 `#[serde(rename_all = "camelCase")]` 序列化后**原样透传**，两个宿主
 * （Tauri command / HTTP handler）都不做二次加工。因此这里的字段名改动必须与 core
 * 同步，否则前端面板会静默丢字段。
 */

/**
 * 一轮结束时的阶段。`unsupported` / `skipped` 是**前置检查**的结果（未发任何请求），
 * 其余是会话流程的五步。
 */
export type DailyActivityPhase =
  | "unsupported"
  | "skipped"
  | "create"
  | "session"
  | "turn"
  | "poll"
  | "done";

/** 单个账号的每日活跃结果（逐账号返回，单账号失败不影响其它账号）。 */
export interface DailyActivityAccountResult {
  /** 账号 key：`id` 优先，退 `uid`（与后端每日去重 key 同源）。 */
  accountId: string;
  /** 本轮是否跑完（会话 `completed`）。注意 `skipped` 行的 `ok` 也是 `true`。 */
  ok: boolean;
  /** 今天已经做过、本轮直接跳过（未发任何请求）。 */
  skipped: boolean;
  phase: DailyActivityPhase;
  /** 实际使用的模型 id（`hy3` 免费 / `deepseek-v4.1-flash` 付费备选）。 */
  model?: string | null;
  /** `session/set_model` 是否被服务端接受；`false` = 本轮按会话默认模型跑完。 */
  modelApplied?: boolean | null;
  /** 尝试过的模型，按顺序。 */
  modelsTried?: string[];
  /** 会话最后状态（`completed` / `failed` / …）。 */
  status?: string | null;
  /** 收尾动作：`stop`（成功）/ `delete`（失败）。 */
  cleanup?: string | null;
  /** 收尾请求是否被服务端接受。 */
  cleanupOk?: boolean | null;
  /** 本轮耗时（毫秒）。 */
  elapsedMs?: number;
  /** 失败原因（后端已脱敏，不含 token / 沙箱地址 query）。 */
  error?: string | null;
  /** 不影响结果的提示（状态文件写入失败、收尾失败等）。 */
  notice?: string | null;
}

/** 一次手动触发的完整返回：整轮只取一次本地日期，跨零点不会一半今天一半昨天。 */
export interface DailyActivityRunResult {
  /** 本地日期 `YYYY-MM-DD`（后端 `local_today()`）。 */
  date: string;
  accounts: DailyActivityAccountResult[];
}
