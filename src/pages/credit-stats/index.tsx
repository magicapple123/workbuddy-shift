import { useCallback, useEffect, useMemo, useState } from "react";
import { Download, FileSpreadsheet } from "lucide-react";
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
import { ExportMenu } from "@/components/export-menu";
import { Card, CardContent } from "@/components/ui/card";
import * as api from "@/lib/api";
import { formatAmount, formatDateTime } from "@/lib/format";
import { saveCsvFile, saveXlsxFile } from "@/lib/export-download";
import {
  invalidateCreditStatisticsCache,
  loadCreditStatistics,
  readCreditStatisticsCache,
} from "@/lib/stats-cache";
import { toast } from "sonner";
import type {
  CreditRange,
  CreditStatistics,
  WbVariant,
} from "@/lib/types";

import { accountVariant } from "@/lib/variant";
import { useAccountsStore } from "@/stores/accounts";
import { SelectedAccountDetails } from "./account-details";
import { AccountUsageShare } from "./account-usage-share";
import { ClientFilterMenu } from "./client-filter-menu";
import {
  CLIENT_VIEW_LABELS,
  clientViewOptions,
  filterOfficialByClient,
  ledgerClientViewOptions,
  ledgerToOfficialUsage,
  type CreditClientKey,
} from "./data";
import {
  creditSourceBadge,
  localCoverageText,
  usageWindowValue,
} from "./coverage";
import { StatsVariantView, VARIANT_VIEW_OPTIONS, isOfficialUsageAvailable, recomputeLocalStats, recomputeOfficialUsage, resolveAccountVariant, statsVariantViewLabel } from "./data";
import { ModelBreakdown } from "./model-breakdown";
import { useProgressiveRefresh } from "./progressive-refresh";
import { buildCreditStatsCsv, creditStatsFileName } from "./export";
import { buildCreditStatsXlsx, creditStatsXlsxFileName } from "./export-xlsx";
import { StatMetric, TrendChart } from "./trend-chart";

/** 毫秒展示：未完成显示「…」，避免把 0 当成「瞬间完成」。 */
function formatMs(value: number | null): string {
  return value === null ? "…" : `${(value / 1000).toFixed(1)}s`;
}

/** 进入统计页时距上次采集超过此时长（ms）则自动重新采集一次 */
const STATISTICS_AUTO_REFRESH_MS = 30 * 60 * 1000;

/** 统计范围：官方窗口 30/90 天，或本地账本的「全部」。 */
const CREDIT_RANGE_OPTIONS: { key: CreditRange; label: string }[] = [
  { key: "30d", label: "近 30 天" },
  { key: "90d", label: "近 90 天" },
  { key: "all", label: "全部" },
];


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
  /** 客户端筛选（与档位正交）：只在明细上做，选具体值时四卡改「据明细汇总」。 */
  const [clientView, setClientView] = useState<CreditClientKey>("all");
  /** 范围：30 天 / 90 天（官方窗口）/ 全部（本地账本）。 */
  const [range, setRange] = useState<CreditRange>("30d");
  const [rangeLoading, setRangeLoading] = useState(false);

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
    range,
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

  /**
   * 切范围：30 天走共享缓存；90 天 / 全部单独取一次。
   * 「全部」不联网（后端读账本），所以断网也能切。
   */
  async function selectRange(next: CreditRange) {
    if (next === range || rangeLoading) return;
    setRange(next);
    setRangeLoading(true);
    try {
      if (next === "30d") {
        await load(false);
      } else {
        setStats(await api.getCreditStatistics(false, next));
        setError(null);
      }
    } catch (cause) {
      toast.error("切换统计范围失败", { description: api.asError(cause) });
    } finally {
      setRangeLoading(false);
    }
  }

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

  // 客户端维度只能在明细上做（官方没有按客户端聚合）。
  const clientOptions = useMemo(
    () =>
      range === "all"
        ? ledgerClientViewOptions(stats?.ledger)
        : clientViewOptions(stats?.officialUsage?.requests),
    [range, stats],
  );
  useEffect(() => {
    // 明细里没有该客户端（换账号、后端换了明细范围）→ 回到「全部」，不留一个空筛选项。
    if (clientView === "all") return;
    if (!clientOptions.some((option) => option.key === clientView)) setClientView("all");
  }, [clientOptions, clientView]);
  const clientDetail = useMemo(() => {
    if (clientView === "all" || !filteredOfficial) return null;
    return filterOfficialByClient(filteredOfficial, clientView, variantAccountIds);
  }, [clientView, filteredOfficial, variantAccountIds]);

  // 选「全部」走账本；账本不可用/为空时降级回官方 30 天并在页面上说明。
  const ledgerOfficial = useMemo(
    () => (range === "all" ? ledgerToOfficialUsage(stats?.ledger, clientView) : null),
    [range, stats, clientView],
  );
  const ledgerFallback = range === "all" && !ledgerOfficial;
  /** 当前口径的官方数据：全部=账本 / 具体客户端=据明细汇总 / 否则=官方聚合。 */
  const officialUsage = ledgerFallback
    ? filteredOfficial
    : ledgerOfficial ?? clientDetail?.official ?? filteredOfficial;

  /** 导出：当前档位 + 当前客户端筛选，全量日期；官方明细标注每账号上限与是否截断。 */
  async function exportCsv() {
    if (!stats) {
      toast.error("还没有可导出的统计数据");
      return;
    }
    try {
      const content = buildCreditStatsCsv(stats, officialUsage ?? undefined);
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

  /** 导出 Excel：与 CSV 同一份口径，多 sheet、数字可求和、每个 sheet 末行写说明。 */
  async function exportXlsx() {
    if (!stats) {
      toast.error("还没有可导出的统计数据");
      return;
    }
    try {
      const bytes = buildCreditStatsXlsx(stats, officialUsage ?? undefined, {
        range,
        ledger: stats.ledger,
        clientLabel: clientView === "all" ? undefined : CLIENT_VIEW_LABELS[clientView],
        caliberNote: ledgerCaliber ?? undefined,
      });
      const outcome = await saveXlsxFile(creditStatsXlsxFileName(viewVariant), bytes);
      if (outcome === "saved") {
        toast.success("已导出积分统计 Excel", {
          description: `${viewVariant} · ${(bytes.length / 1024).toFixed(0)} KB · 7 个 sheet`,
        });
      }
    } catch (cause) {
      toast.error("导出失败", { description: api.asError(cause) });
    }
  }

  const official = isOfficialUsageAvailable(officialUsage) ? officialUsage : undefined;

  /** 顶部口径说明：账本优先，其次明细汇总，最后官方/本地。 */
  const ledgerCaliber =
    range === "all"
      ? ledgerOfficial
        ? `本地账本：${stats?.ledger?.startAt ?? "—"} ~ ${stats?.ledger?.endAt ?? "—"} · ${
            stats?.ledger?.rows ?? 0
          } 条（自本工具首次抓取起累计）`
        : "本地账本暂无数据，暂用官方近 30 天口径"
      : null;
  const rangeFallbackText =
    stats?.rangeFallback === true
      ? `官方接口只接受近 ${stats?.windowProbe?.maxDays ?? 30} 天窗口，已按近 30 天统计`
      : null;
  const variantEmpty =
    viewVariant !== "all" && variantAccountIds !== null && variantAccountIds.size === 0;

  // 本地观察口径的覆盖面：官方不可用时用它解释「哪些窗口是完整的」。
  // 档位过滤后的 `coverage` 沿用全局快照区间（后端只下发一份），因此是最保守的下界：
  // 单档位账号的快照可能更晚，绝不会更早，不会把未覆盖说成已覆盖。
  const coverage = filteredStats?.coverage;
  const todayValue = filteredStats ? usageWindowValue("today", official, filteredStats, coverage) : null;
  const weekValue = filteredStats ? usageWindowValue("7d", official, filteredStats, coverage) : null;
  const monthValue = filteredStats ? usageWindowValue("month", official, filteredStats, coverage) : null;
  const coverageHint = weekValue?.absenceReason ?? monthValue?.absenceReason ?? null;
  const officialRangeText = official ? `${official.rangeStart} ~ ${official.rangeEnd}` : "";
  // 失败原因取**未过滤**的官方响应：档位过滤会按 accountId 收紧 `errors`，
  // 而归一化出来的「整页不可用」没有任何账号行，过滤后会把原因一起丢掉。
  const failureSource = stats?.officialUsage ?? officialUsage;
  const officialFailureText = (failureSource?.errors ?? [])
    .map((item) => `${item.accountName}: ${item.error}`)
    .join("；");

  // 明细已取到但总览仍回退本地时额外提示，避免「明细有数字、总览是另一个数」的自相矛盾。
  const detailHasData =
    !official &&
    (officialUsage?.requests.length ?? 0) > 0 &&
    (officialUsage?.accounts ?? []).some((account) => account.ok && account.requestCount > 0);

  return (
    <div className="mx-auto w-full max-w-[1180px] min-w-0 px-4 py-6 sm:px-8 sm:py-9">
      <header className="mb-10 flex min-w-0 flex-wrap items-start justify-between gap-4 sm:mb-12">
        <div className="min-w-0">
          <h1 className="text-[28px] font-semibold tracking-tight">积分统计</h1>
          <p className="mt-2 max-w-2xl text-sm leading-6 text-muted-foreground">
            当前数据更新于 {stats ? formatDateTime(official?.collectedAt ?? stats.generatedAt) : "—"}
          </p>
          <p className="mt-1 max-w-2xl text-[11px] leading-5 text-muted-foreground">
            积分按账号汇总（国内版与国际版分开计算）；「客户端」筛选在官方明细上做，
            选中具体客户端时总览改「据明细汇总」。官方用量接口只提供近 30 天，
            逐请求明细每账号上限 1000 条（聚合不受此限制）。
          </p>
          {stats ? (
            <p className="mt-0.5 max-w-2xl text-[11px] leading-5 text-muted-foreground">
            {ledgerCaliber ??
              (clientDetail
                ? `据明细汇总（${CLIENT_VIEW_LABELS[clientView]}）· ${clientDetail.records} 条 · ${
                    clientDetail.truncated ? "部分截断（数字为下限）" : "未截断"
                  }${rangeFallbackText ? ` · ${rangeFallbackText}` : ""}`
                : official
                  ? `官方口径：${officialRangeText} · 明细上限 ${official.detailLimitPerAccount} 条/账号${
                      rangeFallbackText ? ` · ${rangeFallbackText}` : ""
                    }`
                  : `本地观察口径：${localCoverageText(coverage)}`)}
          </p>
          ) : null}

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
            aria-label="统计范围"
          >
            {CREDIT_RANGE_OPTIONS.map((option) => {
              // 服务端只接受 31 天时禁止选 90 天；尚未探测（maxDays=null）先允许，后端会降级。
              const disabled =
                rangeLoading ||
                (option.key === "90d" &&
                  stats?.windowProbe?.maxDays != null &&
                  (stats.windowProbe.maxDays ?? 0) < 90);
              const title =
                option.key === "90d" && disabled
                  ? (stats?.windowProbe?.reason ??
                    "官方接口只接受近 31 天窗口，无法统计 90 天")
                  : option.key === "all"
                    ? "本地账本：从本工具首次抓取官方明细起累计"
                    : undefined;
              return (
                <button
                  key={option.key}
                  type="button"
                  role="tab"
                  aria-selected={range === option.key}
                  disabled={disabled}
                  title={title}
                  className={`cursor-pointer rounded-md px-2.5 py-1.5 text-xs transition-colors ${
                    range === option.key
                      ? "bg-background font-medium text-foreground shadow-sm"
                      : "text-muted-foreground hover:text-foreground"
                  } ${disabled ? "cursor-not-allowed opacity-50" : ""}`}
                  onClick={() => void selectRange(option.key)}
                >
                  {option.label}
                </button>
              );
            })}
          </div>
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
          <ClientFilterMenu
            options={clientOptions}
            value={clientView}
            onChange={setClientView}
            counts={stats?.officialUsage?.requests.length ?? 0}
          />
          <ExportMenu
            disabled={!stats}
            title={stats ? "导出当前范围 + 档位 + 客户端筛选" : "没有可导出的数据"}
            items={[
              {
                key: "csv",
                label: "CSV",
                description: "6 段明细，UTF-8 BOM",
                icon: <Download className="size-3.5" />,
                onSelect: () => void exportCsv(),
              },
              {
                key: "xlsx",
                label: "Excel（xlsx）",
                description: "7 个 sheet，数值可求和",
                icon: <FileSpreadsheet className="size-3.5" />,
                onSelect: () => void exportXlsx(),
              },
            ]}
          />
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
              {!official && (
                <Alert variant="warning">
                  <CircleAlert />
                  <AlertTitle>官方用量暂不可用，当前为本地观察数据</AlertTitle>
                  <AlertDescription className="flex flex-col gap-1">
                    <span>
                      本地观察覆盖：{localCoverageText(coverage)}。今日值为本地快照差值；
                      近 7 天/本月在本地覆盖不足时显示「—」，不用今日值代替。
                      失败原因已写入 error.log，官方接口恢复后点「刷新统计」即可重新同步。
                    </span>
                    {detailHasData && (
                      <span className="text-xs">
                        明细已取到部分官方请求，但总览暂用本地口径；明细与总览的差异来自这里。
                      </span>
                    )}
                    {officialFailureText && (
                      <span className="text-xs text-warning-foreground">{officialFailureText}</span>
                    )}
                  </AlertDescription>
                </Alert>
              )}

              {official && official.status === "partial" && (
                <Alert variant="warning">
                  <CircleAlert />
                  <AlertTitle>部分账号官方用量未同步（partial）</AlertTitle>
                  <AlertDescription className="flex flex-col gap-1">
                    <span>
                      已同步 {official.accounts.filter((account) => account.ok).length}/
                      {official.accounts.length} 个当前账号；失败账号的官方数值显示为「—」，
                      总览为已同步账号之和，不代表全部账号。
                    </span>
                    {officialFailureText && (
                      <span className="text-xs text-warning-foreground">{officialFailureText}</span>
                    )}
                  </AlertDescription>
                </Alert>
              )}

              <Card className="min-w-0 gap-0 overflow-hidden rounded-2xl bg-card/70 py-0 shadow-none" aria-label="积分总览">
                <div className="flex min-w-0 flex-wrap items-center justify-between gap-x-3 gap-y-1 border-b border-border/60 px-4 py-2 text-[11px] leading-5 text-muted-foreground sm:px-5">
                  <span className="min-w-0">
                    {range === "all" && ledgerOfficial
                      ? `本地账本 · ${stats?.ledger?.rows ?? 0} 条 · ${
                          ledgerFallback ? "降级" : "累计"
                        }（剩余积分为账号级）`
                      : clientDetail
                        ? `据明细汇总 · ${clientDetail.records} 条 · ${
                            clientDetail.truncated ? "部分截断" : "未截断"
                          }（剩余积分为账号级）`
                        : `${creditSourceBadge(official)}：${
                            official
                              ? `${officialRangeText}（官方仅提供近 30 天）`
                              : localCoverageText(coverage)
                          }`}
                  </span>
                  <span className="min-w-0">
                    {coverageHint ?? "明细每账号上限 1000 条，聚合与趋势不受限"}
                  </span>
                </div>
                <CardContent className="grid min-w-0 grid-cols-1 divide-y divide-border/60 p-0 sm:grid-cols-4 sm:divide-y-0 sm:py-5">
                  <StatMetric
                    icon={Sparkles}
                    label="剩余积分"
                    value={formatAmount(filteredStats.summary.currentRemaining)}
                  />
                  <StatMetric
                    icon={TrendingDown}
                    label="今日消耗"
                    value={todayValue?.text ?? "—"}
                    divided
                  />
                  <StatMetric
                    icon={CalendarDays}
                    label="近 7 天消耗"
                    value={weekValue?.text ?? "—"}
                    divided
                  />
                  <StatMetric
                    icon={CalendarRange}
                    label="本月消耗"
                    value={monthValue?.text ?? "—"}
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
