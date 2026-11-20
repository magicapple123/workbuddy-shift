import type { ReactNode } from "react";
import { ArrowLeftRight, Check, ChevronDown, Loader2 } from "lucide-react";

import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";

/**
 * 紧凑卡的产品切换下拉（R5-05）。
 *
 * 紧凑卡宽度只有 300–340px，5 个产品按钮会把账号名压成 1–2 个字，所以收成一个
 * 下拉：触发按钮显示「当前激活产品图标 + chevron」，菜单里列全部可用产品。
 *
 * 非紧凑卡**保持 5 个按钮原样**，不受本组件影响。
 */

/** 一个产品入口；由 `header.tsx` 按现有 props 组装，本组件只负责渲染。 */
export interface CompactToolEntry {
  /** 稳定标识（也是 React key）。 */
  id: string;
  /** 菜单里的产品名（WorkBuddy / CodeBuddy IDE / …）。 */
  label: string;
  icon: ReactNode;
  /** 当前账号在该产品里已激活（右侧打勾，菜单项置灰）。 */
  active: boolean;
  /** 可点击：已安装 / 已接入且没有并发切换。 */
  enabled: boolean;
  /** 切换中：图标换成 spinner，菜单项置灰。 */
  loading: boolean;
  /** 不可用原因（未安装 / 未接入 / 正在切换），作为副文案与 tooltip。 */
  disabledReason?: string;
  onSelect?: () => void;
}

export function CompactToolMenu({ entries }: { entries: readonly CompactToolEntry[] }) {
  if (entries.length === 0) return null;
  // 多个产品同时激活时按传入顺序（workbuddy > codebuddyIde > vscodeExt > jetbrains > codebuddyCli）取第一个。
  const current = entries.find((entry) => entry.active) ?? null;
  const triggerLabel = current ? current.label : "切换";

  return (
    <DropdownMenu>
      <Tooltip>
        <TooltipTrigger asChild>
          <DropdownMenuTrigger asChild>
            <Button
              variant="outline"
              size="sm"
              className="h-7 gap-0.5 rounded-lg px-1.5"
              aria-label="切换产品"
            >
              {current ? current.icon : <ArrowLeftRight className="size-3.5" />}
              <ChevronDown className="size-3 opacity-70" aria-hidden="true" />
            </Button>
          </DropdownMenuTrigger>
        </TooltipTrigger>
        <TooltipContent side="top">{`切换产品（当前：${triggerLabel}）`}</TooltipContent>
      </Tooltip>
      <DropdownMenuContent align="end" className="w-56">
        {entries.map((entry, index) => {
          const disabled = !entry.enabled || entry.loading || entry.active;
          return (
            <div key={entry.id}>
              {/* 分隔线：WorkBuddy 系与 IDE / 插件系之间一条，纯视觉分组。 */}
              {index === 1 && <DropdownMenuSeparator />}
              <DropdownMenuItem
                disabled={disabled}
                onSelect={() => {
                  if (disabled) return;
                  entry.onSelect?.();
                }}
                aria-label={
                  entry.active
                    ? `${entry.label}（当前使用）`
                    : entry.enabled
                      ? `切换到 ${entry.label}`
                      : `${entry.label}：${entry.disabledReason ?? "不可用"}`
                }
                className={cn("gap-2", entry.active && "font-medium")}
              >
                <span className="flex size-4 shrink-0 items-center justify-center">
                  {entry.loading ? <Loader2 className="size-3.5 animate-spin" /> : entry.icon}
                </span>
                <span className="min-w-0 flex-1 truncate">{entry.label}</span>
                {entry.active ? (
                  <Check className="size-3.5 shrink-0 text-primary" aria-hidden="true" />
                ) : !entry.enabled ? (
                  <span className="max-w-[7rem] shrink-0 truncate text-[10px] text-muted-foreground">
                    {entry.disabledReason ?? "不可用"}
                  </span>
                ) : null}
              </DropdownMenuItem>
            </div>
          );
        })}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
