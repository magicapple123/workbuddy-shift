import { useEffect, useState } from "react";
import { ArrowUpDown, ChevronLeft, ChevronRight, CircleAlert } from "lucide-react";
import { Button } from "@/components/ui/button";
import { formatAmount } from "@/lib/format";
import type {
  CreditOfficialUsage,
  CreditOfficialUsageRequest,
} from "@/lib/types";

import { RequestSortKey, SortDirection, compareRequests, isOfficialUsageAvailable, officialAccountFor } from "./data";

function OfficialRequestRow({
  request,
  showAccount,
}: {
  request: CreditOfficialUsageRequest;
  showAccount: boolean;
}) {
  return (
    <tr className="border-t border-border/60 align-top">
      <td className="whitespace-nowrap px-3 py-3 text-muted-foreground">{request.requestTime}</td>
      {showAccount && (
        <td className="max-w-[140px] truncate px-3 py-3" title={request.accountName}>
          {request.accountName}
        </td>
      )}
      <td className="whitespace-nowrap px-3 py-3 text-right font-medium text-primary">
        {formatAmount(request.credit)}
      </td>
      <td className="max-w-[180px] truncate px-3 py-3" title={request.model}>
        {request.model}
      </td>
      <td className="max-w-[120px] truncate px-3 py-3 text-muted-foreground" title={request.client}>
        {request.client}
      </td>
      <td className="max-w-[170px] truncate px-3 py-3 font-mono text-[10px] text-muted-foreground" title={request.requestId}>
        {request.requestId}
      </td>
    </tr>
  );
}

/** 请求用量明细每页条数；明细由后端一次性返回，分页只控制单页渲染量 */
const REQUEST_PAGE_SIZE = 100;

function SortableHeader({
  label,
  sortKey,
  activeKey,
  direction,
  onSort,
  align = "left",
}: {
  label: string;
  sortKey: RequestSortKey;
  activeKey: RequestSortKey;
  direction: SortDirection;
  onSort: (key: RequestSortKey) => void;
  align?: "left" | "right";
}) {
  const active = activeKey === sortKey;
  return (
    <th
      className={`px-3 py-2.5 font-medium ${align === "right" ? "text-right" : ""}`}
      aria-sort={active ? (direction === "desc" ? "descending" : "ascending") : "none"}
    >
      <button
        type="button"
        className={`inline-flex cursor-pointer items-center gap-1 transition-colors hover:text-foreground ${
          active ? "text-foreground" : ""
        } ${align === "right" ? "flex-row-reverse" : ""}`}
        onClick={() => onSort(sortKey)}
        title={active ? (direction === "desc" ? "当前降序，点击切换升序" : "当前升序，点击切换降序") : "点击排序"}
      >
        {label}
        <ArrowUpDown className={`size-3 shrink-0 ${active ? "opacity-100" : "opacity-40"}`} />
      </button>
    </th>
  );
}

export function OfficialUsageBreakdown({
  officialUsage,
  accountId,
}: {
  officialUsage?: CreditOfficialUsage;
  accountId: string | null;
}) {
  const [sortKey, setSortKey] = useState<RequestSortKey>("time");
  const [sortDirection, setSortDirection] = useState<SortDirection>("desc");
  const [page, setPage] = useState(0);
  const officialAvailable = isOfficialUsageAvailable(officialUsage);

  // 换账号或换排序都回到第一页；重新采集后数据整体更换，同样重置
  useEffect(() => {
    setPage(0);
  }, [accountId, sortKey, sortDirection, officialUsage?.collectedAt]);

  const toggleSort = (key: RequestSortKey) => {
    if (key === sortKey) {
      setSortDirection((current) => (current === "desc" ? "asc" : "desc"));
      return;
    }
    setSortKey(key);
    // 新列默认从「最大」开始看：消耗列先看最高消耗，时间列看最近请求
    setSortDirection("desc");
  };
  const account = accountId ? officialAccountFor(officialUsage, accountId) : undefined;

  if (!officialAvailable || !officialUsage) {
    return (
      <div className="flex items-start gap-2 px-4 py-8 text-sm text-muted-foreground sm:px-5">
        <CircleAlert className="mt-0.5 size-4 shrink-0" />
        <span>官方请求用量暂不可用；总览已回退到本地观察数据，请稍后刷新重试。</span>
      </div>
    );
  }

  if (accountId && !account) {
    return <div className="px-4 py-8 text-center text-sm text-muted-foreground sm:px-5">该账号暂无官方用量记录。</div>;
  }

  if (account && !account.ok) {
    return (
      <div className="flex items-start gap-2 px-4 py-8 text-sm text-danger sm:px-5">
        <CircleAlert className="mt-0.5 size-4 shrink-0" />
        <span>{account.error || "该账号的官方请求用量查询失败"}</span>
      </div>
    );
  }

  const requests = account
    ? officialUsage.requests.filter((request) => request.accountId === account.accountId)
    : officialUsage.requests;
  const totalRequests = account
    ? (account.reportedTotal ?? account.requestCount)
    : officialUsage.accounts.reduce((sum, item) => sum + (item.reportedTotal ?? item.requestCount), 0);
  const detailTruncated = account
    ? account.detailTruncated
    : officialUsage.accounts.some((item) => item.detailTruncated);
  const showAccount = !account;

  const sortedRequests = [...requests].sort((left, right) =>
    compareRequests(left, right, sortKey, sortDirection),
  );
  const pageCount = Math.max(1, Math.ceil(sortedRequests.length / REQUEST_PAGE_SIZE));
  const safePage = Math.min(Math.max(page, 0), pageCount - 1);
  const pageStart = safePage * REQUEST_PAGE_SIZE;
  const pageRows = sortedRequests.slice(pageStart, pageStart + REQUEST_PAGE_SIZE);

  return (
    <div className="min-w-0">
      {detailTruncated && (
        <div className="flex items-start gap-2 border-b bg-warning-soft px-4 py-2.5 text-xs text-warning-foreground sm:px-5">
          <CircleAlert className="mt-0.5 size-3.5 shrink-0" />
          <span>
            最多展示每账号最近 {officialUsage.detailLimitPerAccount} 条请求明细；合计使用官方返回的全部 {formatAmount(totalRequests)} 条请求。
          </span>
        </div>
      )}
      {requests.length === 0 ? (
        <div className="px-4 py-8 text-center text-sm text-muted-foreground sm:px-5">
          {totalRequests > 0 ? "官方返回了请求总数，但明细未通过格式校验。" : "官方暂无请求用量。"}
        </div>
      ) : (
        <div className="min-w-0 overflow-x-auto">
          <table className="w-full min-w-[700px] text-left text-[11px]">
            <thead className="sticky top-0 bg-muted/95 text-muted-foreground">
              <tr>
                <SortableHeader
                  label="请求时间"
                  sortKey="time"
                  activeKey={sortKey}
                  direction={sortDirection}
                  onSort={toggleSort}
                />
                {showAccount && <th className="px-3 py-2.5 font-medium">账号</th>}
                <SortableHeader
                  label="消耗"
                  sortKey="credit"
                  activeKey={sortKey}
                  direction={sortDirection}
                  onSort={toggleSort}
                  align="right"
                />
                <th className="px-3 py-2.5 font-medium">模型</th>
                <th className="px-3 py-2.5 font-medium">客户端</th>
                <th className="px-3 py-2.5 font-medium">请求 ID</th>
              </tr>
            </thead>
            <tbody>
              {pageRows.map((request) => (
                <OfficialRequestRow
                  key={`${request.requestId}-${request.requestTime}`}
                  request={request}
                  showAccount={showAccount}
                />
              ))}
            </tbody>
          </table>
          {sortedRequests.length > REQUEST_PAGE_SIZE && (
            <div className="flex flex-wrap items-center justify-between gap-2 border-t px-3 py-2.5 text-xs text-muted-foreground sm:px-4">
              <span>
                第 {pageStart + 1}–{Math.min(pageStart + REQUEST_PAGE_SIZE, sortedRequests.length)} 条，共{" "}
                {sortedRequests.length} 条
              </span>
              <div className="flex items-center gap-1.5">
                <Button
                  variant="ghost"
                  size="sm"
                  className="h-7 px-2 text-xs"
                  disabled={safePage === 0}
                  onClick={() => setPage(safePage - 1)}
                >
                  <ChevronLeft className="size-3.5" />
                  上一页
                </Button>
                <span className="tabular-nums">
                  {safePage + 1} / {pageCount}
                </span>
                <Button
                  variant="ghost"
                  size="sm"
                  className="h-7 px-2 text-xs"
                  disabled={safePage >= pageCount - 1}
                  onClick={() => setPage(safePage + 1)}
                >
                  下一页
                  <ChevronRight className="size-3.5" />
                </Button>
              </div>
            </div>
          )}
        </div>
      )}
    </div>
  );
}
