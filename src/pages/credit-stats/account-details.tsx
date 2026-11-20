import { useState } from "react";
import { Card, CardDescription, CardHeader } from "@/components/ui/card";
import { formatDateTime } from "@/lib/format";
import type {
  CreditExpiry,
  CreditOfficialUsage,
  CreditStatistics,
} from "@/lib/types";

import { AccountFilterMenu } from "./account-filter-menu";
import { isOfficialUsageAvailable } from "./data";
import { OfficialUsageBreakdown } from "./official-usage";
import { ResourcesByAccount } from "./resources-by-account";

export function SelectedAccountDetails({
  stats,
  officialUsage,
  creditMap,
  creditLoadingMap,
}: {
  stats: CreditStatistics;
  officialUsage?: CreditOfficialUsage;
  creditMap: Record<string, CreditExpiry>;
  creditLoadingMap: Record<string, boolean>;
}) {
  const [detailTab, setDetailTab] = useState<"credits" | "requests">("credits");
  const official = isOfficialUsageAvailable(officialUsage) ? officialUsage : undefined;
  const filterAccounts = official ? official.accounts : stats.accounts;
  /** 本卡片仅允许选择单个账号，默认第一个账号 */
  const [accountFilter, setAccountFilter] = useState<string | null>(
    () => filterAccounts[0]?.accountId ?? null,
  );
  const activeFilterAccount =
    accountFilter && filterAccounts.some((account) => account.accountId === accountFilter)
      ? filterAccounts.find((account) => account.accountId === accountFilter)
      : undefined;
  // 筛选失效（如账号列表刷新变化）时回退到第一个账号
  const effectiveFilter = activeFilterAccount?.accountId ?? filterAccounts[0]?.accountId ?? null;
  const visibleAccounts = effectiveFilter
    ? stats.accounts.filter((account) => account.accountId === effectiveFilter)
    : stats.accounts;
  // 最近事件卡片已隐藏，events 不再使用。恢复时取消本注释。
  // const events = (effectiveFilter
  //   ? stats.events.filter((event) => event.accountId === effectiveFilter)
  //   : stats.events
  // ).slice(0, 50);
  const latestSnapshotAt = visibleAccounts.reduce<number | null>((latest, account) => {
    if (account.lastSnapshotAt == null) return latest;
    if (latest == null || account.lastSnapshotAt > latest) return account.lastSnapshotAt;
    return latest;
  }, null);

  // R5-15：不随账号切换重置 detailTab。切号后保持当前子 Tab，
  // 无数据 / 查询失败由 OfficialUsageBreakdown 自身的空态与错误态覆盖，
  // 分页仍由该组件在 accountId 变化时回到第 1 页。
  return (
    <div className="flex min-w-0 flex-col gap-12">
      <section className="min-w-0 space-y-2.5" aria-labelledby="credit-detail-title">
        <div className="px-1">
          <h2 id="credit-detail-title" className="text-[13px] font-medium leading-5">积分明细</h2>
        </div>
        <Card className="min-w-0 gap-0 overflow-hidden rounded-xl py-0 shadow-none">
          <CardHeader className="gap-0 border-b px-4 pt-3 pb-3 sm:px-5">
            <div className="flex min-w-0 flex-wrap items-center justify-between gap-3">
              <CardDescription className="min-w-0 truncate text-xs">
                {latestSnapshotAt ? `最近采集 ${formatDateTime(latestSnapshotAt)}` : "暂无账号资源包。"}
              </CardDescription>
              <AccountFilterMenu
                accounts={filterAccounts}
                accountFilter={effectiveFilter}
                onAccountFilterChange={setAccountFilter}
                ariaLabel="按账号筛选积分明细"
                allowAll={false}
              />
            </div>
            <div className="mt-3 flex max-w-full gap-1 rounded-lg bg-muted p-1" role="tablist" aria-label="积分详情类型">
              {(
                [
                  ["credits", "积分明细"],
                  ["requests", "请求用量"],
                ] as const
              ).map(([value, label]) => (
                <button
                  key={value}
                  type="button"
                  role="tab"
                  aria-selected={detailTab === value}
                  className={`min-w-0 flex-1 rounded-md px-2.5 py-1.5 text-xs transition-colors ${
                    detailTab === value
                      ? "bg-background font-medium text-foreground shadow-sm"
                      : "text-muted-foreground hover:text-foreground"
                  }`}
                  onClick={() => setDetailTab(value)}
                >
                  {label}
                </button>
              ))}
            </div>
          </CardHeader>
          {detailTab === "credits" ? (
            <div className="min-w-0">
              <ResourcesByAccount accounts={visibleAccounts} creditMap={creditMap} creditLoadingMap={creditLoadingMap} />
            </div>
          ) : (
            <OfficialUsageBreakdown officialUsage={officialUsage} accountId={effectiveFilter} />
          )}
        </Card>
      </section>
    </div>
  );
}
