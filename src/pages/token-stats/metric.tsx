import type {
  ReactNode,
} from "react";
import { type LucideIcon } from "lucide-react";
import type {
  TokenStatsTotals,
} from "@/lib/types";

import { formatTokenCompact, formatTokenExact, percentage, tokenTotal } from "./data";

export function SectionTitle({ id, children }: { id: string; children: ReactNode }) {
  return (
    <div className="px-1">
      <h2 id={id} className="text-[13px] font-medium leading-5">
        {children}
      </h2>
    </div>
  );
}

export function StatMetric({
  icon: Icon,
  label,
  value,
  divided = false,
}: {
  icon: LucideIcon;
  label: string;
  value: string;
  divided?: boolean;
}) {
  return (
    <div
      className={`flex min-w-0 flex-col items-center justify-center px-4 py-5 text-center sm:py-3 ${
        divided ? "sm:border-l sm:border-border/60" : ""
      }`}
    >
      <div className="flex max-w-full items-center justify-center gap-2 text-[13px] font-medium leading-5 text-muted-foreground">
        <Icon className="size-4 shrink-0 stroke-[1.75]" aria-hidden="true" />
        <span className="truncate">{label}</span>
      </div>
      <div
        className="mt-3 max-w-full truncate text-[26px] font-semibold leading-8 tracking-[-0.025em] text-foreground tabular-nums"
        style={{
          fontFamily:
            '"Bricolage Grotesque Variable", "SF Pro Display", ui-sans-serif, sans-serif',
        }}
      >
        {value}
      </div>
    </div>
  );
}

export function CompactComposition({ value }: { value: TokenStatsTotals }) {
  const total = tokenTotal(value);
  const rows = [
    // 与趋势图图例共用同一套 token，避免"同一个指标两种颜色"。
    { label: "缓存占比", value: value.cacheRead, color: "bg-[var(--data-series-blue)]" },
    { label: "新增", value: value.uncachedInput, color: "bg-[var(--data-series-cyan)]" },
    { label: "输出", value: value.output, color: "bg-[var(--data-series-violet)]" },
    { label: "写入", value: value.cacheWrite, color: "bg-[var(--data-series-slate)]" },
  ];

  return (
    <div className="min-w-0 flex-1">
      <div className="min-w-0">
        <div
          className="flex h-2 w-full max-w-[360px] overflow-hidden rounded-full bg-muted"
          role="img"
          aria-label={rows
            .map((row) => `${row.label} ${percentage(row.value, total)}`)
            .join("，")}
        >
          {rows.map((row) => (
            <span
              key={row.label}
              className={`h-full min-w-0 ${row.color}`}
              style={{ width: total > 0 ? `${(row.value / total) * 100}%` : "0%" }}
              title={`${row.label} ${formatTokenCompact(row.value)} · ${percentage(row.value, total)}`}
              aria-label={`${row.label} ${formatTokenExact(row.value)} Token，${percentage(row.value, total)}`}
            />
          ))}
        </div>
        <div className="mt-1 flex flex-wrap gap-x-2.5 gap-y-0.5 text-[10px] text-muted-foreground">
          {rows.map((row) => (
            <span key={row.label} className="inline-flex items-center gap-1 whitespace-nowrap">
              <span className={`size-1.5 rounded-full ${row.color}`} aria-hidden="true" />
              {row.label} {percentage(row.value, total)}
            </span>
          ))}
        </div>
      </div>
    </div>
  );
}
