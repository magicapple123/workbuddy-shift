import { Badge } from "@/components/ui/badge";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";

import { chipClass } from "./chip-class";
import { hiddenChipsAriaLabel } from "./status-selection";

/**
 * 紧凑卡被收起的状态：`+N` 徽标（R5-05）。
 *
 * 纯信息、不可点击（清单明确不引入 Popover）：悬停列出被收起的 `detail`，
 * 读屏用户通过 `aria-label` 拿到同一份清单。
 */
export function HiddenStatusChips({ hidden }: { hidden: readonly { detail: string }[] }) {
  if (hidden.length === 0) return null;
  // tooltip 最多 6 行明细，第 7 行起用省略号，避免弹出层比卡片还高。
  const details = hidden.slice(0, 6).map((chip) => chip.detail);
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <Badge
          variant="secondary"
          className={cn(chipClass, "min-w-5 px-1 tabular-nums text-secondary-foreground")}
          aria-label={hiddenChipsAriaLabel(hidden)}
        >
          +{hidden.length}
        </Badge>
      </TooltipTrigger>
      <TooltipContent side="top">
        <span className="flex flex-col gap-0.5 text-[11px] leading-4">
          <span>还有 {hidden.length} 个状态</span>
          {details.map((detail) => (
            <span key={detail}>{detail}</span>
          ))}
          {hidden.length > details.length && <span>…</span>}
        </span>
      </TooltipContent>
    </Tooltip>
  );
}
