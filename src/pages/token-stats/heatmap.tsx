import { useLayoutEffect, useRef } from "react";
import { Card, CardContent, CardDescription, CardHeader } from "@/components/ui/card";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { dateKey } from "@/lib/format";
import type {
  TokenStatsGroup,
} from "@/lib/types";

import { exact, formatHeatmapDate, formatTokenCompact, formatTokenExact, tokenTotal } from "./data";
import { SectionTitle } from "./metric";

const HEATMAP_LEVEL_CLASS = [
  "bg-muted/70",
  "bg-primary/20",
  "bg-primary/40",
  "bg-primary/65",
  "bg-primary",
] as const;

export function Heatmap({ groups }: { groups: TokenStatsGroup[] }) {
  const scrollerRef = useRef<HTMLDivElement>(null);
  const valueByDate = new Map(groups.map((group) => [group.key, tokenTotal(group)]));
  const recordByDate = new Map(groups.map((group) => [group.key, group.records]));
  const today = new Date();
  today.setHours(12, 0, 0, 0);
  const todayKey = dateKey(today);
  const start = new Date(today);
  start.setDate(start.getDate() - start.getDay() - 52 * 7);

  const weeks = Array.from({ length: 53 }, (_, weekIndex) =>
    Array.from({ length: 7 }, (_, dayIndex) => {
      const date = new Date(start);
      date.setDate(start.getDate() + weekIndex * 7 + dayIndex);
      const key = dateKey(date);
      return {
        date,
        key,
        value: valueByDate.get(key) ?? 0,
        records: recordByDate.get(key) ?? 0,
        future: key > todayKey,
      };
    }),
  );
  const max = Math.max(
    1,
    ...weeks.flatMap((week) => week.filter((day) => !day.future).map((day) => day.value)),
  );
  const monthLabels = weeks.map((week, weekIndex) => {
    const firstOfMonth = week.find((day) => day.date.getDate() === 1);
    let labelDate: Date | null = null;
    if (firstOfMonth && firstOfMonth.key <= todayKey) {
      labelDate = firstOfMonth.date;
    } else if (weekIndex === 0) {
      labelDate = week[0].date;
    }
    if (!labelDate || dateKey(labelDate) > todayKey) return null;
    return labelDate.toLocaleDateString("zh-CN", {
      month: "short",
    });
  });
  const activeDays = weeks
    .flat()
    .filter((day) => !day.future && day.value > 0).length;

  useLayoutEffect(() => {
    const scroller = scrollerRef.current;
    if (!scroller) return;
    scroller.scrollLeft = scroller.scrollWidth - scroller.clientWidth;
  }, [groups]);

  return (
    <section className="min-w-0 space-y-2.5" aria-labelledby="token-heatmap-title">
      <SectionTitle id="token-heatmap-title">Token 活动</SectionTitle>
      <Card className="min-w-0 gap-0 rounded-xl py-0 shadow-none">
        <CardHeader className="px-4 pt-4 pb-0 sm:px-5">
          <div className="flex items-center justify-between gap-3">
            <CardDescription className="text-xs">最近一年按天显示 Token 活跃度。</CardDescription>
            <span className="shrink-0 text-xs font-medium text-foreground">每日</span>
          </div>
        </CardHeader>
        <CardContent className="min-w-0 px-4 pt-5 pb-5 sm:px-5">
          <div ref={scrollerRef} className="overflow-x-auto pb-1">
            <div
              className="min-w-[760px]"
              role="img"
              aria-label={`最近一年每日 Token 活动热力图，共 ${activeDays} 个活跃日`}
            >
              <div
                className="grid gap-1"
                style={{ gridTemplateColumns: "repeat(53, minmax(10px, 1fr))" }}
                aria-hidden="true"
              >
                {weeks.flatMap((week, weekIndex) =>
                  week.map((day, dayIndex) => {
                    const level = day.value
                      ? Math.max(1, Math.ceil(Math.sqrt(day.value / max) * 4))
                      : 0;
                    const cell = (
                      <span
                        key={day.key}
                        className={`aspect-square min-w-0 rounded-[3px] ${
                          day.future ? "opacity-0" : HEATMAP_LEVEL_CLASS[level]
                        }`}
                        style={{ gridColumn: weekIndex + 1, gridRow: dayIndex + 1 }}
                        aria-label={`${formatHeatmapDate(day.date)}使用了 ${formatTokenExact(day.value)} 个 Token`}
                      />
                    );

                    if (day.future) return cell;

                    return (
                      <Tooltip key={day.key} disableHoverableContent>
                        <TooltipTrigger asChild>{cell}</TooltipTrigger>
                        <TooltipContent
                          side="top"
                          sideOffset={7}
                          className="pointer-events-none rounded-lg bg-foreground px-2.5 py-1.5 text-xs leading-4 text-background shadow-md"
                        >
                          {formatHeatmapDate(day.date)} 使用了 {formatTokenCompact(day.value)} 个 Token
                          {day.records > 0 ? ` · ${exact.format(day.records)} 次调用` : ""}
                        </TooltipContent>
                      </Tooltip>
                    );
                  }),
                )}
              </div>
              <div
                className="mt-3 grid gap-1 text-[11px] text-muted-foreground"
                style={{ gridTemplateColumns: "repeat(53, minmax(10px, 1fr))" }}
                aria-hidden="true"
              >
                {monthLabels.map((label, index) => (
                  <span key={`${index}-${label ?? "empty"}`} className="whitespace-nowrap">
                    {label}
                  </span>
                ))}
              </div>
            </div>
          </div>
        </CardContent>
      </Card>
    </section>
  );
}
