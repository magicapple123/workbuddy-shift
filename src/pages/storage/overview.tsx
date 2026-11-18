import { HardDrive, MessagesSquare, RotateCcw, ServerCog } from "lucide-react";

import { Badge } from "@/components/ui/badge";
import type { StorageScan } from "@/lib/types";

import { formatBytes, formatCount, formatTime } from "./format";

interface OverviewProps {
  scan: StorageScan;
}

/** 总览：三个根 + 会话资产 + 回收站，数字全部来自同一次扫描。 */
export function StorageOverview({ scan }: OverviewProps) {
  const cards = [
    {
      icon: ServerCog,
      label: "工具数据",
      value: scan.roots.find((root) => root.id === "store")?.bytes ?? 0,
      detail: formatCount(scan.roots.find((root) => root.id === "store")?.files ?? 0),
    },
    {
      icon: HardDrive,
      label: "客户端数据",
      value: scan.roots.find((root) => root.id === "cn")?.bytes ?? 0,
      detail: formatCount(scan.roots.find((root) => root.id === "cn")?.files ?? 0),
    },
    {
      icon: MessagesSquare,
      label: "会话资产包",
      value: scan.sessions.bytes,
      detail: `${scan.sessions.counts.live} 存活 / ${scan.sessions.counts.softDeleted} 软删除 / ${scan.sessions.counts.orphan} 孤儿`,
    },
    {
      icon: RotateCcw,
      label: "回收站",
      value: scan.trash.bytes,
      detail: `${scan.trash.entries} 个条目 · ${formatCount(scan.trash.files)}`,
    },
  ];

  return (
    <div className="space-y-4">
      <div className="grid gap-3 sm:grid-cols-2 xl:grid-cols-4">
        {cards.map((card) => (
          <div key={card.label} className="rounded-xl border border-border/60 bg-card p-4">
            <div className="flex items-center gap-2 text-xs font-medium text-muted-foreground">
              <card.icon className="size-3.5" />
              {card.label}
            </div>
            <div className="mt-2 text-xl font-semibold tabular-nums">{formatBytes(card.value)}</div>
            <div className="mt-1 text-[11px] text-muted-foreground">{card.detail}</div>
          </div>
        ))}
      </div>

      <div className="flex flex-wrap items-center gap-x-4 gap-y-2 text-xs text-muted-foreground">
        <span>
          共 {formatBytes(scan.totals.bytes)} / {formatCount(scan.totals.files)}
        </span>
        <span>扫描于 {formatTime(scan.generatedAt)}</span>
        {scan.cached ? <Badge variant="secondary">60 秒缓存</Badge> : null}
        <Badge
          variant="secondary"
          className={
            scan.clientRunning
              ? "border-warning-border bg-warning-soft text-warning"
              : "border-success-border bg-success-soft text-success"
          }
        >
          {scan.clientRunning ? "客户端运行中：会话清理已锁定" : "客户端已退出：可清理会话"}
        </Badge>
      </div>
    </div>
  );
}
