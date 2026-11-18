import { ArrowDownToLine, ArrowUpFromLine, Zap } from "lucide-react";
import { HoverCard, HoverCardContent, HoverCardTrigger } from "@/components/ui/hover-card";
import { TableCell } from "@/components/ui/table";
import type {
  TokenStatsRequestRow,
} from "@/lib/types";

import { formatTokenCompact, formatTokenExact, percentage } from "./data";

/**
 * 详情卡的一行：分类色块 + 数值。`depth = 1` 表示父项（输入 / 输出）的拆分项，
 * 只缩进、不再画色块，保持「父项 → 子项」的视觉层次。
 */
function UsageDetailLine({
  label,
  value,
  color,
  depth = 0,
}: {
  label: string;
  value: number;
  color?: string;
  depth?: 0 | 1;
}) {
  return (
    <div className={`flex items-center justify-between gap-3 ${depth === 1 ? "pl-4" : ""}`}>
      <span className="inline-flex items-center gap-1.5 text-muted-foreground">
        {color ? (
          <span
            className="size-2 shrink-0 rounded-[2px]"
            style={{ backgroundColor: color }}
            aria-hidden="true"
          />
        ) : null}
        {label}
      </span>
      <span className="tabular-nums">{formatTokenExact(value)}</span>
    </div>
  );
}

/**
 * 「用量」单元格：默认只显示总计，副行是 输入 / 输出 / 命中率；悬停或键盘聚焦
 * 弹出「Token 消耗明细」分类卡（Radix HoverCard 自带 focus 触发，不需要本地状态）。
 *
 * 口径（与后端明细行一致，见 design §10.2）：总计 = 输入 + 输出 + 缓存写入；
 * 输入 = 缓存命中 + 缓存未命中；输出 = 思考过程 + 回复内容（饱和减）。
 * 缓存写入与输入 / 输出同级：本项目 `input` 不含 cacheWrite，嵌进输入父子加不上。
 */
export function RequestUsageCell({ row }: { row: TokenStatsRequestRow }) {
  // 旧后端可能没有 thinking 键，按 0 兜底。
  const thinking = row.thinking ?? 0;
  const reply = Math.max(0, row.output - thinking);
  const segments = [
    { label: "命中", value: row.cacheRead, color: "var(--data-series-blue)" },
    { label: "未命中", value: row.uncachedInput, color: "var(--data-series-rose)" },
    { label: "写入", value: row.cacheWrite, color: "var(--data-series-slate)" },
  ];
  const legend = [
    { label: "命中", color: "var(--data-series-blue)" },
    { label: "写入", color: "var(--data-series-slate)" },
    { label: "未命中", color: "var(--data-series-rose)" },
  ];

  return (
    <HoverCard openDelay={150} closeDelay={80}>
      <HoverCardTrigger asChild>
        <TableCell
          tabIndex={0}
          className="w-[150px] rounded-md text-right outline-hidden focus-visible:ring-2 focus-visible:ring-ring/50"
        >
          <span className="block text-[15px] font-medium tabular-nums">
            {formatTokenCompact(row.total)}
          </span>
          <span className="mt-0.5 flex items-center justify-end gap-2 text-[11px] text-muted-foreground tabular-nums">
            <span className="inline-flex items-center gap-0.5" title="输入">
              <ArrowUpFromLine className="size-3" aria-hidden="true" />
              {formatTokenCompact(row.input)}
            </span>
            <span className="inline-flex items-center gap-0.5" title="输出">
              <ArrowDownToLine className="size-3" aria-hidden="true" />
              {formatTokenCompact(row.output)}
            </span>
            <span title="缓存命中率：缓存命中 / 输入">{percentage(row.cacheRead, row.input)}</span>
          </span>
        </TableCell>
      </HoverCardTrigger>
      {/* 贴在单元格左侧：这张卡比「用量」列高得多，若开在下方会盖住后面几行的
          用量数字（正在纵向对比时最碍事）。靠左后在垂直方向仍由碰撞检测兜底。 */}
      <HoverCardContent
        side="left"
        align="center"
        sideOffset={6}
        collisionPadding={8}
        className="w-[264px] space-y-1.5 text-xs"
      >
        <div className="flex items-center justify-between gap-3">
          <span className="font-medium">Token 消耗明细</span>
          <span className="text-muted-foreground tabular-nums">
            总计 {formatTokenExact(row.total)}
          </span>
        </div>
        <div className="space-y-1 border-t pt-1.5">
          <UsageDetailLine label="输入" value={row.input} color="var(--data-series-sky)" />
          <UsageDetailLine
            label="缓存命中"
            value={row.cacheRead}
            color="var(--data-series-blue)"
            depth={1}
          />
          <UsageDetailLine
            label="缓存未命中"
            value={row.uncachedInput}
            color="var(--data-series-rose)"
            depth={1}
          />
          <UsageDetailLine label="输出" value={row.output} color="var(--data-series-violet)" />
          <UsageDetailLine label="思考过程" value={thinking} depth={1} />
          <UsageDetailLine label="回复内容" value={reply} depth={1} />
          <UsageDetailLine
            label="缓存写入"
            value={row.cacheWrite}
            color="var(--data-series-slate)"
          />
        </div>
        <div className="flex items-center justify-between gap-3 border-t pt-1.5">
          <span className="inline-flex items-center gap-1 text-muted-foreground">
            <Zap className="size-3" aria-hidden="true" />
            缓存命中率
          </span>
          <span className="font-medium tabular-nums" style={{ color: "var(--data-series-blue)" }}>
            {percentage(row.cacheRead, row.input)}
          </span>
        </div>
        <div className="space-y-1.5 border-t pt-1.5">
          {/* 轨道用前景色透明度：暗色主题下 `--muted` 与卡片同色，背景色会看不见。 */}
          <div className="flex h-2 w-full overflow-hidden rounded-full bg-foreground/10">
            {segments.map((segment) =>
              segment.value > 0 ? (
                <div
                  key={segment.label}
                  className="h-full shrink"
                  style={{
                    flexBasis: 0,
                    flexGrow: segment.value,
                    // 占比极小的段也要看得见，但不改变其余段的比例关系。
                    minWidth: 4,
                    backgroundColor: segment.color,
                  }}
                />
              ) : null,
            )}
          </div>
          <div className="flex flex-wrap items-center gap-x-3 gap-y-1 text-[11px] text-muted-foreground">
            {legend.map((item) => (
              <span key={item.label} className="inline-flex items-center gap-1">
                <span
                  className="size-2 shrink-0 rounded-[2px]"
                  style={{ backgroundColor: item.color }}
                  aria-hidden="true"
                />
                {item.label}
              </span>
            ))}
          </div>
        </div>
      </HoverCardContent>
    </HoverCard>
  );
}
