import { readPreference, writePreference } from "@/lib/persisted-preference";
import { dateDaysAgo, dateKey, pad2 } from "@/lib/format";
import type {
  TokenStatsGroup,
  TokenStatsSource,
  TokenStatsTotals,
} from "@/lib/types";

export type SourceKey = TokenStatsSource["source"];

export type RangeKey = "30d" | "today" | "7d" | "month";

export type OverviewRangeKey = "today" | "7d" | "30d" | "total";

export type DistributionKey = "projects" | "models";

const TOKEN_SOURCE_STORAGE_KEY = "workbuddy-shift:token-stats:source";

export const RANKING_LIMIT = 10;

export const REQUEST_PAGE_SIZE = 50;

export function isSourceKey(value: unknown): value is SourceKey {
  return (
    value === "workbuddy" ||
    value === "workbuddy-ai" ||
    value === "codebuddy-cli" ||
    value === "codebuddy-ide"
  );
}

const TOKEN_SOURCE_PREFERENCE = {
  key: TOKEN_SOURCE_STORAGE_KEY,
  fallback: "workbuddy" as SourceKey,
  parse: (raw: string): SourceKey | null => (isSourceKey(raw) ? raw : null),
  serialize: (source: SourceKey) => source,
  eventName: "workbuddy-shift:token-stats-source-changed",
} as const;

export function readPreferredTokenSource(): SourceKey {
  return readPreference(TOKEN_SOURCE_PREFERENCE);
}

export function persistPreferredTokenSource(source: SourceKey): void {
  writePreference(TOKEN_SOURCE_PREFERENCE, source);
}

export const RANGE_OPTIONS: { key: RangeKey; label: string }[] = [
  { key: "30d", label: "近 30 天" },
  { key: "today", label: "今天" },
  { key: "7d", label: "近 7 天" },
  { key: "month", label: "本月" },
];

export const OVERVIEW_RANGE_OPTIONS: { key: OverviewRangeKey; label: string }[] = [
  { key: "today", label: "今日" },
  { key: "7d", label: "近 7 天" },
  { key: "30d", label: "近 30 天" },
  { key: "total", label: "总计" },
];

const compactTokenFormatter = new Intl.NumberFormat("en-US", {
  notation: "compact",
  compactDisplay: "short",
  maximumFractionDigits: 1,
});

export const exact = new Intl.NumberFormat("zh-CN");

const exactTokenFormatter = new Intl.NumberFormat("en-US");

export function formatTokenCompact(value: number): string {
  return compactTokenFormatter
    .format(value)
    .replace(/([kmb])$/i, (unit) => unit.toUpperCase());
}

export function formatTokenExact(value: number): string {
  return exactTokenFormatter.format(value);
}

/** 展示总量：input 已包含 cacheRead，因此不能再次加上 cacheRead。 */
export const tokenTotal = (value: TokenStatsTotals) =>
  value.input + value.output + value.cacheWrite;

export const percentage = (value: number, sum: number) =>
  sum > 0 ? `${((value / sum) * 100).toFixed(1)}%` : "—";

export function formatChartDate(date: string): string {
  return date.slice(5).replace("-", "/");
}

/** 请求明细的精确时间：本地时区 `YYYY-MM-DD HH:mm:ss`。 */
export function formatRequestTime(timestamp: number): string {
  const date = new Date(timestamp);
  return `${date.getFullYear()}-${pad2(date.getMonth() + 1)}-${pad2(date.getDate())} ${pad2(date.getHours())}:${pad2(date.getMinutes())}:${pad2(date.getSeconds())}`;
}

export function formatHeatmapDate(date: Date): string {
  return date.toLocaleDateString("zh-CN", {
    month: "long",
    day: "numeric",
  });
}

export function rangeLabel(range: RangeKey): string {
  return RANGE_OPTIONS.find((option) => option.key === range)?.label ?? "近 30 天";
}

export function rangePoints(daily: TokenStatsGroup[], range: RangeKey): TokenStatsGroup[] {
  const today = dateKey(new Date());
  const firstDate =
    range === "today" ? today : range === "7d" ? dateDaysAgo(6) : dateDaysAgo(29);

  return daily
    .filter((point) => {
      if (range === "month") {
        return point.key.startsWith(`${today.slice(0, 7)}-`);
      }
      return point.key >= firstDate && point.key <= today;
    })
    .sort((left, right) => left.key.localeCompare(right.key));
}

function rangeBounds(range: RangeKey): { start: string; end: string } {
  const today = dateKey(new Date());
  if (range === "today") return { start: today, end: today };
  if (range === "7d") return { start: dateDaysAgo(6), end: today };
  if (range === "30d") return { start: dateDaysAgo(29), end: today };
  const monthStart = new Date();
  monthStart.setHours(12, 0, 0, 0);
  monthStart.setDate(1);
  return { start: dateKey(monthStart), end: today };
}

export function fillRangePoints(points: TokenStatsGroup[], range: RangeKey): TokenStatsGroup[] {
  if (points.length === 0) return [];
  const { start, end } = rangeBounds(range);
  const byDate = new Map(points.map((point) => [point.key, point]));
  const cursor = new Date(`${start}T12:00:00`);
  const endDate = new Date(`${end}T12:00:00`);
  const filled: TokenStatsGroup[] = [];
  while (cursor <= endDate) {
    const key = dateKey(cursor);
    const point = byDate.get(key);
    filled.push(
      point ?? {
        key,
        total: 0,
        input: 0,
        output: 0,
        cacheRead: 0,
        cacheWrite: 0,
        uncachedInput: 0,
        records: 0,
        cacheHitRate: null,
      },
    );
    cursor.setDate(cursor.getDate() + 1);
  }
  return filled;
}

export function rangeTotals(points: TokenStatsGroup[]): TokenStatsTotals {
  const totals = points.reduce(
    (sum, point) => ({
      total: sum.total + tokenTotal(point),
      input: sum.input + point.input,
      output: sum.output + point.output,
      cacheRead: sum.cacheRead + point.cacheRead,
      cacheWrite: sum.cacheWrite + point.cacheWrite,
      uncachedInput: sum.uncachedInput + point.uncachedInput,
      records: sum.records + point.records,
      cacheHitRate: null,
    }),
    {
      total: 0,
      input: 0,
      output: 0,
      cacheRead: 0,
      cacheWrite: 0,
      uncachedInput: 0,
      records: 0,
      cacheHitRate: null,
    } satisfies TokenStatsTotals,
  );

  return {
    ...totals,
    cacheHitRate: totals.input > 0 ? totals.cacheRead / totals.input : null,
  };
}

export function overviewTotals(source: TokenStatsSource, range: OverviewRangeKey): TokenStatsTotals {
  if (range === "total") return source.summary;
  return rangeTotals(rangePoints(source.daily, range));
}