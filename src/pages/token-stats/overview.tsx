import { useMemo } from "react";
import { ArrowDownToLine, ArrowUpFromLine, Gauge, MessagesSquare } from "lucide-react";
import { Card, CardContent, CardHeader } from "@/components/ui/card";
import { statsRangeLabel, type StatsRangeKey } from "@/lib/stats-range";
import type {
  TokenStatsSource,
} from "@/lib/types";

import { exact, formatTokenCompact, overviewTotals, tokenTotal } from "./data";
import { CompactComposition, SectionTitle, StatMetric } from "./metric";

/**
 * 第一屏四指标（总 Token / 输入 / 输出 / 缓存命中率）。
 *
 * **本卡片不持有范围状态**：范围是全页唯一的（见 `@/lib/stats-range`），由
 * `token-stats/index.tsx` 的头像控件控制后传进来。改造前这里有一套「今天 / 7 天 /
 * 30 天 / 本月」按钮，与趋势图那套同名不同默认值，用户无法判断哪个是全局口径。
 */
export function Overview({ source, range }: { source: TokenStatsSource; range: StatsRangeKey }) {
  const summary = useMemo(() => overviewTotals(source, range), [range, source]);
  const cacheRate = summary.cacheHitRate;
  const rangeText = statsRangeLabel(range);

  return (
    <section className="min-w-0 space-y-2.5" aria-labelledby="token-overview-title">
      <SectionTitle id="token-overview-title">Token 总览</SectionTitle>
      <Card
        className="min-w-0 gap-0 overflow-hidden rounded-2xl bg-card/70 py-0 shadow-none"
        aria-label="Token 总览"
      >
        <CardHeader className="gap-0 px-4 pt-3 pb-0 sm:px-5">
          <div className="flex min-w-0 flex-wrap items-center justify-between gap-3">
            <CompactComposition value={summary} />
            {/* 范围不在卡内切换，只在卡上标注「当前这四个数字属于哪个范围」 */}
            <span className="shrink-0 text-[11px] leading-5 text-muted-foreground tabular-nums">
              {rangeText} · {exact.format(summary.records)} 次调用
            </span>
          </div>
        </CardHeader>
        <CardContent className="grid min-w-0 grid-cols-1 divide-y divide-border/60 p-0 sm:grid-cols-4 sm:divide-y-0 sm:py-5">
          <StatMetric icon={MessagesSquare} label="总 Token" value={formatTokenCompact(tokenTotal(summary))} />
          <StatMetric icon={ArrowUpFromLine} label="输入 Token" value={formatTokenCompact(summary.input)} divided />
          <StatMetric
            icon={ArrowDownToLine}
            label="输出 Token"
            value={formatTokenCompact(summary.output)}
            divided
          />
          <StatMetric
            icon={Gauge}
            label="缓存命中率"
            value={cacheRate == null ? "—" : `${(cacheRate * 100).toFixed(1)}%`}
            divided
          />
        </CardContent>
      </Card>
    </section>
  );
}
