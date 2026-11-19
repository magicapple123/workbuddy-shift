import { useEffect, useMemo, useState } from "react";
import { CircleAlert, Download, FileSpreadsheet, ListTree, Loader2, RefreshCw } from "lucide-react";
import { toast } from "sonner";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { DemoAction } from "@/components/demo-action";
import { CodeBuddyCnIdeMark, CodeBuddyMark, WorkBuddyAiMark, WorkBuddyMark } from "@/components/product-marks";
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { Skeleton } from "@/components/ui/skeleton";
import * as api from "@/lib/api";
import { formatDate, formatDateTime } from "@/lib/format";
import {
  isTokenStatisticsCacheFresh,
  loadTokenStatistics,
  readTokenStatisticsCache,
} from "@/lib/stats-cache";
import type {
  TokenStatistics,
} from "@/lib/types";
import { useAccountsStore } from "@/stores/accounts";

import { saveCsvFile, saveXlsxFile } from "@/lib/export-download";
import {
  type AccountSelection,
  accountOptions,
  cliAccountsUnsupported,
  filterSourceByAccount,
  hasAccountDimension,
  parseSelection,
  persistAccountSelection,
  readAccountSelection,
  selectionLabel,
  serializeSelection,
} from "./accounts";
import { TokenAccountSelect } from "./account-select";
import { Dashboard } from "./dashboard";
import { buildTokenStatsCsv, buildTokenStatsXlsx, tokenStatsFileName, tokenStatsXlsxFileName } from "./export";
import { SourceKey, isSourceKey, persistPreferredTokenSource, readPreferredTokenSource } from "./data";
import { RequestDetailDialog } from "./request-detail";
import { TokenStatsLoadingSkeleton } from "./skeleton";

const SOURCE_KEYS: SourceKey[] = ["workbuddy", "workbuddy-ai", "codebuddy-cli", "codebuddy-ide"];

/** 每个来源记住上次选择的账号（第 7 条：筛选层级按来源独立）。 */
function readAllSelections(): Record<SourceKey, AccountSelection> {
  return SOURCE_KEYS.reduce(
    (all, key) => {
      all[key] = readAccountSelection(key);
      return all;
    },
    {} as Record<SourceKey, AccountSelection>,
  );
}


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

  const accounts = useAccountsStore((state) => state.accounts);
  const fetchAccounts = useAccountsStore((state) => state.fetchAll);
  const [selections, setSelections] = useState<Record<SourceKey, AccountSelection>>(readAllSelections);

  useEffect(() => {
    // 账号下拉的选项来自账号库：没有账号时补拉一次（与积分页同一数据源）。
    if (accounts.length === 0) void fetchAccounts();
  }, [accounts.length, fetchAccounts]);

  const source = stats?.sources.find((item) => item.source === active);
  const options = useMemo(
    () => accountOptions(active, accounts, source?.accounts),
    [active, accounts, source],
  );
  // 保存的 uid 不在当前列表（账号被删、换机器、旧后端没有账号维度）时回退「所有账号」。
  const selection = useMemo<AccountSelection>(() => {
    const current = selections[active];
    if (!current || current.kind === "all") return { kind: "all" };
    const value = serializeSelection(current);
    return options.some((option) => option.value === value) ? current : { kind: "all" };
  }, [selections, active, options]);
  const selectionValue = serializeSelection(selection);
  const filteredSource = useMemo(
    () => (source ? filterSourceByAccount(source, selection) : undefined),
    [source, selection],
  );
  const scopeLabel = selectionLabel(selection, accounts);
  const accountNotice = cliAccountsUnsupported(active)
    ? "CLI 日志不记录账号，只能按「未归属」统计；IDE 来源按数据目录 uid 归因。"
    : null;

  /** 导出当前来源 + 当前账号：范围是全量日期，图表时间范围不参与。 */
  async function exportCsv() {
    if (!filteredSource) {
      toast.error("当前数据源没有可导出的数据");
      return;
    }
    try {
      const content = buildTokenStatsCsv(filteredSource);
      const outcome = await saveCsvFile(
        tokenStatsFileName(filteredSource.source, selection.kind === "all" ? undefined : scopeLabel),
        content,
      );
      if (outcome === "saved") {
        toast.success("已导出 Token 统计 CSV", {
          description: `${filteredSource.source} · ${scopeLabel} · ${(content.length / 1024).toFixed(0)} KB`,
        });
      }
    } catch (cause) {
      toast.error("导出失败", { description: api.asError(cause) });
    }
  }



  /** 导出 Excel（xlsx）：同样按「当前来源 + 当前账号」，7 个 sheet 各一段。 */
  async function exportXlsx() {
    if (!filteredSource) {
      toast.error("当前数据源没有可导出的数据");
      return;
    }
    try {
      const bytes = buildTokenStatsXlsx(filteredSource);
      const outcome = await saveXlsxFile(
        tokenStatsXlsxFileName(filteredSource.source, selection.kind === "all" ? undefined : scopeLabel),
        bytes,
      );
      if (outcome === "saved") {
        toast.success("已导出 Token 统计 Excel", {
          description: `${filteredSource.source} · ${scopeLabel} · ${(bytes.length / 1024).toFixed(0)} KB`,
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
              {source ? (
                <p className="mt-1 max-w-2xl text-[11px] leading-5 text-muted-foreground">
                  本地日志口径（不是官方账单）：覆盖 {formatDate(source.coverageStartAt)} ~{" "}
                  {formatDate(source.coverageEndAt)} · 账号：{scopeLabel} · 逐请求明细仅 CodeBuddy CLI 提供
                </p>
              ) : null}
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
              disabled={!filteredSource}
              title={filteredSource ? "导出当前数据源 + 当前账号的 CSV" : "没有可导出的数据"}
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
              onClick={() => void exportXlsx()}
              disabled={!filteredSource}
              title={filteredSource ? "导出当前数据源 + 当前账号的 Excel（xlsx）" : "没有可导出的数据"}
            >
              <FileSpreadsheet />
              导出 Excel
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
        <div className="mb-8 flex min-w-0 flex-wrap items-center justify-between gap-3">
          <Tabs
            className="min-w-0 gap-0"
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
          {hasAccountDimension(source) ? (
            <TokenAccountSelect
              options={options}
              value={selectionValue}
              notice={accountNotice}
              onChange={(value) => {
                const next = parseSelection(value);
                if (!next) return;
                setSelections((previous) => ({ ...previous, [active]: next }));
                persistAccountSelection(active, next);
              }}
            />
          ) : null}
        </div>
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

      {loading && !stats ? (
        <TokenStatsLoadingSkeleton />
      ) : filteredSource ? (
        <Dashboard source={filteredSource} scopeLabel={scopeLabel} />
      ) : (
        !error && (
          <div className="rounded-xl border border-dashed px-4 py-16 text-center text-sm text-muted-foreground">
            该来源暂无可用统计数据，请点击刷新重试。
          </div>
        )
      )}

      {filteredSource && (
        <RequestDetailDialog
          open={detailOpen}
          onOpenChange={setDetailOpen}
          source={filteredSource}
        />
      )}
    </div>
  );
}
