import { CircleAlert, Loader2 } from "lucide-react";
import { creditResourceName } from "@/lib/credit-package-names";
import { formatAmount, formatDateTime, formatFullDateTime, pad2 } from "@/lib/format";
import { cn } from "@/lib/utils";
import type {
  CreditExpiry,
  CreditResource,
} from "@/lib/types";

import { expiryClass } from "@/components/account-card/formatters";

/** 到期日 `MM/DD`（完整时刻放 `title`）：紧凑表格里年份没有信息量，月份日期足够排序判断。 */
function shortExpiry(expireAt: number | null): string {
  if (!expireAt) return "长期";
  const date = new Date(expireAt);
  if (Number.isNaN(date.getTime())) return "长期";
  return `${pad2(date.getMonth() + 1)}/${pad2(date.getDate())}`;
}

/** 未使用（剩余 === 总量）时的中性色：灰 = 还没动过，不是「已用完」。 */
const UNUSED_BAR = "bg-muted-foreground/35";

/** 迷你条的配色：过期红 / 临期橙（已登记的例外）> 未使用灰 > 用过的主题蓝。 */
function barTone(resource: CreditResource): string {
  if (resource.expired) return "bg-red-600/60 dark:bg-red-400/60";
  if (resource.expiringSoon) return "bg-orange-500/80";
  if (resource.used <= 0) return UNUSED_BAR;
  return "bg-[var(--data-series-blue)]";
}

export function ResourceBreakdown({ credit, loading }: { credit?: CreditExpiry; loading?: boolean }) {
  if (loading) {
    return (
      <div className="flex items-center gap-2 px-4 py-8 text-sm text-muted-foreground sm:px-5">
        <Loader2 className="size-4 animate-spin" />
        正在加载资源包…
      </div>
    );
  }
  if (!credit) {
    return <div className="px-4 py-8 text-center text-sm text-muted-foreground sm:px-5">尚未采集当前资源包。</div>;
  }
  if (!credit.ok) {
    return (
      <div className="flex items-start gap-2 px-4 py-8 text-sm text-destructive sm:px-5">
        <CircleAlert className="mt-0.5 size-4 shrink-0" />
        <span>{credit.error || "积分资源查询失败"}</span>
      </div>
    );
  }
  const allResources = credit.resources ?? [];
  // 已用完的资源包即使还没到期也不再占用明细列表
  const resources = allResources.filter((resource) => resource.remaining > 0);
  if (allResources.length === 0) {
    return <div className="px-4 py-8 text-center text-sm text-muted-foreground sm:px-5">当前没有可展示的资源包。</div>;
  }
  if (resources.length === 0) {
    return <div className="px-4 py-8 text-center text-sm text-muted-foreground sm:px-5">当前没有可用积分，已用完的资源包不展示。</div>;
  }
  const totalRemaining = resources.reduce((sum, resource) => sum + resource.remaining, 0);

  return (
    <div className="min-w-0">
      {/* 表头即结论：共几个包、还剩多少、什么时候采的 */}
      <div className="flex min-w-0 flex-wrap items-center justify-between gap-x-3 gap-y-1 border-b border-border/60 px-4 py-2 text-[11px] leading-5 text-muted-foreground sm:px-5">
        <span className="min-w-0">
          共 {resources.length} 个资源包 · 合计剩余{" "}
          <span className="font-medium tabular-nums text-foreground">{formatAmount(totalRemaining)}</span>
        </span>
        <span className="min-w-0 tabular-nums">
          {credit.updatedAt ? `最近采集 ${formatDateTime(credit.updatedAt)}` : "最近采集 —"}
        </span>
      </div>
      {/* 列宽固定：名称自适应，其余四列定宽；行高 34px 换一屏能看 18 行以上 */}
      <div className="grid grid-cols-[minmax(0,1fr)_52px_64px_104px_64px] items-center gap-3 border-b border-border/40 px-4 py-1 text-[10px] font-medium uppercase tracking-wide text-muted-foreground sm:px-5">
        <span>资源包</span>
        <span className="text-right">到期</span>
        <span className="text-right">已用</span>
        <span className="text-right">剩余 / 总量</span>
        <span className="text-right">占比</span>
      </div>
      <div className="divide-y divide-border/60">
        {resources.map((resource, index) => {
          const ratio =
            resource.total > 0
              ? Math.min(100, Math.max(0, (resource.remaining / resource.total) * 100))
              : 0;
          const unused = resource.used <= 0;
          const name = creditResourceName(resource, "未命名资源包");
          return (
            <div
              key={`${resource.packageCode || resource.packageName || "resource"}-${index}`}
              className="grid h-[34px] min-w-0 grid-cols-[minmax(0,1fr)_52px_64px_104px_64px] items-center gap-3 px-4 text-xs sm:px-5"
            >
              <span className="min-w-0 truncate font-medium" title={name}>
                {name}
              </span>
              <span
                className={cn(
                  "text-right tabular-nums",
                  expiryClass(resource.expired, resource.expiringSoon),
                )}
                title={resource.expired ? "已到期" : formatFullDateTime(resource.expireAt)}
              >
                {shortExpiry(resource.expireAt)}
              </span>
              <span className="text-right tabular-nums text-muted-foreground">
                {unused ? "—" : formatAmount(resource.used)}
              </span>
              <span className="min-w-0 text-right tabular-nums">
                <span
                  className={cn(
                    "text-[15px] font-semibold leading-none",
                    unused ? "text-muted-foreground" : "text-foreground",
                  )}
                >
                  {formatAmount(resource.remaining)}
                </span>
                <span className="ml-1 text-[11px] font-normal text-muted-foreground">
                  / {formatAmount(resource.total)}
                </span>
              </span>
              <span
                className="ml-auto h-1.5 w-16 overflow-hidden rounded-full bg-muted"
                aria-hidden="true"
                title={`${ratio.toFixed(0)}%`}
              >
                <span
                  className={cn("block h-full rounded-full", barTone(resource))}
                  style={{ width: `${ratio}%` }}
                />
              </span>
            </div>
          );
        })}
      </div>
    </div>
  );
}
