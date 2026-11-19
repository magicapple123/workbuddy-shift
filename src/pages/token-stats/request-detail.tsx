import { useState } from "react";
import { ChevronLeft, ChevronRight } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import type {
  TokenStatsRequestRow,
  TokenStatsSource,
} from "@/lib/types";

import { REQUEST_PAGE_SIZE, exact, formatRequestTime, shortAccountLabel } from "./data";
import { RequestUsageCell } from "./usage-detail";

/**
 * 明细表格与分页。页状态挂在 `DialogContent` 的子组件里：Radix 关闭即卸载，
 * 因此重新打开弹框会自动回到第 1 页，不需要额外的重置逻辑。
 */
function RequestDetailRows({ rows, records }: { rows: TokenStatsRequestRow[]; records: number }) {
  const [page, setPage] = useState(0);
  const pageCount = Math.max(1, Math.ceil(rows.length / REQUEST_PAGE_SIZE));
  const start = page * REQUEST_PAGE_SIZE;
  const pageRows = rows.slice(start, start + REQUEST_PAGE_SIZE);
  const shownEnd = start + pageRows.length;

  return (
    <>
      {/* 两个方向都由本容器滚动，表头才能 sticky：Table 自带的横向滚动 wrapper
          会让 overflow-y 的计算值变成 auto，吃掉 sticky 的参照系（见 table.tsx）。 */}
      <div className="min-h-0 flex-1 overflow-auto rounded-lg border">
        <Table containerClassName="overflow-visible" className="min-w-[720px]">
          <TableHeader className="sticky top-0 z-10 bg-background [&_th]:bg-background">
            <TableRow className="hover:bg-transparent">
              <TableHead>时间</TableHead>
              <TableHead>模型</TableHead>
              <TableHead>会话</TableHead>
              <TableHead>项目</TableHead>
              <TableHead>账号</TableHead>
              <TableHead className="w-[150px] text-right">用量</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {pageRows.map((row, index) => {
              const sessionTitle = row.title?.trim();
              const sessionLabel = sessionTitle || row.sessionId.slice(0, 8);
              return (
                <TableRow key={`${row.timestamp}-${row.sessionId}-${start + index}`}>
                  <TableCell className="tabular-nums text-muted-foreground">
                    {formatRequestTime(row.timestamp)}
                  </TableCell>
                  <TableCell className="max-w-[200px] truncate" title={row.model}>
                    {row.model}
                  </TableCell>
                  <TableCell
                    className="max-w-[220px] truncate"
                    title={sessionTitle || row.sessionId}
                  >
                    {sessionLabel}
                  </TableCell>
                  <TableCell className="max-w-[160px] truncate" title={row.project}>
                    {row.project}
                  </TableCell>
                  <TableCell className="max-w-[120px] truncate text-muted-foreground" title={row.uid ?? ""}>
                    {shortAccountLabel(row.uid) ?? "—"}
                  </TableCell>
                  <RequestUsageCell row={row} />
                </TableRow>
              );
            })}
          </TableBody>
        </Table>
      </div>
      <div className="flex flex-wrap items-center justify-between gap-2">
        <span className="text-[11px] text-muted-foreground">
          {rows.length < records
            ? `仅展示最近 ${exact.format(rows.length)} 条（共 ${exact.format(records)} 次调用）`
            : `共 ${exact.format(rows.length)} 次调用`}
        </span>
        <div className="flex items-center gap-2">
          <Button
            variant="outline"
            size="sm"
            onClick={() => setPage((value) => Math.max(0, value - 1))}
            disabled={page === 0}
          >
            <ChevronLeft />
            上一页
          </Button>
          <span className="text-[11px] text-muted-foreground tabular-nums">
            第 {exact.format(start + 1)}–{exact.format(shownEnd)} 条 · 共 {exact.format(rows.length)} 条
          </span>
          <Button
            variant="outline"
            size="sm"
            onClick={() => setPage((value) => Math.min(pageCount - 1, value + 1))}
            disabled={page >= pageCount - 1}
          >
            下一页
            <ChevronRight />
          </Button>
        </div>
      </div>
    </>
  );
}

export function RequestDetailDialog({
  open,
  onOpenChange,
  source,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  source: TokenStatsSource;
}) {
  const rows = source.requests ?? [];

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="flex max-h-[85vh] min-w-0 flex-col gap-3 sm:max-w-5xl">
        <DialogHeader>
          <DialogTitle>请求明细</DialogTitle>
          <DialogDescription>
            每次模型调用一行，按时间倒序展示本地 CodeBuddy CLI 日志记录（仅 CLI 提供逐请求明细，
            其它来源只有聚合）。
          </DialogDescription>
        </DialogHeader>
        {rows.length === 0 ? (
          <div className="rounded-lg border border-dashed px-4 py-10 text-center text-sm text-muted-foreground">
            该来源暂无可展示的请求明细。
          </div>
        ) : (
          <RequestDetailRows rows={rows} records={source.summary.records} />
        )}
      </DialogContent>
    </Dialog>
  );
}
