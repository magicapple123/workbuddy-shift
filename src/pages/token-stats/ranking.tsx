import type {
  ReactNode,
} from "react";

import { Card, CardContent, CardDescription, CardHeader } from "@/components/ui/card";
import type {
  TokenStatsGroup,
} from "@/lib/types";

import { RANKING_LIMIT, formatTokenCompact, formatTokenExact, percentage, shortAccountLabel, tokenTotal } from "./data";
import { SectionTitle } from "./metric";

export function Ranking({
  groups,
  denominator,
  description,
  controls,
}: {
  groups: TokenStatsGroup[];
  denominator: number;
  description: string;
  controls?: ReactNode;
}) {
  const rows = groups.slice(0, RANKING_LIMIT);

  return (
    <Card className="min-w-0 gap-0 rounded-xl py-0 shadow-none">
      <CardHeader className="gap-0 px-4 pt-3 pb-0 sm:px-5">
        <div className="flex min-w-0 flex-wrap items-center justify-between gap-3">
          <CardDescription className="min-w-0 text-xs">{description}</CardDescription>
          {controls}
        </div>
      </CardHeader>
      <CardContent className="space-y-3 px-4 pt-3 pb-5 sm:px-5">
        {rows.map((row, index) => {
          const amount = tokenTotal(row);
          const share = percentage(amount, denominator);
          return (
            <div key={row.key}>
              <div className="mb-1.5 flex min-w-0 items-center gap-3 text-xs">
                <span className="w-5 shrink-0 font-mono text-muted-foreground">
                  {String(index + 1).padStart(2, "0")}
                </span>
                <span className="min-w-0 flex-1 truncate font-medium" title={row.key}>
                  {row.key}
                </span>
                <span
                  className="shrink-0 tabular-nums"
                  title={`${formatTokenExact(amount)} Token`}
                  aria-label={`${formatTokenExact(amount)} Token`}
                >
                  {formatTokenCompact(amount)}
                </span>
                <span className="w-12 shrink-0 text-right text-muted-foreground tabular-nums">
                  {share}
                </span>
              </div>
              <div className="ml-8 h-1.5 overflow-hidden rounded-full bg-muted">
                <div
                  className="h-full rounded-full bg-primary"
                  style={{ width: share === "—" ? "0%" : share }}
                />
              </div>
            </div>
          );
        })}
        {rows.length === 0 && (
          <div className="rounded-lg border border-dashed px-4 py-10 text-center text-sm text-muted-foreground">
            暂无统计数据
          </div>
        )}
      </CardContent>
    </Card>
  );
}

export function SessionRanking({
  groups,
  denominator,
  heading = true,
}: {
  groups: TokenStatsGroup[];
  denominator: number;
  /** 折叠区里由 `AccordionTrigger` 承担标题，卡内不再重复渲染区块标题。 */
  heading?: boolean;
}) {
  const rows = groups.slice(0, RANKING_LIMIT);

  return (
    <section
      className="min-w-0 space-y-2.5"
      aria-labelledby={heading ? "token-sessions-title" : undefined}
    >
      {heading ? <SectionTitle id="token-sessions-title">消耗最高的会话</SectionTitle> : null}
      <Card className="min-w-0 gap-0 rounded-xl py-0 shadow-none">
        <CardHeader className="px-4 pt-3 pb-0 sm:px-5">
          <CardDescription className="text-xs">
            按本地聚合 Token 从高到低排列（会话维度只有全量聚合，不受统计范围影响）。
          </CardDescription>
        </CardHeader>
        <CardContent className="space-y-3 px-4 pt-3 pb-5 sm:px-5">
          {rows.map((row, index) => {
            const amount = tokenTotal(row);
            const share = percentage(amount, denominator);
            const label = row.title?.trim() || "未命名会话";
            const detail = [row.project, row.title ? undefined : row.sessionId, shortAccountLabel(row.uid)]
              .filter(Boolean)
              .join(" · ");
            return (
              <div key={row.key}>
                <div className="mb-1.5 flex min-w-0 items-start gap-3 text-xs">
                  <span className="mt-0.5 w-5 shrink-0 font-mono text-muted-foreground">
                    {String(index + 1).padStart(2, "0")}
                  </span>
                  <div className="min-w-0 flex-1">
                    <div className="truncate font-medium" title={label} aria-label={label}>
                      {label}
                    </div>
                    {detail && (
                      <div className="mt-0.5 truncate text-[11px] text-muted-foreground" title={detail}>
                        {detail}
                      </div>
                    )}
                  </div>
                  <span
                    className="mt-0.5 shrink-0 tabular-nums"
                    title={`${formatTokenExact(amount)} Token`}
                    aria-label={`${formatTokenExact(amount)} Token`}
                  >
                    {formatTokenCompact(amount)}
                  </span>
                  <span className="mt-0.5 w-12 shrink-0 text-right text-muted-foreground tabular-nums">
                    {share}
                  </span>
                </div>
                <div className="ml-8 h-1.5 overflow-hidden rounded-full bg-muted">
                  <div
                    className="h-full rounded-full bg-primary"
                    style={{ width: share === "—" ? "0%" : share }}
                  />
                </div>
              </div>
            );
          })}
          {rows.length === 0 && (
            <div className="rounded-lg border border-dashed px-4 py-10 text-center text-sm text-muted-foreground">
              暂无统计数据
            </div>
          )}
        </CardContent>
      </Card>
    </section>
  );
}
