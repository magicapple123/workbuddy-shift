import { useCallback, useEffect, useMemo, useState } from "react";
import { Download } from "lucide-react";
import {
  CalendarDays,
  CalendarRange,
  CircleAlert,
  CircleCheck,
  Loader2,
  Sparkles,
  RefreshCw,
  TrendingDown,
} from "lucide-react";

import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { DemoAction } from "@/components/demo-action";
import { Card, CardContent } from "@/components/ui/card";
import * as api from "@/lib/api";
import { formatAmount, formatDateTime } from "@/lib/format";
import { saveCsvFile } from "@/lib/export-download";
import {
  invalidateCreditStatisticsCache,
  loadCreditStatistics,
  readCreditStatisticsCache,
} from "@/lib/stats-cache";
import { toast } from "sonner";
import type {
  CreditStatistics,
  WbVariant,
} from "@/lib/types";
import { accountVariant } from "@/lib/variant";
import { useAccountsStore } from "@/stores/accounts";
import { SelectedAccountDetails } from "./account-details";
import { AccountUsageShare } from "./account-usage-share";
import { StatsVariantView, VARIANT_VIEW_OPTIONS, isOfficialUsageAvailable, recomputeLocalStats, recomputeOfficialUsage, resolveAccountVariant, statsVariantViewLabel } from "./data";
import { ModelBreakdown } from "./model-breakdown";
import { useProgressiveRefresh } from "./progressive-refresh";
import { buildCreditStatsCsv, creditStatsFileName } from "./export";
import { StatMetric, TrendChart } from "./trend-chart";

/** 毫秒展示：未完成显示「…」，避免把 0 当成「瞬间完成」。 */
function formatMs(value: number | null): string {
  return value === null ? "…" : `${(value / 1000).toFixed(1)}s`;
}

/** 进入统计页时距上次采集超过此时长（ms）则自动重新采集一次 */
const STATISTICS_AUTO_REFRESH_MS = 30 * 60 * 1000;

export default function CreditStatsPage() {
  const {
    accounts,
    creditMap,
    creditLoadingMap,
    fetchAll,
    refreshCredits,
  } = useAccountsStore();
  // 首帧直接渲染共享缓存（可能来自启动预热）：有缓存就不出整页 loading。
  const [stats, setStats] = useState<CreditStatistics | null>(() => readCreditStatisticsCache());
  const [loading, setLoading] = useState(() => readCreditStatisticsCache() === null);
  const [error, setError] = useState<string | null>(null);
  const [viewVariant, setViewVariant] = useState<StatsVariantView>("all");

  // 渐进式刷新：本地数字 → 积分 / 官方用量并行；按钮显示「更新中 N/2 项」。
  const progressive = useProgressiveRefresh({
    commit: (next) => {
      // `progressive-refresh` 直接调 api（三段编排要拿到各自的中间结果），绕过了共享缓存，
      // 这里让缓存失效，避免「刚刷新完、切页回来又看到旧数字」。
      invalidateCreditStatisticsCache();
      setStats(next);
      setError(null);
    },
    refreshCredits,
    loadAccountIds: async () => {
      await fetchAll();
      return useAccountsStore.getState().accounts.map((account) => account.id);
    },
  });

  const load = useCallback(
    async (refresh = false, options?: { background?: boolean }) => {
      const background = options?.background === true;
      const cached = readCreditStatisticsCache();
      if (!refresh && cached) {
        setStats(cached);
        setLoading(false);
        setError(null);
        return;
      }
      // 后台刷新时保留已渲染的旧数据：采集 12 个账号的官方用量要几秒，
      // 把整页推回骨架屏等于让用户重新等一次，而这期间旧数据仍然可用。
      if (!background) setLoading(true);
      setError(null);
      try {
        await fetchAll();
        const accountState = useAccountsStore.getState();
        if (accountState.error) {
          throw new Error(accountState.error);
        }
        const currentAccounts = accountState.accounts;
        const ids = currentAccounts.map((account) => account.id);
        if (refresh && ids.length > 0) {
          await refreshCredits(ids);
        }
        // 共享模块负责 single-flight 与成功缓存：与启动预热共用同一份数据来源。
        setStats(await loadCreditStatistics(refresh));
      } catch (cause) {
        if (!background) setError(api.asError(cause));
      } finally {
        if (!background) setLoading(false);
      }
    },
    [fetchAll, refreshCredits],
  );

  useEffect(() => {
    void (async () => {
      // 先渲染本地缓存（后端只读磁盘，不用等网络；预热命中时这里不发请求）
      await load(false);
      if (api.isDemoMode()) return;
      // 过期只看后端记录的采集时刻：会话内变量每次启动都归零，判断不出
      // 「缓存其实是几小时前采的」，所以刚打开应用时不会自动刷新。
      // 启动预热已在同一门控下补采过并回填缓存，这里读到的是新鲜时刻，不会二次采集。
      const collectedAt = readCreditStatisticsCache()?.officialUsage?.collectedAt ?? 0;
      if (collectedAt > 0 && Date.now() - collectedAt < STATISTICS_AUTO_REFRESH_MS) return;
      if (useAccountsStore.getState().accounts.length === 0) return;
      // 后台补采：不清空、不挡交互，采完再替换数字。
      await load(true, { background: true });
    })();
  }, [load]);

  const variantByAccountId = useMemo(() => {
    const map = new Map<string, WbVariant>();
    for (const account of accounts) {
      map.set(account.id, accountVariant(account));
    }
    return map;
  }, [accounts]);

  const variantAccountIds = useMemo(() => {
    if (!stats || viewVariant === "all") return null;
    return new Set(
      stats.accounts
        .filter((account) => resolveAccountVariant(account, variantByAccountId) === viewVariant)
        .map((account) => account.accountId),
    );
  }, [stats, viewVariant, variantByAccountId]);

  const filteredStats = useMemo(() => {
    // D1: viewVariant === "all" 时直接沿用后端聚合值（stats.summary / stats.daily），
    // 不得走过滤重算路径，避免把国内版与国际版不可比积分加回一起，破坏改动前回归基线。
    if (viewVariant === "all" || !stats || !variantAccountIds) return stats;
    return recomputeLocalStats(stats, variantAccountIds, viewVariant, variantByAccountId);
  }, [stats, viewVariant, variantAccountIds, variantByAccountId]);

  const filteredOfficial = useMemo(() => {
    const officialUsage = stats?.officialUsage;
    // D1: 「全部」原样使用 officialUsage.summary / daily / models，不走过滤重算。
    if (viewVariant === "all" || !officialUsage || !variantAccountIds) return officialUsage;
    return recomputeOfficialUsage(officialUsage, variantAccountIds);
  }, [stats, viewVariant, variantAccountIds]);

  /** 导出：当前档位筛选，全量日期；官方明细标注每账号上限与是否截断。 */
  async function exportCsv() {
    if (!stats) {
      toast.error("还没有可导出的统计数据");
      return;
    }
    try {
      const content = buildCreditStatsCsv(stats, filteredOfficial ?? undefined);
      const outcome = await saveCsvFile(creditStatsFileName(viewVariant), content);
      if (outcome === "saved") {
        toast.success("已导出积分统计 CSV", {
          description: `${viewVariant} · ${(content.length / 1024).toFixed(0)} KB`,
        });
      }
    } catch (cause) {
      toast.error("导出失败", { description: api.asError(cause) });
    }
  }

  const officialUsage = filteredOfficial;
  const official = isOfficialUsageAvailable(officialUsage) ? officialUsage : undefined;
  const variantEmpty =
    viewVariant !== "all" && variantAccountIds !== null && variantAccountIds.size === 0;

  return (
    <div className="mx-auto w-full max-w-[1180px] min-w-0 px-4 py-6 sm:px-8 sm:py-9">
      <header className="mb-10 flex min-w-0 flex-wrap items-start justify-between gap-4 sm:mb-12">
        <div className="min-w-0">
          <h1 className="text-[28px] font-semibold tracking-tight">积分统计</h1>
          <p className="mt-2 max-w-2xl text-sm leading-6 text-muted-foreground">
            当前数据更新于 {stats ? formatDateTime(official?.collectedAt ?? stats.generatedAt) : "—"}
          </p>
          {progressive.running || progressive.timing.officialMs !== null ? (
            <p className="mt-1 flex flex-wrap gap-x-3 text-[11px] text-muted-foreground">
              <span>本地 {formatMs(progressive.timing.localMs)}</span>
              <span>积分 {formatMs(progressive.timing.creditsMs)}</span>
              <span>官方 {formatMs(progressive.timing.officialMs)}</span>
              {progressive.phase.local ? <span className="text-success">本地已更新</span> : null}
            </p>
          ) : null}
        </div>
        <div className="flex min-w-0 max-w-full flex-wrap items-center justify-end gap-2">
          <div
            className="flex max-w-full flex-wrap gap-1 rounded-lg bg-muted p-1"
            role="tablist"
            aria-label="档位筛选"
          >
            {VARIANT_VIEW_OPTIONS.map((option) => (
              <button
                key={option.key}
                type="button"
                role="tab"
                aria-selected={viewVariant === option.key}
                className={`cursor-pointer rounded-md px-2.5 py-1.5 text-xs transition-colors ${
                  viewVariant === option.key
                    ? "bg-background font-medium text-foreground shadow-sm"
                    : "text-muted-foreground hover:text-foreground"
                }`}
                onClick={() => setViewVariant(option.key)}
              >
                {option.label}
              </button>
            ))}
          </div>
          <DemoAction>
            <Button
              className="shrink-0"
              variant="outline"
              size="sm"
              onClick={() => void exportCsv()}
              disabled={!stats}
              title={stats ? "导出当前档位筛选下的 CSV" : "没有可导出的数据"}
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
              onClick={() => void progressive.run()}
              disabled={loading || progressive.running}
            >
              {progressive.running ? <Loader2 className="animate-spin" /> : <RefreshCw />}
              {progressive.running ? `更新中 ${progressive.done}/2 项` : "刷新统计"}
            </Button>
          </DemoAction>
        </div>
      </header>

      {error && (
        <Alert variant="destructive" className="mb-5">
          <CircleAlert />
          <AlertTitle>统计加载失败</AlertTitle>
          <AlertDescription className="flex flex-wrap items-center gap-3">
            <span>{error}</span>
            <Button size="sm" variant="outline" onClick={() => void load()}>
              重试
            </Button>
          </AlertDescription>
        </Alert>
      )}

      {loading && !stats ? (
        <div className="flex items-center gap-2 py-20 text-sm text-muted-foreground">
          <Loader2 className="animate-spin" />
          正在采集账号积分并加载统计…
        </div>
      ) : stats && filteredStats ? (
        <div className="min-w-0 space-y-12">
          {viewVariant === "all" && accounts.length === 0 && (
            <Alert>
              <CircleAlert />
              <AlertTitle>暂无当前账号</AlertTitle>
              <AlertDescription>可以先去账号管理导入或登录账号；历史事件仍会保留在下方最近事件中。</AlertDescription>
            </Alert>
          )}

          {variantEmpty ? (
            <div className="rounded-xl border border-dashed px-4 py-16 text-center text-sm text-muted-foreground">
              <p>暂无{statsVariantViewLabel(viewVariant)}账号的积分数据。</p>
              <p className="mt-2 text-xs leading-5">国内版与国际版积分体系不同，不会合并计算。</p>
            </div>
          ) : (
            <>
              {officialUsage && officialUsage.status !== "complete" && (
                <Alert variant="warning">
                  <CircleAlert />
                  <AlertTitle>
                    {officialUsage.status === "partial" ? "部分账号官方用量未同步" : "官方用量暂不可用"}
                  </AlertTitle>
                  <AlertDescription>
                    {officialUsage.status === "partial"
                      ? `已同步 ${officialUsage.accounts.filter((account) => account.ok).length}/${officialUsage.accounts.length} 个当前账号；失败账号的官方数值显示为“—”。`
                      : "今日、近 7 天和本月消耗将使用本地观察口径；官方接口恢复后刷新即可重新同步。"}
                    {officialUsage.errors.length > 0 && (
                      <span className="text-xs text-warning-foreground">
                        {officialUsage.errors.map((item) => `${item.accountName}: ${item.error}`).join("；")}
                      </span>
                    )}
                  </AlertDescription>
                </Alert>
              )}

              <Card className="min-w-0 gap-0 overflow-hidden rounded-2xl bg-card/70 py-0 shadow-none" aria-label="积分总览">
                <CardContent className="grid min-w-0 grid-cols-1 divide-y divide-border/60 p-0 sm:grid-cols-4 sm:divide-y-0 sm:py-5">
                  <StatMetric
                    icon={Sparkles}
                    label="剩余积分"
                    value={formatAmount(filteredStats.summary.currentRemaining)}
                  />
                  <StatMetric
                    icon={TrendingDown}
                    label="今日消耗"
                    value={formatAmount(official ? official.summary.usageToday : filteredStats.summary.usageToday)}
                    divided
                  />
                  <StatMetric
                    icon={CalendarDays}
                    label="近 7 天消耗"
                    value={formatAmount(official ? official.summary.usage7Days : filteredStats.summary.usage7Days)}
                    divided
                  />
                  <StatMetric
                    icon={CalendarRange}
                    label="本月消耗"
                    value={formatAmount(official ? official.summary.usageThisMonth : filteredStats.summary.usageThisMonth)}
                    divided
                  />
                </CardContent>
              </Card>

              {!official && !filteredStats.coverageStartAt && filteredStats.events.some((event) => event.kind === "checkin") && (
                <Alert>
                  <CircleCheck />
                  <AlertTitle>目前只有签到记录</AlertTitle>
                  <AlertDescription>签到不会被计入积分消耗。首次成功采集积分资源后，趋势统计才会开始累计。</AlertDescription>
                </Alert>
              )}

              <AccountUsageShare
                stats={filteredStats}
                officialUsage={officialUsage}
                variantByAccountId={variantByAccountId}
              />

              <TrendChart
                stats={filteredStats}
                officialUsage={officialUsage}
              />

              {official && (
                <ModelBreakdown
                  officialUsage={official}
                />
              )}

              {/* 与下方「积分明细」重复，先隐藏。
              <AccountTable stats={stats} officialUsage={officialUsage} selectedId={selectedId} onSelect={setSelectedId} />
              */}

              <SelectedAccountDetails
                stats={filteredStats}
                officialUsage={officialUsage}
                creditMap={creditMap}
                creditLoadingMap={creditLoadingMap}
              />
            </>
          )}
        </div>
      ) : (
        <div className="rounded-xl border border-dashed px-4 py-16 text-center text-sm text-muted-foreground">
          暂无统计数据，请点击刷新重试。
        </div>
      )}
    </div>
  );
}
