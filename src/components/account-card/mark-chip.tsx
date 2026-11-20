import { Tag } from "lucide-react";

import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";
import type { AccountMark, AccountMarkColor } from "@/lib/types";

import { formatMarkClock, formatMarkExpiryLine, markAriaLabel } from "./formatters";

/** 标记色 → 图标底色 / 前景色（只影响图标，不加左侧色条、不显示文字）。 */
export const MARK_TONE: Record<AccountMarkColor, string> = {
  warning: "bg-warning-soft text-warning",
  danger: "bg-danger-soft text-danger",
  info: "bg-primary-soft text-primary-soft-foreground",
  success: "bg-success-soft text-success",
  neutral: "bg-secondary text-secondary-foreground",
};

/**
 * 标记图标（R5-04）：**只有图标、没有文字、没有左侧色条**。
 *
 * 用 `<button>` 而不是 `Badge`：需求要求图标可点击。`statusIconChip` 渲染的是 `span`，
 * 外面再套按钮会变成嵌套可交互元素（无效的无障碍结构），所以这里直接写按钮 + 同款徽标样式。
 * 视觉尺寸仍是 20×20，用 `before:-inset-1` 把可点区域扩到约 28×28，避免小图标难点。
 *
 * 悬停 tooltip 固定三行：标记文本 / 到期信息（`至 … · 剩 2 小时` 或「手动解除」）/ 创建时间。
 */
export function AccountMarkChip({
  mark,
  now,
  onEdit,
}: {
  mark: AccountMark;
  now: number;
  onEdit?: () => void;
}) {
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <button
          type="button"
          onClick={() => onEdit?.()}
          aria-label={markAriaLabel(mark, now)}
          className={cn(
            "relative inline-flex size-5 shrink-0 items-center justify-center rounded-md",
            "before:absolute before:-inset-1 before:content-['']",
            "transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring",
            MARK_TONE[mark.color],
          )}
        >
          <Tag className="size-3.5" aria-hidden="true" />
        </button>
      </TooltipTrigger>
      <TooltipContent side="top">
        <span className="flex flex-col gap-0.5 text-[11px] leading-4">
          <span>{mark.text}</span>
          <span>{formatMarkExpiryLine(mark, now)}</span>
          <span className="text-muted-foreground">创建于 {formatMarkClock(mark.createdAt)}</span>
        </span>
      </TooltipContent>
    </Tooltip>
  );
}
