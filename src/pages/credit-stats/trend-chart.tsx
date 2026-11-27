import { useState, type ComponentProps } from "react";
import { Bar, BarChart, CartesianGrid, Rectangle, XAxis, YAxis } from "recharts";
import type { LucideIcon } from "lucide-react";
import { Card, CardContent, CardDescription, CardHeader } from "@/components/ui/card";
import { ChartContainer, ChartTooltip, ChartTooltipContent, type ChartConfig } from "@/components/ui/chart";
import { formatAmount, formatDate, formatDateTime } from "@/lib/format";
import { getStackedSegmentVisualLayout } from "@/lib/stacked-bar-visuals";
import type { StatsRangeKey } from "@/lib/stats-range";
import type {
  CreditOfficialUsage,
  CreditStatsDailyPoint,
  CreditStatistics,
} from "@/lib/types";

import { AccountFilterMenu } from "./account-filter-menu";
import { chartPoints, formatChartDate, isOfficialUsageAvailable, officialAccountFor, rangeUsage } from "./data";

export function StatMetric({
  icon: Icon,
  label,
  value,
  divided = false,
}: {
  icon: LucideIcon;
  label: string;
  value: string;
  divided?: boolean;
}) {
  return (
    <div
      className={`flex min-w-0 flex-col items-center justify-center px-4 py-5 text-center sm:py-3 ${
        divided ? "sm:border-l sm:border-border/60" : ""
      }`}
    >
      <div className="flex max-w-full items-center justify-center gap-2 text-[13px] font-medium leading-5 text-muted-foreground">
        <Icon className="size-4 shrink-0 stroke-[1.75]" aria-hidden="true" />
        <span className="truncate">{label}</span>
      </div>
      <div className="mt-3 max-w-full truncate text-[26px] font-semibold leading-8 tracking-[-0.025em] text-foreground tabular-nums" style={{ fontFamily: '"Bricolage Grotesque Variable", "SF Pro Display", ui-sans-serif, sans-serif' }}>
        {value}
      </div>
    </div>
  );
}

/**
 * 模型趋势共享数据色板；颜色由浅色/深色主题 token 提供。
 *
 * 顺序即分配顺序：前几位给最常见的模型，所以把品牌蓝放第一位、青色放第二位。
 * 全部是冷色（青 → 蓝 → 靛 → 紫 + 两个中性/低饱和强调），与蓝色主题同色温；
 * 暖色（琥珀/橙/黄绿）已从 token 里移除，别再加回来。
 */
const MODEL_COLORS = [
  "var(--data-series-blue)",
  "var(--data-series-cyan)",
  "var(--data-series-violet)",
  "var(--data-series-slate)",
  "var(--data-series-rose)",
  "var(--data-series-indigo)",
  "var(--data-series-sky)",
  "var(--data-series-teal)",
];

interface ModelChartPoint {
  date: string;
  total: number;
  [model: string]: number | string;
}

type CreditBarShapeProps = ComponentProps<typeof Rectangle> & {
  segmentKey: string;
  seriesKeys: string[];
  payload?: ModelChartPoint;
  value?: number | [number, number];
};

function CreditBarShape({
  segmentKey,
  seriesKeys,
  payload,
  x = 0,
  y = 0,
  width = 0,
  height = 0,
  value,
  fill,
  stroke,
  strokeWidth,
  ...rest
}: CreditBarShapeProps) {
  if (width <= 0 || height <= 0) return null;
  const segmentIndex = seriesKeys.indexOf(segmentKey);
  const stackStart = Array.isArray(value) ? Number(value[0]) : 0;
  const layout = payload
    ? getStackedSegmentVisualLayout({
        values: seriesKeys.map((key) => Number(payload[key] ?? 0)),
        segmentIndex,
        segmentHeight: height,
        segmentY: y,
        stackStart,
      })
    : null;
  return (
    <Rectangle
      {...rest}
      x={x}
      y={layout?.y ?? y}
      width={width}
      height={layout?.height ?? height}
      fill={fill}
      radius={layout?.isTop ? [6, 6, 0, 0] : 0}
      stroke={stroke ?? "var(--background)"}
      strokeWidth={strokeWidth ?? 2}
    />
  );
}

/** 从官方 daily（全量按模型聚合）构建层叠数据；模型按总消耗降序全部保留。 */
function buildStackedChart(
  daily: CreditStatsDailyPoint[],
): { models: string[]; points: ModelChartPoint[] } {
  const modelTotals = new Map<string, number>();
  for (const point of daily) {
    for (const model of point.models ?? []) {
      modelTotals.set(model.model, (modelTotals.get(model.model) ?? 0) + model.credit);
    }
  }
  const models = [...modelTotals.entries()]
    .sort((left, right) => right[1] - left[1])
    .map(([model]) => model);

  const points: ModelChartPoint[] = daily.map((point) => {
    const entry: ModelChartPoint = { date: point.date, total: point.usage };
    for (const model of point.models ?? []) {
      const key = model.model;
      entry[key] = (typeof entry[key] === "number" ? entry[key] : 0) + model.credit;
    }
    return entry;
  });
  return { models, points };
}

export function TrendChart({
  stats,
  officialUsage,
  range,
}: {
  stats: CreditStatistics;
  officialUsage?: CreditOfficialUsage;
  /** 全局统计范围：本卡片不再单独持有范围状态。 */
  range: StatsRangeKey;
}) {
  /** null = 所有账号汇总；本卡片独立，不影响其他卡片 */
  const [accountFilter, setAccountFilter] = useState<string | null>(null);
  const official = isOfficialUsageAvailable(officialUsage) ? officialUsage : undefined;
  const officialAvailable = Boolean(official);
  const filterAccounts = official ? official.accounts : stats.accounts;
  const activeFilterAccount =
    accountFilter && filterAccounts.some((account) => account.accountId === accountFilter)
      ? filterAccounts.find((account) => account.accountId === accountFilter)
      : undefined;
  const effectiveFilter = activeFilterAccount?.accountId ?? null;
  const officialAccount = effectiveFilter ? officialAccountFor(official, effectiveFilter) : undefined;
  const localAccount = effectiveFilter
    ? stats.accounts.find((account) => account.accountId === effectiveFilter)
    : undefined;
  // 选中账号时切到该账号的逐日数据与汇总；否则用全部账号的聚合
  const daily = official
    ? (officialAccount ? officialAccount.daily ?? [] : official.daily)
    : (localAccount ? localAccount.daily ?? [] : stats.daily);
  const summary = official
    ? (officialAccount
        ? {
            usageToday: officialAccount.usageToday ?? 0,
            usage7Days: officialAccount.usage7Days ?? 0,
            usageThisMonth: officialAccount.usageThisMonth ?? 0,
          }
        : official.summary)
    : (localAccount
        ? {
            usageToday: localAccount.usageToday,
            usage7Days: localAccount.usage7Days,
            usageThisMonth: localAccount.usageThisMonth,
          }
        : stats.summary);
  const basePoints = chartPoints(daily, range);
  const hasDataSource = officialAvailable || Boolean(stats.coverageStartAt);

  // 官方 daily 带全量模型聚合 → 层叠柱（按模型）；否则单层「本地观察」柱
  const hasModelDetail = (official?.daily ?? []).some((point) => (point.models?.length ?? 0) > 0);
  const stacked = official && hasModelDetail ? buildStackedChart(basePoints) : null;
  const chartData: ModelChartPoint[] = stacked
    ? stacked.points
    : basePoints.map((point) => ({ date: point.date, total: point.usage }));

  // 单层本地数据实际使用 `total` 字段；官方数据才按模型名称分层。
  const series = stacked ? stacked.models : ["total"];
  const chartConfig: ChartConfig = {};
  for (const model of series) {
    chartConfig[model] = {
      label: model === "total" ? "总消耗" : model,
      ...(stacked
        ? { color: MODEL_COLORS[series.indexOf(model) % MODEL_COLORS.length] }
        : { color: "var(--data-series-blue)" }),
    };
  }

  const hasObservedUsage = chartData.some((point) => point.total > 0);
  return (
    <section className="min-w-0 space-y-2.5" aria-labelledby="trend-chart-title">
      <div className="px-1">
        <h2 id="trend-chart-title" className="text-[13px] font-medium leading-5">
          {officialAvailable ? "官方积分消耗" : "本地观察积分消耗"}
        </h2>
      </div>
      <Card className="min-w-0 gap-0 overflow-hidden rounded-xl py-0 shadow-none">
        <CardHeader className="gap-0 px-4 pt-3 pb-0 sm:px-5">
          <div className="flex min-w-0 flex-wrap items-center justify-between gap-3">
            <CardDescription className="min-w-0 text-xs">
              {officialAvailable
                ? `来自 WorkBuddy 官方请求用量 · ${official?.rangeStart} 至 ${official?.rangeEnd}`
                : "只统计连续快照中余额下降的正差值；官方用量暂不可用时保留此口径。"}
            </CardDescription>
            <div className="flex max-w-full flex-wrap items-center gap-1.5">
              <AccountFilterMenu
                accounts={filterAccounts}
                accountFilter={effectiveFilter}
                onAccountFilterChange={setAccountFilter}
                ariaLabel="按账号筛选趋势"
              />
            </div>
          </div>
        </CardHeader>
        <CardContent className="min-w-0 px-4 pt-3 pb-4 sm:px-5">
        {!hasDataSource ? (
          <div className="rounded-lg border border-dashed px-4 py-10 text-center text-sm text-muted-foreground">
            尚无积分快照。首次成功采集后，统计会从该时刻开始累计。
          </div>
        ) : chartData.length === 0 ? (
          <div className="rounded-lg border border-dashed px-4 py-10 text-center text-sm text-muted-foreground">
            当前口径暂无可展示的观察数据。
          </div>
        ) : (
          <>
            <ChartContainer config={chartConfig} className="h-56 w-full">
              <BarChart data={chartData} margin={{ top: 8, right: 8, left: 0, bottom: 0 }}>
                <CartesianGrid vertical={false} strokeDasharray="3 3" />
                <XAxis
                  dataKey="date"
                  tickLine={false}
                  axisLine={false}
                  tickMargin={8}
                  tickFormatter={(value) => formatChartDate(String(value))}
                />
                <YAxis tickLine={false} axisLine={false} width={42} tickFormatter={(value) => formatAmount(value)} />
                <ChartTooltip
                  cursor={{ fill: "var(--muted)", opacity: 0.4 }}
                  content={
                    <ChartTooltipContent
                      labelFormatter={(_, payload) => {
                        const item = Array.isArray(payload) ? payload[0] : payload;
                        return `${formatChartDate(String(item?.payload?.date ?? ""))} 消耗`;
                      }}
                    />
                  }
                />
                {series.map((model, index) => (
                  <Bar
                    key={model}
                    dataKey={model}
                    stackId="usage"
                    fill={stacked ? MODEL_COLORS[index % MODEL_COLORS.length] : "var(--color-total)"}
                    stroke="var(--background)"
                    strokeWidth={2}
                    maxBarSize={28}
                    shape={<CreditBarShape segmentKey={model} seriesKeys={series} />}
                    isAnimationActive={false}
                  />
                ))}
              </BarChart>
            </ChartContainer>
            {stacked && (
              <div className="mt-3 flex flex-wrap items-center justify-center gap-x-4 gap-y-1.5 text-xs text-muted-foreground">
                {stacked.models.map((model, index) => (
                  <span key={model} className="inline-flex items-center gap-1.5">
                    <span className="h-2 w-2 shrink-0 rounded-[2px]" style={{ backgroundColor: MODEL_COLORS[index % MODEL_COLORS.length] }} aria-hidden="true" />
                    {model}
                  </span>
                ))}
              </div>
            )}
            <div className="mt-3 flex flex-wrap items-center justify-between gap-2 text-xs text-muted-foreground">
              <span>
                {hasObservedUsage
                  ? `当前口径合计 ${formatAmount(rangeUsage(summary, daily, range))} 积分`
                  : officialAvailable
                    ? "官方已返回明细，当前范围暂无积分消耗"
                    : "已采集快照，暂未观察到余额下降"}
              </span>
              <span>{officialAvailable ? `数据更新于 ${formatDateTime(official?.collectedAt ?? stats.generatedAt)}` : `数据覆盖至 ${formatDate(stats.generatedAt)}`}</span>
            </div>
            <p className="sr-only">
              {chartData.map((point) => `${point.date} 消耗 ${formatAmount(point.total)} 积分`).join("；")}
            </p>
          </>
        )}
        </CardContent>
      </Card>
    </section>
  );
}
