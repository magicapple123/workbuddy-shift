import { useMemo, useState } from "react";
import { ArrowDownToLine, ArrowUpFromLine, Gauge, MessagesSquare } from "lucide-react";
import { Card, CardContent, CardHeader } from "@/components/ui/card";
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs";
import type {
  TokenStatsSource,
} from "@/lib/types";

import { OVERVIEW_RANGE_OPTIONS, OverviewRangeKey, formatTokenCompact, overviewTotals, tokenTotal } from "./data";
import { CompactComposition, SectionTitle, StatMetric } from "./metric";

export function Overview({ source }: { source: TokenStatsSource }) {
  const [range, setRange] = useState<OverviewRangeKey>("today");
  const summary = useMemo(() => overviewTotals(source, range), [range, source]);
  const cacheRate = summary.cacheHitRate;

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
            <Tabs
              className="min-w-0 shrink-0 gap-0"
              value={range}
              onValueChange={(value) => setRange(value as OverviewRangeKey)}
            >
              <TabsList
                className="grid h-auto w-full grid-cols-2 sm:inline-flex sm:w-fit sm:flex-wrap"
                aria-label="总览范围"
              >
                {OVERVIEW_RANGE_OPTIONS.map((option) => (
                  <TabsTrigger key={option.key} value={option.key} className="px-2">
                    {option.label}
                  </TabsTrigger>
                ))}
              </TabsList>
            </Tabs>
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
