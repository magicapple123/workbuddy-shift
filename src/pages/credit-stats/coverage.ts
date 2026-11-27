/**
 * 积分统计的口径判定与文案（纯逻辑，不依赖 React）。
 *
 * 背景（issue #225）：官方用量取数失败时页面会退回本地观察值。旧实现在只有「今天」
 * 一份快照时把今日值同时当成近 7 天与本月显示（28.7 / 28.7 / 28.7），用户无法分辨
 * 「真的没消耗」和「本地根本没见过历史」。这里把「窗口是否被本地快照完整覆盖」抽成
 * 可单测的纯函数，页面只负责渲染。
 */

import { formatAmount, formatDate } from "@/lib/format";
import { statsDateInRange, statsRangeDayCount, type StatsRangeKey } from "@/lib/stats-range";
import type {
  CreditLocalCoverage,
  CreditOfficialUsage,
  CreditStatistics,
} from "@/lib/types";

import { rangeUsage } from "./data";

/** 概览卡上的三个消耗窗口。 */
export type UsageWindowKey = "today" | "7d" | "month";

/** 数据来源标签：官方口径 / 本地观察口径。 */
export type CreditSourceBadge = "官方用量" | "本地观察";

/**
 * 本地观察的覆盖说明：`2026/11/10 ~ 2026/11/19（覆盖 10 天）`。
 *
 * 没有快照时返回「本地暂无快照」：官方不可用时用户必须能一眼看出「本地也没有数」，
 * 而不是把 0 当成「今天没消耗」。
 */
export function localCoverageText(coverage?: CreditLocalCoverage): string {
  if (!coverage || coverage.startAt === null) return "本地暂无快照";
  const end = coverage.endAt ?? coverage.startAt;
  return `${formatDate(coverage.startAt)} ~ ${formatDate(end)}（覆盖 ${coverage.days} 天）`;
}

/**
 * 本地观察是否完整覆盖近 7 天 / 本月。
 *
 * 旧后端不下发 `coverage` 时回退成「有快照即覆盖」：保持改造前的显示行为，不让整页
 * 变成「—」。新后端一定下发该字段，因此退化路径只服务历史响应。
 */
export function localWindowCoverage(
  coverage: CreditLocalCoverage | undefined,
  hasCoverage: boolean,
): { week: boolean; month: boolean } {
  if (!coverage) return { week: hasCoverage, month: hasCoverage };
  return { week: coverage.covers7Days, month: coverage.coversThisMonth };
}

/** 当前数字的来源标记：官方可用（complete/partial）用官方，否则用本地观察。 */
export function creditSourceBadge(official?: CreditOfficialUsage): CreditSourceBadge {
  const usable = official?.status === "complete" || official?.status === "partial";
  return usable ? "官方用量" : "本地观察";
}

/**
 * 概览卡某个窗口应显示的值。
 *
 * - 官方可用：直接用官方汇总（官方没有对应字段时由 `formatAmount` 退化成「—」）；
 * - 官方不可用：今日仍显示本地观察值（快照差值可信），但 7 天/本月只有在本地快照
 *   **完整覆盖**该窗口时才显示数字，否则返回「—」——绝不用今日值冒充。
 *
 * `absenceReason` 给出「—」的原因，供页面补 tooltip 文案。
 */
export function usageWindowValue(
  window: UsageWindowKey,
  official: CreditOfficialUsage | undefined,
  stats: CreditStatistics,
  coverage: CreditLocalCoverage | undefined,
): { text: string; absenceReason: string | null } {
  if (official) {
    const summary = official.summary;
    const value =
      window === "today"
        ? summary.usageToday
        : window === "7d"
          ? summary.usage7Days
          : summary.usageThisMonth;
    // 官方口径下总览永远是「已同步账号之和」（partial 由横幅说明），单账号缺失在
    // 账号明细里显示「—」，不在这里改写总额。
    return { text: formatAmount(value), absenceReason: null };
  }

  if (window === "today") {
    return { text: formatAmount(stats.summary.usageToday), absenceReason: null };
  }
  const covered = localWindowCoverage(coverage, stats.coverageStartAt !== null);
  const isWeek = window === "7d";
  const complete = isWeek ? covered.week : covered.month;
  if (!complete) {
    return {
      text: "—",
      absenceReason: coverage
        ? `本地快照仅覆盖 ${coverage.days} 天，${isWeek ? "近 7 天" : "本月"}未完整覆盖`
        : "本地快照未覆盖该窗口",
    };
  }
  return {
    text: formatAmount(isWeek ? stats.summary.usage7Days : stats.summary.usageThisMonth),
    absenceReason: null,
  };
}

/** 当前统计范围的消耗值：数字 + 文案 + 「—」的原因。 */
export interface RangeConsumption {
  /** 可参与后续计算（日均等）的数值；显示为「—」时为 null。 */
  value: number | null;
  text: string;
  absenceReason: string | null;
}

/**
 * 四卡里的「当前范围消耗」。
 *
 * 官方可用时直接按范围求和（与趋势图底部「当前口径合计」同一个 `rangeUsage`）；
 * 官方不可用时沿用 issue #225 的判定：今日快照差值可信，近 7 天/本月要求本地覆盖
 * 完整才给数字，30 天/90 天/全部按本地快照求和并由调用方标注覆盖范围。
 */
export function statsRangeConsumption(
  range: StatsRangeKey,
  official: CreditOfficialUsage | undefined,
  stats: CreditStatistics,
  coverage: CreditLocalCoverage | undefined,
): RangeConsumption {
  if (official) {
    const value = rangeUsage(official.summary, official.daily, range);
    return { value, text: formatAmount(value), absenceReason: null };
  }

  if (range === "today") {
    const value = stats.summary.usageToday;
    return { value, text: formatAmount(value), absenceReason: null };
  }

  if (range === "7d" || range === "month") {
    const window = usageWindowValue(
      range === "7d" ? "7d" : "month",
      official,
      stats,
      coverage,
    );
    const complete = !window.absenceReason;
    return {
      value: complete
        ? range === "7d"
          ? stats.summary.usage7Days
          : stats.summary.usageThisMonth
        : null,
      text: window.text,
      absenceReason: window.absenceReason,
    };
  }

  // 30 天 / 90 天 / 全部：本地快照求和就是趋势图的口径，数字同源。
  const points = (stats.daily ?? []).filter((point) => statsDateInRange(point.date, range));
  const value = points.reduce((sum, point) => sum + point.usage, 0);
  // 覆盖不足时把话说清楚（数字仍然显示，但只是下限）；覆盖完整就不加噪音。
  const uncovered =
    points.length === 0 || (coverage?.days ?? 0) < statsRangeDayCount(range, points.length);
  return {
    value,
    text: formatAmount(value),
    absenceReason: uncovered
      ? coverage
        ? `本地快照仅覆盖 ${coverage.days} 天，更早的日期没有本地记录（数字为下限）`
        : "本地暂无快照，数字为下限"
      : null,
  };
}
