/**
 * 统计页（积分 / Token）统一的**一个**时间范围。
 *
 * 改造前两页各自维护多份范围状态：页面顶部是「30 天 / 90 天 / 全部」的取数窗口，
 * 卡片内部还有 5 份外观完全相同的「今天 / 7 天 / 30 天 / 本月」按钮，用户无法判断
 * 哪个才是全局口径，默认值也互不一致。这里把范围收敛成：
 *
 * - **唯一定义**：`StatsRangeKey` + `statsRangeOptions()`（选项表只在这里维护）；
 * - **唯一切片口径**：`statsDateInRange()`，两页的逐日过滤都调它，不再各写一遍；
 * - **取数窗口映射**：`creditDataRange()`，不新增后端 `range` 取值（契约不变），
 *   只把展示区间映射到官方已有窗口。
 *
 * 「近 90 天」不是固定项：官方窗口探测不支持时（积分页 `windowProbe.maxDays < 90`）
 * 或本地数据跨度不足 90 天（Token 页）就不渲染该按钮，避免给出一个必然降级的选项。
 */
import { dateDaysAgo, dateKey } from "@/lib/format";
import type { PersistedPreferenceOptions } from "@/lib/persisted-preference";
import type { CreditRange } from "@/lib/types";

/** 全局统计范围。`all` 与后端 `range=\"all\"`（本地账本）语义一致。 */
export type StatsRangeKey = "today" | "7d" | "30d" | "month" | "90d" | "all";

export interface StatsRangeOption {
  key: StatsRangeKey;
  label: string;
  /** 悬停说明（口径来源），没有则为空。 */
  hint?: string;
}

/** 默认范围：与改造前两页的取数默认值（30 天）保持一致。 */
export const DEFAULT_STATS_RANGE: StatsRangeKey = "30d";

/** 固定 5 项：今天 / 近 7 天 / 近 30 天 / 本月 / 全部。 */
const BASE_OPTIONS: readonly StatsRangeOption[] = [
  { key: "today", label: "今天" },
  { key: "7d", label: "近 7 天" },
  { key: "30d", label: "近 30 天" },
  { key: "month", label: "本月" },
  { key: "all", label: "全部" },
];

/** 第 6 项：只在数据支持时插入到「全部」之前。 */
const NINETY_DAYS_OPTION: StatsRangeOption = { key: "90d", label: "近 90 天" };

/**
 * 当前数据下可用的范围选项。
 *
 * @param include90d 是否支持 90 天（积分页看窗口探测结果，Token 页看本地数据跨度）。
 */
export function statsRangeOptions(include90d: boolean): StatsRangeOption[] {
  if (!include90d) return [...BASE_OPTIONS];
  const all = BASE_OPTIONS[BASE_OPTIONS.length - 1];
  return [...BASE_OPTIONS.slice(0, -1), NINETY_DAYS_OPTION, all];
}

/** 两页共用的范围偏好：localStorage 键名 + 同页事件名 + 默认值。 */
export const STATS_RANGE_PREFERENCE: PersistedPreferenceOptions<StatsRangeKey> = {
  key: "workbuddy-shift.stats.range",
  fallback: DEFAULT_STATS_RANGE,
  // 只接受选项表里出现过的键（含 90d），损坏值返回 null 由 hook 回退默认
  parse: (raw) => statsRangeOptions(true).find((option) => option.key === raw)?.key ?? null,
  serialize: (value) => value,
  eventName: "workbuddy-shift:stats-range-changed",
};

/** 范围标签（导出文件名 / 提示 / 折叠标题共用，避免各处手写字面量）。 */
export function statsRangeLabel(range: StatsRangeKey): string {
  return (
    statsRangeOptions(true).find((option) => option.key === range)?.label ?? "近 30 天"
  );
}

/**
 * 范围覆盖的自然日天数（用于「日均消耗」这类派生指标）。
 *
 * `allDays` 只在 `all` 时使用：它应该传入当前口径逐日序列的长度（数据实际跨度），
 * 而不是一个猜出来的常数。
 */
export function statsRangeDayCount(range: StatsRangeKey, allDays: number): number {
  switch (range) {
    case "today":
      return 1;
    case "7d":
      return 7;
    case "30d":
      return 30;
    case "90d":
      return 90;
    case "month":
      // 本月只统计已经过去的日子，否则月初的「日均」会被整月天数稀释。
      return Math.max(1, new Date().getDate());
    case "all":
      return Math.max(1, allDays);
  }
}

/** 某个 `YYYY-MM-DD` 是否落在范围内（`month` 按自然月，`all` 全保留）。 */
export function statsDateInRange(
  date: string,
  range: StatsRangeKey,
  today: string = dateKey(new Date()),
): boolean {
  switch (range) {
    case "all":
      return true;
    case "month":
      return date.startsWith(`${today.slice(0, 7)}-`);
    case "today":
      return date === today;
    case "7d":
      return date >= dateDaysAgo(6) && date <= today;
    case "30d":
      return date >= dateDaysAgo(29) && date <= today;
    case "90d":
      return date >= dateDaysAgo(89) && date <= today;
  }
}

/** 官方窗口探测是否支持 90 天（`null` = 尚未探测，按不支持处理）。 */
export function statsRangeSupports90d(maxDays: number | null | undefined): boolean {
  return typeof maxDays === "number" && maxDays >= 90;
}

/**
 * 展示区间 → 取数窗口（后端已有取值，不新增契约）。
 *
 * - 今天 / 近 7 天 / 近 30 天 → 取 30 天窗口，只改展示切片；
 * - 本月 → 支持 90 天时取 90 天窗口后按自然月切片，否则退化成 30 天切片；
 * - 近 90 天 → 90 天窗口；全部 → 本地账本（`range=\"all\"` 的既有路径）。
 */
export function creditDataRange(range: StatsRangeKey, supports90d: boolean): CreditRange {
  if (range === "all") return "all";
  if (range === "90d") return "90d";
  if (range === "month") return supports90d ? "90d" : "30d";
  return "30d";
}

/** 「本月」退化成 30 天切片时的提示文案；不需要提示时返回 null。 */
export function monthSliceNote(range: StatsRangeKey, supports90d: boolean): string | null {
  return range === "month" && !supports90d ? "本月按近 30 天数据统计" : null;
}

/**
 * 两个 `YYYY-MM-DD` 之间的天数（含首尾）。
 *
 * Token 页用它判断本地数据跨度是否够 90 天：不够就不渲染第 6 项按钮。
 * 非法输入返回 0（调用方按「不支持」处理）。
 */
export function statsDateSpanDays(firstDate?: string, lastDate?: string): number {
  if (!firstDate || !lastDate) return 0;
  const first = Date.parse(`${firstDate}T12:00:00`);
  const last = Date.parse(`${lastDate}T12:00:00`);
  if (!Number.isFinite(first) || !Number.isFinite(last)) return 0;
  return Math.round((last - first) / 86_400_000) + 1;
}
