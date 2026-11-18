

import { Card, CardContent, CardHeader } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import { RANKING_LIMIT } from "./data";

export function TokenStatsLoadingSkeleton() {
  return (
    <div
      className="min-w-0 space-y-12"
      role="status"
      aria-label="正在扫描本地会话日志…"
    >
      <span className="sr-only">正在扫描本地会话日志…</span>
      <p className="flex items-center gap-2 text-sm text-muted-foreground" aria-hidden="true">
        <span className="size-1.5 rounded-full bg-primary/70" />
        正在扫描本地会话日志…
      </p>
      {/* 首次要把本地日志全扫一遍，之后的解析结果带缓存，回来看几乎立刻出数。
          预先说明这一点，免得用户把"首次较慢"当成卡死。 */}
      <p className="-mt-2 text-xs text-muted-foreground/70" aria-hidden="true">
        首次统计需要读取本地会话日志，之后会快很多
      </p>

      <section className="min-w-0 space-y-2.5" aria-hidden="true">
        <Skeleton className="h-4 w-20" />
        <Card className="min-w-0 gap-0 overflow-hidden rounded-2xl bg-card/70 py-0 shadow-none">
          <CardContent className="grid min-w-0 grid-cols-1 divide-y divide-border/60 p-0 sm:grid-cols-4 sm:divide-y-0 sm:py-5">
            {Array.from({ length: 4 }, (_, index) => (
              <div
                key={index}
                className={`flex min-w-0 flex-col items-center justify-center px-4 py-5 sm:py-3 ${
                  index > 0 ? "sm:border-l sm:border-border/60" : ""
                }`}
              >
                <Skeleton className="h-4 w-24" />
                <Skeleton className="mt-3 h-8 w-28" />
              </div>
            ))}
          </CardContent>
        </Card>
      </section>

      <section className="min-w-0 space-y-2.5" aria-hidden="true">
        <Skeleton className="h-4 w-36" />
        <Card className="min-w-0 gap-0 overflow-hidden rounded-xl py-0 shadow-none">
          <CardHeader className="gap-0 px-4 pt-3 pb-0 sm:px-5">
            <div className="flex min-w-0 flex-wrap items-center justify-between gap-3">
              <Skeleton className="h-4 w-64 max-w-full" />
              <Skeleton className="h-8 w-40 rounded-lg" />
            </div>
          </CardHeader>
          <CardContent className="min-w-0 px-4 pt-3 pb-4 sm:px-5">
            <div className="mb-3 flex flex-wrap items-center gap-4">
              {Array.from({ length: 5 }, (_, index) => (
                <Skeleton key={index} className="h-4 w-16" />
              ))}
            </div>
            <Skeleton className="h-64 w-full rounded-lg sm:h-72" />
          </CardContent>
        </Card>
      </section>

      <section className="min-w-0 space-y-2.5" aria-hidden="true">
        <Skeleton className="h-4 w-24" />
        <Card className="min-w-0 gap-0 overflow-hidden rounded-xl py-0 shadow-none">
          <CardHeader className="px-4 pt-4 pb-0 sm:px-5">
            <Skeleton className="h-4 w-56 max-w-full" />
          </CardHeader>
          <CardContent className="px-4 pt-5 pb-5 sm:px-5">
            <Skeleton className="h-44 w-full rounded-lg" />
          </CardContent>
        </Card>
      </section>

      <section className="min-w-0 space-y-2.5" aria-hidden="true">
        <Skeleton className="h-4 w-24" />
        <Card className="min-w-0 gap-0 overflow-hidden rounded-xl py-0 shadow-none">
          <CardHeader className="gap-0 px-4 pt-3 pb-0 sm:px-5">
            <div className="flex items-center justify-between gap-3">
              <Skeleton className="h-4 w-56 max-w-full" />
              <Skeleton className="h-8 w-32 rounded-lg" />
            </div>
          </CardHeader>
          <CardContent className="space-y-3 px-4 pt-3 pb-5 sm:px-5">
            {Array.from({ length: RANKING_LIMIT }, (_, index) => (
              <div key={index} className="space-y-1.5">
                <div className="flex items-center gap-3">
                  <Skeleton className="h-4 w-5" />
                  <Skeleton className="h-4 flex-1" />
                  <Skeleton className="h-4 w-16" />
                </div>
                <Skeleton className="ml-8 h-1.5 w-[70%] rounded-full" />
              </div>
            ))}
          </CardContent>
        </Card>
      </section>

      <section className="min-w-0 space-y-2.5" aria-hidden="true">
        <Skeleton className="h-4 w-36" />
        <Card className="min-w-0 gap-0 overflow-hidden rounded-xl py-0 shadow-none">
          <CardHeader className="px-4 pt-3 pb-0 sm:px-5">
            <Skeleton className="h-4 w-56 max-w-full" />
          </CardHeader>
          <CardContent className="space-y-3 px-4 pt-3 pb-5 sm:px-5">
            {Array.from({ length: RANKING_LIMIT }, (_, index) => (
              <div key={index} className="space-y-1.5">
                <div className="flex items-start gap-3">
                  <Skeleton className="mt-0.5 h-4 w-5" />
                  <Skeleton className="h-8 flex-1" />
                  <Skeleton className="h-4 w-16" />
                </div>
                <Skeleton className="ml-8 h-1.5 w-[70%] rounded-full" />
              </div>
            ))}
          </CardContent>
        </Card>
      </section>
    </div>
  );
}
