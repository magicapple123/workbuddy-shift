import { useEffect, useState } from "react";
import { CircleAlert, Download, ListTree, Loader2, RefreshCw } from "lucide-react";
import { toast } from "sonner";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { DemoAction } from "@/components/demo-action";
import { CodeBuddyCnIdeMark, CodeBuddyMark, WorkBuddyAiMark, WorkBuddyMark } from "@/components/product-marks";
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { Skeleton } from "@/components/ui/skeleton";
import * as api from "@/lib/api";
import { formatDateTime } from "@/lib/format";
import {
  isTokenStatisticsCacheFresh,
  loadTokenStatistics,
  readTokenStatisticsCache,
} from "@/lib/stats-cache";
import type {
  TokenStatistics,
} from "@/lib/types";

import { saveCsvFile } from "@/lib/export-download";
import { Dashboard } from "./dashboard";
import { buildTokenStatsCsv, tokenStatsFileName } from "./export";
import { SourceKey, isSourceKey, persistPreferredTokenSource, readPreferredTokenSource } from "./data";
import { RequestDetailDialog } from "./request-detail";
import { TokenStatsLoadingSkeleton } from "./skeleton";

export default function TokenStatsPage() {
  // 首帧直接用共享缓存（启动预热可能已经扫完盘），避免把整页推回骨架屏再等一次 IPC。
  const [stats, setStats] = useState<TokenStatistics | null>(() => readTokenStatisticsCache());
  const [active, setActive] = useState<SourceKey>(readPreferredTokenSource);
  const [loading, setLoading] = useState(() => readTokenStatisticsCache() === null);
  const [error, setError] = useState<string | null>(null);
  /** 0 = 首次进入（缓存新鲜时不发请求）；>0 = 用户点了刷新/重试，强制重新扫描。 */
  const [reload, setReload] = useState(0);
  const [detailOpen, setDetailOpen] = useState(false);

  useEffect(() => {
    const force = reload > 0;
    // 10 分钟窗口内直接渲染缓存：与启动预热共用同一份判定（都看响应自带的 generatedAt），
    // 因此「预热刚扫过 → 进入本页」不会触发第二次全量扫描。
    if (!force && isTokenStatisticsCacheFresh()) {
      setStats(readTokenStatisticsCache());
      setLoading(false);
      setError(null);
      return;
    }
    let disposed = false;
    setLoading(true);
    setError(null);
    // 统一走共享模块：已有在途请求会被 join，不会与预热并发扫两遍。
    loadTokenStatistics({ force })
      .then((result) => {
        if (disposed) return;
        setStats(result);
      })
      .catch((cause: unknown) => {
        // 失败时保留旧数据继续展示，由下面的错误条提示重试（不弹 toast）。
        if (!disposed) setError(api.asError(cause));
      })
      .finally(() => {
        if (!disposed) setLoading(false);
      });
    return () => {
      disposed = true;
    };
  }, [reload]);

  useEffect(() => {
    if (!stats || stats.sources.length === 0) return;
    const available = stats.sources.map((item) => item.source);
    const next = available.includes(active)
      ? active
      : available.includes("workbuddy")
        ? "workbuddy"
        : available[0];
    if (!next) return;
    if (next !== active) setActive(next);
    persistPreferredTokenSource(next);
  }, [active, stats]);

  const source = stats?.sources.find((item) => item.source === active);

  /** 导出当前数据源：范围是全量日期，图表时间范围不参与。 */
  async function exportCsv() {
    if (!source) {
      toast.error("当前数据源没有可导出的数据");
      return;
    }
    try {
      const content = buildTokenStatsCsv(source);
      const outcome = await saveCsvFile(tokenStatsFileName(source.source), content);
      if (outcome === "saved") {
        toast.success("已导出 Token 统计 CSV", {
          description: `${source.source} · ${(content.length / 1024).toFixed(0)} KB`,
        });
      }
    } catch (cause) {
      toast.error("导出失败", { description: api.asError(cause) });
    }
  }

  return (
    <div className="mx-auto w-full max-w-[1180px] min-w-0 px-4 py-6 sm:px-8 sm:py-9">
      <header className="mb-6 flex min-w-0 flex-wrap items-start justify-between gap-4">
        <div className="min-w-0">
          {loading && !stats ? (
            <div aria-hidden="true">
              <Skeleton className="h-8 w-40" />
              <Skeleton className="mt-2 h-5 w-64 max-w-full" />
            </div>
          ) : (
            <>
              <h1 className="text-[28px] font-semibold tracking-tight">Token 统计</h1>
              <p className="mt-2 max-w-2xl text-sm leading-6 text-muted-foreground">
                当前数据更新于 {stats ? formatDateTime(stats.generatedAt) : "—"}
              </p>
            </>
          )}
        </div>
        <div className="flex max-w-full flex-wrap items-center justify-end gap-2">
          {active === "codebuddy-cli" && (
            <Button
              className="shrink-0"
              variant="outline"
              size="sm"
              onClick={() => setDetailOpen(true)}
            >
              <ListTree />
              查看请求明细
            </Button>
          )}
          <DemoAction>
            <Button
              className="shrink-0"
              variant="outline"
              size="sm"
              onClick={() => void exportCsv()}
              disabled={!source}
              title={source ? "导出当前数据源的 CSV" : "没有可导出的数据"}
            >
              <Download />
              导出 CSV
            </Button>
          </DemoAction>
          <DemoAction>
            <Button
              className="shrink-0"
              variant="outline"
              size="sm"
              onClick={() => setReload((value) => value + 1)}
              disabled={loading}
            >
              {loading ? <Loader2 className="animate-spin" /> : <RefreshCw />}
              刷新统计
            </Button>
          </DemoAction>
        </div>
      </header>

      {/* 数据源 Tab 独占一行：数据源变多时不与右上角操作挤在同一排 */}
      {loading && !stats ? (
        <Skeleton className="mb-8 h-9 w-full max-w-md rounded-lg" aria-hidden="true" />
      ) : (
        <Tabs
          className="mb-8 min-w-0 gap-0"
          value={active}
          onValueChange={(value) => {
            if (!isSourceKey(value)) return;
            if (stats && !stats.sources.some((item) => item.source === value)) return;
            setActive(value);
          }}
        >
          <TabsList className="h-auto max-w-full flex-wrap" aria-label="Token 数据来源">
            <TabsTrigger
              className="max-w-full gap-2 whitespace-normal"
              value="workbuddy"
              disabled={Boolean(stats && !stats.sources.some((item) => item.source === "workbuddy"))}
            >
              <WorkBuddyMark size={16} />
              WorkBuddy
            </TabsTrigger>
            <TabsTrigger
              className="max-w-full gap-2 whitespace-normal"
              value="workbuddy-ai"
              disabled={Boolean(stats && !stats.sources.some((item) => item.source === "workbuddy-ai"))}
            >
              <WorkBuddyAiMark size={16} />
              WorkBuddy 国际版
            </TabsTrigger>
            <TabsTrigger
              className="max-w-full gap-2 whitespace-normal"
              value="codebuddy-cli"
              disabled={Boolean(stats && !stats.sources.some((item) => item.source === "codebuddy-cli"))}
            >
              <CodeBuddyMark size={16} />
              CodeBuddy CLI
            </TabsTrigger>
            <TabsTrigger
              className="max-w-full gap-2 whitespace-normal"
              value="codebuddy-ide"
              disabled={Boolean(stats && !stats.sources.some((item) => item.source === "codebuddy-ide"))}
            >
              <CodeBuddyCnIdeMark size={16} />
              CodeBuddy IDE / VS Code
            </TabsTrigger>
          </TabsList>
        </Tabs>
      )}

      {error && (
        <Alert variant="destructive" className="mb-5">
          <CircleAlert />
          <AlertTitle>统计加载失败</AlertTitle>
          <AlertDescription className="flex flex-wrap items-center gap-3">
            <span>{error}</span>
            <Button size="sm" variant="outline" onClick={() => setReload((value) => value + 1)}>
              重试
            </Button>
          </AlertDescription>
        </Alert>
      )}

      {loading && !stats ? <TokenStatsLoadingSkeleton /> : source ? (
        <Dashboard source={source} />
      ) : (
        !error && (
          <div className="rounded-xl border border-dashed px-4 py-16 text-center text-sm text-muted-foreground">
            该来源暂无可用统计数据，请点击刷新重试。
          </div>
        )
      )}

      {source && (
        <RequestDetailDialog
          open={detailOpen}
          onOpenChange={setDetailOpen}
          source={source}
        />
      )}
    </div>
  );
}
