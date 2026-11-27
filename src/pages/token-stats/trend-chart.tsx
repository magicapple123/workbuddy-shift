import { useEffect, useMemo, useState } from "react";
import type {
  ComponentProps,
} from "react";
import { Check, SlidersHorizontal } from "lucide-react";
import { Bar, CartesianGrid, ComposedChart, Line, Rectangle, XAxis, YAxis } from "recharts";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader } from "@/components/ui/card";
import { ChartContainer, ChartTooltip, type ChartConfig } from "@/components/ui/chart";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { formatDateTime } from "@/lib/format";
import { getStackedSegmentVisualLayout } from "@/lib/stacked-bar-visuals";
import type { StatsRangeKey } from "@/lib/stats-range";
import type {
  TokenStatsGroup,
  TokenStatsSource,
} from "@/lib/types";

import { exact, fillRangePoints, formatChartDate, formatTokenCompact, formatTokenExact, percentage, rangeLabel, rangePoints, rangeTotals, tokenTotal } from "./data";
import { SectionTitle } from "./metric";

const chartConfig = {
  cacheRead: { label: "缓存读取", color: "var(--data-series-blue)" },
  uncachedInput: { label: "新增输入", color: "var(--data-series-cyan)" },
  output: { label: "输出", color: "var(--data-series-violet)" },
  cacheWrite: { label: "缓存写入", color: "var(--data-series-slate)" },
  records: { label: "调用次数", color: "var(--data-series-indigo)" },
} satisfies ChartConfig;

type TrendChartPoint = TokenStatsGroup & { date: string };

type TokenSeriesKey = "cacheRead" | "uncachedInput" | "output" | "cacheWrite";

type TokenBarShapeProps = ComponentProps<typeof Rectangle> & {
  segmentKey: TokenSeriesKey;
  payload?: TrendChartPoint;
  value?: number | [number, number];
};

/**
 * 图表系列（顺序即堆叠顺序）。**这份列表必须与导出列一一对应**：
 * 任何一方增删系列，都要同时改 `./export.ts` 的 `TOTALS_HEADERS` / `totalsCells`
 * 以及 `./metric.tsx` 的口径说明（见 docs/FAQ.md「为什么缓存写入是 0」）。
 *
 * `cacheWrite` 在 WorkBuddy（DeepSeek 系）数据里恒为 0：上游 `usage` 对象根本没有
 * 写入字段，只在 `providerData.rawUsage` 里为 schema 兼容保留、从不填值（R5-18）。
 * 恒 0 属**预期**，不是统计缺陷；因此这里保持显示该系列与 0 值，不做自动隐藏、
 * 不加 tooltip 说明——若将来接入会上报写入的模型（Anthropic 系），该系列会自动
 * 出现非 0 值。
 */
const TOKEN_SERIES: TokenSeriesKey[] = [
  "cacheRead",
  "uncachedInput",
  "output",
  "cacheWrite",
];

/**
 * 使用真实 Rectangle 形状绘制每个堆叠段：整柱约束分配小段的 5px 视觉保底，
 * 但不修改 data 值；每个日期实际顶部的非零段才使用顶部圆角。
 */
function TokenBarShape({
  segmentKey,
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
}: TokenBarShapeProps) {
  if (width <= 0 || height <= 0) return null;
  const segmentIndex = TOKEN_SERIES.indexOf(segmentKey);
  const stackStart = Array.isArray(value) ? Number(value[0]) : 0;
  const layout = payload
    ? getStackedSegmentVisualLayout({
        values: TOKEN_SERIES.map((key) => payload[key]),
        segmentIndex,
        segmentHeight: height,
        segmentY: y,
        stackStart,
      })
    : null;
  if (!layout) {
    return (
      <Rectangle
        {...rest}
        x={x}
        y={y}
        width={width}
        height={height}
        fill={fill}
        radius={0}
        stroke={stroke ?? "var(--background)"}
        strokeWidth={strokeWidth ?? 1}
      />
    );
  }

  return (
    <Rectangle
      {...rest}
      x={x}
      y={layout.y}
      width={width}
      height={layout.height}
      fill={fill}
      radius={layout.isTop ? [6, 6, 0, 0] : 0}
      stroke={stroke ?? "var(--background)"}
      strokeWidth={strokeWidth ?? 1}
    />
  );
}

function TrendLegend() {
  const items = [
    { key: "cacheRead", label: "缓存读取", color: "var(--data-series-blue)", kind: "area" },
    { key: "uncachedInput", label: "新增输入", color: "var(--data-series-cyan)", kind: "area" },
    { key: "output", label: "输出", color: "var(--data-series-violet)", kind: "area" },
    { key: "cacheWrite", label: "缓存写入", color: "var(--data-series-slate)", kind: "area" },
    { key: "records", label: "调用次数", color: "var(--data-series-indigo)", kind: "line" },
  ];

  return (
    <div
      className="flex min-w-0 flex-wrap items-center gap-x-4 gap-y-1.5 text-xs text-muted-foreground"
      aria-label="图表图例"
    >
      {items.map((item) => (
        <span key={item.key} className="inline-flex items-center gap-1.5 whitespace-nowrap">
          {item.kind === "line" ? (
            <span
              className="relative inline-flex h-2 w-4 shrink-0 items-center"
              aria-hidden="true"
            >
              <span
                className="absolute inset-x-0 top-1/2 border-t-2 border-dashed"
                style={{ borderColor: item.color }}
              />
              <span
                className="relative z-10 mx-auto size-1.5 rounded-full border border-background"
                style={{ backgroundColor: item.color }}
              />
            </span>
          ) : (
            <span
              className="size-2.5 shrink-0 rounded-[3px]"
              style={{ backgroundColor: item.color }}
              aria-hidden="true"
            />
          )}
          {item.label}
        </span>
      ))}
    </div>
  );
}

function TrendTooltipContent({
  active,
  payload,
}: {
  active?: boolean;
  payload?: Array<{ payload?: TrendChartPoint }>;
}) {
  if (!active || !payload?.length) return null;
  const point = payload[0]?.payload;
  if (!point) return null;
  const rows = [
    { key: "cacheRead", label: "缓存读取", value: point.cacheRead, color: "var(--data-series-blue)" },
    {
      key: "uncachedInput",
      label: "新增输入",
      value: point.uncachedInput,
      color: "var(--data-series-cyan)",
    },
    { key: "output", label: "输出", value: point.output, color: "var(--data-series-violet)" },
    { key: "cacheWrite", label: "缓存写入", value: point.cacheWrite, color: "var(--data-series-slate)" },
    { key: "records", label: "调用次数", value: point.records, color: "var(--data-series-indigo)" },
  ];
  const total = tokenTotal(point);

  return (
    <div className="grid min-w-[13rem] gap-2 rounded-lg border border-border/50 bg-background px-3 py-2.5 text-xs shadow-xl">
      <div className="font-medium text-foreground">{formatChartDate(point.date)}</div>
      <div className="flex items-center justify-between border-b border-border/60 pb-1.5">
        <span className="text-muted-foreground">Token 总量</span>
        <span
          className="whitespace-nowrap font-mono font-semibold tabular-nums text-foreground"
          title={`${formatTokenExact(total)} Token`}
          aria-label={`${formatTokenExact(total)} Token`}
        >
          {formatTokenCompact(total)} Token
        </span>
      </div>
      <div className="grid gap-1.5">
        {rows.map((row) => (
          <div key={row.key} className="flex items-center gap-2">
            <span
              className={`shrink-0 ${row.key === "records" ? "h-0 w-3 border-t-2 border-dashed" : "size-2.5 rounded-[3px]"}`}
              style={
                row.key === "records"
                  ? { borderColor: row.color }
                  : { backgroundColor: row.color }
              }
              aria-hidden="true"
            />
            <span className="flex-1 text-muted-foreground">{row.label}</span>
            <span
              className="whitespace-nowrap font-mono font-medium tabular-nums text-foreground"
              title={row.key === "records" ? undefined : `${formatTokenExact(row.value)} Token`}
              aria-label={row.key === "records" ? undefined : `${formatTokenExact(row.value)} Token`}
            >
              {row.key === "records" ? exact.format(row.value) : formatTokenCompact(row.value)} {row.key === "records" ? "次" : "Token"}
              {row.key !== "records" ? (
                <span className="ml-1 font-sans text-[11px] font-normal text-muted-foreground">
                  ({percentage(row.value, total)})
                </span>
              ) : null}
            </span>
          </div>
        ))}
      </div>
    </div>
  );
}

/**
 * Token 与调用趋势。
 *
 * **范围受控**：`range` 由页面顶部唯一的「统计范围」控件传入（见 `@/lib/stats-range`）。
 * 本卡只保留「按模型筛选」这一个正交筛选——改造前这里另有一套「今天/7天/30天/本月」
 * 按钮，与总览卡的同名按钮默认值不同，切完两处数字对不上。
 */
export function TrendChart({ source, range }: { source: TokenStatsSource; range: StatsRangeKey }) {
  const [modelFilter, setModelFilter] = useState("all");
  const modelOptions = useMemo(
    () => (source.dailyByModel ? source.models.map((model) => model.key).filter(Boolean) : []),
    [source.dailyByModel, source.models],
  );
  useEffect(() => {
    if (modelFilter !== "all" && !modelOptions.includes(modelFilter)) setModelFilter("all");
  }, [modelFilter, modelOptions]);
  // 序列必须在 `useMemo` 内部算：写成外层的三元表达式时 `? … : []` 每次都产生新数组，
  // 依赖它的 `useMemo` 就每次渲染都重算（`react-hooks/exhaustive-deps` 的历史 warning）。
  // 计算逻辑与依赖项（modelFilter / range / 两份源数据）与改写前完全等价。
  const points = useMemo(() => {
    const dailySeries =
      modelFilter === "all" ? source.daily : source.dailyByModel?.[modelFilter] ?? [];
    return fillRangePoints(rangePoints(dailySeries, range), range);
  }, [modelFilter, range, source.daily, source.dailyByModel]);
  const totals = useMemo(() => rangeTotals(points), [points]);
  const chartData: TrendChartPoint[] = points.map((point) => ({
    ...point,
    date: point.key,
  }));

  return (
    <section className="min-w-0 space-y-2.5" aria-labelledby="token-trend-title">
      <SectionTitle id="token-trend-title">
        Token 与调用趋势
      </SectionTitle>
      <Card className="min-w-0 gap-0 overflow-hidden rounded-xl py-0 shadow-none">
        <CardHeader className="gap-0 px-4 pt-3 pb-0 sm:px-5">
          <div className="flex min-w-0 flex-wrap items-center justify-between gap-3">
            <CardDescription className="min-w-0 text-xs">
              彩色堆叠柱表示每日总 Token 及构成，虚线表示调用次数。
            </CardDescription>
            <div className="flex max-w-full flex-wrap items-center gap-2">
              <DropdownMenu>
                <DropdownMenuTrigger asChild>
                  <Button
                    variant="ghost"
                    size="sm"
                    className="h-8 max-w-[190px] gap-1.5 px-2.5 text-xs text-muted-foreground hover:text-foreground"
                    aria-label="按模型筛选"
                  >
                    <SlidersHorizontal className="size-3.5 shrink-0" />
                    <span className="truncate">{modelFilter === "all" ? "所有模型" : modelFilter}</span>
                  </Button>
                </DropdownMenuTrigger>
                <DropdownMenuContent align="end" className="max-h-80 w-56 overflow-y-auto">
                  <DropdownMenuItem onSelect={() => setModelFilter("all")}>
                    <SlidersHorizontal className="size-3.5 shrink-0" />
                    所有模型
                    {modelFilter === "all" && <Check className="ml-auto size-3.5 shrink-0" />}
                  </DropdownMenuItem>
                  {modelOptions.length > 0 && <DropdownMenuSeparator />}
                  {modelOptions.map((model) => (
                    <DropdownMenuItem key={model} onSelect={() => setModelFilter(model)}>
                      <span className="min-w-0 flex-1 truncate">{model}</span>
                      {modelFilter === model && <Check className="ml-auto size-3.5 shrink-0" />}
                    </DropdownMenuItem>
                  ))}
                </DropdownMenuContent>
              </DropdownMenu>
            </div>
          </div>
        </CardHeader>
        <CardContent className="min-w-0 px-4 pt-3 pb-4 sm:px-5">
          {chartData.length === 0 ? (
            <div className="rounded-lg border border-dashed px-4 py-10 text-center text-sm text-muted-foreground">
              当前范围暂无可展示的 Token 数据。
            </div>
          ) : (
            <>
              <div className="mb-2 flex flex-wrap items-center justify-between gap-2">
                <TrendLegend />
              </div>
              <div className="mb-1 flex items-center justify-end px-1 text-[11px] font-medium text-muted-foreground">
                <span className="font-normal">左轴：Token · 右轴：调用次数</span>
              </div>
              <ChartContainer config={chartConfig} className="h-64 w-full sm:h-72">
                <ComposedChart
                  data={chartData}
                  margin={{ top: 8, right: 8, left: 0, bottom: 0 }}
                  barCategoryGap="18%"
                >
                  <CartesianGrid vertical={false} strokeDasharray="3 3" />
                  <XAxis
                    dataKey="date"
                    tickLine={false}
                    axisLine={false}
                    tickMargin={8}
                    minTickGap={24}
                    tickFormatter={(value) => formatChartDate(String(value))}
                  />
                  <YAxis
                    yAxisId="tokens"
                    tickLine={false}
                    axisLine={false}
                    width={48}
                    tickFormatter={(value) => formatTokenCompact(Number(value))}
                  />
                  <YAxis
                    yAxisId="calls"
                    orientation="right"
                    tickLine={false}
                    axisLine={false}
                    width={46}
                    allowDecimals={false}
                    tickFormatter={(value) => exact.format(Number(value))}
                  />
                  <ChartTooltip
                    cursor={{ fill: "var(--muted)", opacity: 0.4 }}
                    content={<TrendTooltipContent />}
                  />
                  {TOKEN_SERIES.map((key) => (
                    <Bar
                      key={key}
                      yAxisId="tokens"
                      dataKey={key}
                      stackId="token"
                      fill={`var(--color-${key})`}
                      stroke="var(--background)"
                      strokeWidth={2}
                      maxBarSize={28}
                      shape={<TokenBarShape segmentKey={key} />}
                      isAnimationActive={false}
                    >
                    </Bar>
                  ))}
                  <Line
                    yAxisId="calls"
                    type="monotone"
                    dataKey="records"
                    stroke="var(--color-records)"
                    strokeWidth={2}
                    strokeDasharray="7 4"
                    strokeLinecap="round"
                    strokeLinejoin="round"
                    dot={false}
                    activeDot={{ r: 4, fill: "var(--color-records)", stroke: "var(--background)", strokeWidth: 2 }}
                    isAnimationActive={false}
                  />
                </ComposedChart>
              </ChartContainer>
              <div className="mt-3 flex flex-wrap items-center justify-between gap-2 text-xs text-muted-foreground">
                <span>
                  {rangeLabel(range)}合计 {formatTokenCompact(tokenTotal(totals))} Token · {exact.format(totals.records)} 次调用
                </span>
                <span>数据覆盖至 {formatDateTime(source.coverageEndAt)}</span>
              </div>
              <p className="sr-only">
                {chartData
                  .map(
                    (point) =>
                      `${point.date} 使用 ${formatTokenCompact(tokenTotal(point))} Token，${exact.format(point.records)} 次调用`,
                  )
                  .join("；")}
              </p>
            </>
          )}
        </CardContent>
      </Card>
    </section>
  );
}
