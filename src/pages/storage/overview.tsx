import { HardDrive, MessagesSquare, RotateCcw, ServerCog } from "lucide-react";

import { Badge } from "@/components/ui/badge";
import type { StorageScan } from "@/lib/types";

import { formatBytes, formatCount, formatTime } from "./format";

interface OverviewProps {
  scan: StorageScan;
}

/**
 * 总览：工具数据 + 客户端数据（国内 + 国际合计）+ 会话资产 + 回收站。
 *
 * 口径（第三轮第 8 条 8.3-2 / 8.3-4）：
 * - 「客户端数据」是 `~/.workbuddy` + `~/.workbuddy-ai` 的合计，不再只读国内版；
 * - 四张卡**不可相加**：「客户端数据」已包含会话资产，「工具数据」已包含回收站；
 * - `protectedPending` 时数字是部分和，必须标注「受保护项计算中」。
 */
export function StorageOverview({ scan }: OverviewProps) {
  const store = scan.roots.find((root) => root.id === "store");
  const clientRoots = scan.roots.filter((root) => root.id !== "store");
  const clientBytes = clientRoots.reduce((sum, root) => sum + (root.bytes ?? 0), 0);
  const clientFiles = clientRoots.reduce((sum, root) => sum + (root.files ?? 0), 0);
  const clientLabels = clientRoots
    .filter((root) => root.exists)
    .map((root) => root.label)
    .join(" + ");

  const cards = [
    {
      icon: ServerCog,
      label: "工具数据",
      value: store?.bytes ?? 0,
      detail: formatCount(store?.files ?? 0),
    },
    {
      icon: HardDrive,
      label: "客户端数据",
      value: clientBytes,
      detail: clientLabels
        ? `${clientLabels} 合计 · ${formatCount(clientFiles)}`
        : formatCount(clientFiles),
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
            <div className="mt-1 break-all text-[11px] text-muted-foreground">{card.detail}</div>
          </div>
        ))}
      </div>

      <div className="flex flex-wrap items-center gap-x-4 gap-y-2 text-xs text-muted-foreground">
        <span>
          共 {formatBytes(scan.totals.bytes)} / {formatCount(scan.totals.files)}
        </span>
        <span>扫描于 {formatTime(scan.generatedAt)}</span>
        {scan.cached ? <Badge variant="secondary">60 秒快照</Badge> : null}
        {scan.timings ? (
          <span className="tabular-nums">
            扫描 {scan.timings.catalogMs + scan.timings.sessionsMs + scan.timings.trashMs} ms
          </span>
        ) : null}
        {scan.protectedPending ? (
          <Badge
            variant="secondary"
            className="border-warning-border bg-warning-soft text-warning"
            title="运行时 / 插件 / 图片附件 / 账号目录等大目录在后台计量，完成后自动刷新"
          >
            受保护项计算中
          </Badge>
        ) : null}
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

      <p className="text-[11px] leading-5 text-muted-foreground">
        四张卡不可相加：「客户端数据」已包含「会话资产包」（= 会话资产页签里逐条之和），
        「工具数据」已包含「回收站」（= 分类明细里的「回收站」行）；后两张是子集，不是并列项。
        {scan.protectedPending ? "受保护大目录计算中，当前总量是部分和。" : "总量已含受保护大目录。"}
      </p>
    </div>
  );
}
