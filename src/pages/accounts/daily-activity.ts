/**
 * 账号页「每日活跃」的纯逻辑：结果分类、逐行文案、计数摘要与「确认后才发请求」的状态机。
 *
 * 抽在这里是为了能直接单测（本仓库 Vitest 跑在 node 环境，不引 jsdom）：组件只做渲染，
 * 判定与文案都不进 JSX。状态映射的四类（成功 / 今日已跳过 / 不支持 / 失败）来自
 * core 的 `AccountRunResult`，见 `crates/wb-shift-core/src/modules/daily_activity/protocol/mod.rs`。
 */

import { asError } from "@/lib/api";
import { displayName } from "@/lib/account-display";
import type { AccountMeta, DailyActivityAccountResult, DailyActivityRunResult } from "@/lib/types";

/** 结果面板展示的四类状态。 */
export type DailyActivityOutcome = "success" | "skipped" | "unsupported" | "failed";

/** `Badge` 组件可用的变体（与 `src/components/ui/badge.tsx` 的 variant 同名）。 */
export type DailyActivityBadge = "success" | "secondary" | "warning" | "danger";

const OUTCOME_LABELS: Record<DailyActivityOutcome, string> = {
  success: "成功",
  skipped: "今日已跳过",
  unsupported: "不支持",
  failed: "失败",
};

const OUTCOME_BADGES: Record<DailyActivityOutcome, DailyActivityBadge> = {
  success: "success",
  skipped: "secondary",
  // 「不支持」不是错误（国内版账号、缺凭据的账号本来就走不了这条路），用警告色而非红色。
  unsupported: "warning",
  failed: "danger",
};

/** 失败行的阶段文案：告诉用户卡在五步里的哪一步。 */
const PHASE_LABELS: Record<string, string> = {
  unsupported: "前置检查",
  skipped: "去重检查",
  create: "建会话",
  session: "取沙箱地址",
  turn: "会话执行",
  poll: "等待会话完成",
  done: "收尾",
};

/**
 * 单账号结果 → 四类展示状态。
 *
 * **判定顺序有语义**：`skipped` 行的 `ok` 也是 `true`（core 用它表示「不需要再跑」），
 * 所以必须先判 `skipped`；否则「今日已跳过」会被显示成「成功」，用户会以为又拿了一次奖励。
 */
export function classifyDailyActivity(row: DailyActivityAccountResult): DailyActivityOutcome {
  if (row.skipped) return "skipped";
  if (row.phase === "unsupported") return "unsupported";
  if (row.ok) return "success";
  return "failed";
}

/** 一行结果的渲染数据（徽标 + 文案 + 明细）。 */
export interface DailyActivityRowView {
  outcome: DailyActivityOutcome;
  label: string;
  badge: DailyActivityBadge;
  detail: string;
}

/** 单账号结果 → 一行文案。明细里不出现 token / 沙箱地址 query（后端已脱敏）。 */
export function describeDailyActivityRow(row: DailyActivityAccountResult): DailyActivityRowView {
  const outcome = classifyDailyActivity(row);
  const label = OUTCOME_LABELS[outcome];
  const badge = OUTCOME_BADGES[outcome];

  if (outcome === "success") {
    const parts: string[] = [];
    const model = (row.model ?? "").trim();
    if (model) {
      parts.push(
        row.modelApplied === false
          ? `模型 ${model}（set_model 未被接受，按会话默认模型跑完）`
          : `模型 ${model}`,
      );
    }
    if (row.cleanupOk === false) {
      const action = (row.cleanup ?? "").trim() || "stop";
      parts.push(`收尾 ${action} 未被服务端接受，网页端可能留下一条进行中的会话`);
    }
    if (row.notice) parts.push(row.notice);
    return { outcome, label, badge, detail: parts.join("；") || "会话已跑完，活跃奖励已计入" };
  }

  if (outcome === "skipped") {
    return {
      outcome,
      label,
      badge,
      detail: "今天已经完成过一次，本轮没有发任何请求（每日去重）",
    };
  }

  if (outcome === "unsupported") {
    return {
      outcome,
      label,
      badge,
      detail: (row.error ?? "").trim() || "该账号不支持每日活跃会话",
    };
  }

  const phase = PHASE_LABELS[row.phase] ?? row.phase;
  const tried =
    row.modelsTried && row.modelsTried.length > 0
      ? `（已尝试 ${row.modelsTried.join(" → ")}）`
      : "";
  const reason = (row.error ?? "").trim() || "未知原因";
  return { outcome, label, badge, detail: `${phase}失败${tried}：${reason}` };
}

/**
 * 结果行里的账号显示名。
 *
 * `accountId` 是后端去重 key（`id` 优先，退 `uid`），所以两个字段都要比；找不到时退回
 * key 本身，绝不显示空白行（空白行等于静默忽略这个账号）。
 */
export function dailyActivityAccountLabel(accounts: AccountMeta[], accountId: string): string {
  const key = accountId.trim();
  const hit = accounts.find(
    (account) => account.id === key || (account.uid ?? "").trim() === key,
  );
  if (hit) return displayName(hit);
  return key || "未知账号";
}

/** 一轮结果的计数摘要（面板标题右侧的一行字）。 */
export function summarizeDailyActivity(result: DailyActivityRunResult): string {
  const counts: Record<DailyActivityOutcome, number> = {
    success: 0,
    skipped: 0,
    unsupported: 0,
    failed: 0,
  };
  for (const row of result.accounts) counts[classifyDailyActivity(row)] += 1;

  const total = result.accounts.length;
  if (total === 0) return "账号库里还没有账号";
  const parts: string[] = [];
  if (counts.success > 0) parts.push(`${counts.success} 个成功`);
  if (counts.skipped > 0) parts.push(`${counts.skipped} 个今日已跳过`);
  if (counts.unsupported > 0) parts.push(`${counts.unsupported} 个不支持`);
  if (counts.failed > 0) parts.push(`${counts.failed} 个失败`);
  return `共 ${total} 个账号：${parts.join("、")}`;
}

/** 面板状态：进行中 / 最近一次结果 / 最近一次失败原因。 */
export interface DailyActivityState {
  /** 正在跑：按钮禁用 + 显示进度态。 */
  running: boolean;
  /** 最近一次成功返回的逐账号结果；失败时**保留**上一次的值。 */
  result: DailyActivityRunResult | null;
  /** 最近一次失败原因（整体失败，不是单账号失败）；成功或关闭面板后清空。 */
  error: string | null;
}

/** 面板初始状态（也用于「关闭结果面板」）。 */
export const EMPTY_DAILY_ACTIVITY_STATE: DailyActivityState = {
  running: false,
  result: null,
  error: null,
};

/** 真正发请求的函数（生产传 `api.runDailyActivity`，单测传桩）。 */
export type DailyActivityRunner = () => Promise<DailyActivityRunResult>;

/** 一次尝试的入参：状态快照 + 是否已确认 + runner + 状态发布函数。 */
export interface DailyActivityAttempt {
  /** 当前状态快照（用于判断是否已在进行中、以及失败时保留哪份结果）。 */
  state: DailyActivityState;
  /** 用户在确认框里点了「开始」才是 `true`；取消 / 没点过一律 `false`。 */
  confirmed: boolean;
  runner: DailyActivityRunner;
  /** 状态发布（生产传 `setState`，单测收集调用序列）。 */
  publish: (next: DailyActivityState) => void;
}

/**
 * 确认框之后的**唯一**请求入口。
 *
 * - `confirmed === false`（用户没在确认框点「开始」）→ 直接返回，**不调用 runner**，
 *   也不 `publish` 任何状态：未确认不能发出请求；
 * - 已在进行中 → 直接返回，连点不会发第二个请求；
 * - runner 抛错 → 复位 `running`、写入 `error`，但**保留上一次的 `result`**：
 *   失败的这一轮不清空用户已经看到的结果。
 */
export async function runDailyActivityAttempt({
  state,
  confirmed,
  runner,
  publish,
}: DailyActivityAttempt): Promise<void> {
  if (!confirmed || state.running) return;
  publish({ ...state, running: true, error: null });
  try {
    const result = await runner();
    publish({ running: false, result, error: null });
  } catch (error) {
    publish({ running: false, result: state.result, error: asError(error) });
  }
}
