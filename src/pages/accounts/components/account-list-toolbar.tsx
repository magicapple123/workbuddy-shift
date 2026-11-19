import { ArrowDown, ArrowUp, ArrowUpDown, Columns3, RefreshCw, Rows3 } from "lucide-react";

import { DemoAction } from "@/components/demo-action";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";

import { type SortKey, type SortState } from "../ordering";

interface Props {
  accountCount: number;
  /** 当前档位名（「国内版」/「国际版」），用于计数徽标的无障碍文案。 */
  accountTypeLabel: string;
  compact: boolean;
  onToggleCompact: () => void;
  /** 积分刷新或批量签到进行中。 */
  busy: boolean;
  /** 没有任何可见账号时禁用刷新。 */
  refreshDisabled: boolean;
  refreshLabel: string;
  demoMode: boolean;
  onRefresh: () => void;
  /** 当前排序（六项单选 + 同项反转）。 */
  sort: SortState;
  /** 点击某个维度：非当前项=切换并按默认方向，当前项=只翻转方向。 */
  onSortChange: (key: SortKey) => void;
  /** 限额监听开关；`false` 时「额度恢复时间」置灰（`null`=配置未读到，不禁用）。 */
  rateLimitEnabled: boolean | null;
}

interface SortOption {
  key: SortKey;
  label: string;
  /** 该维度默认方向，仅在菜单里给非当前项做提示时用。 */
  hint: string;
}

/** 六个排序维度的文案（顺序即菜单顺序）。 */
const SORT_OPTIONS: readonly SortOption[] = [
  { key: "default", label: "默认", hint: "按积分优先级" },
  { key: "rateLimitRecovery", label: "额度恢复时间", hint: "最快恢复在前" },
  { key: "creditExpiry", label: "积分到期时间", hint: "最快到期在前" },
  { key: "creditRemaining", label: "剩余积分", hint: "多到少" },
  { key: "creditUpdated", label: "最近更新", hint: "新到旧" },
  { key: "name", label: "名称", hint: "拼音升序" },
];

/** 当前维度的按钮文案（默认维度展示「排序」，避免按钮看起来像「空排序」）。 */
function triggerLabel(sort: SortState): string {
  if (sort.key === "default") return "排序";
  return SORT_OPTIONS.find((option) => option.key === sort.key)?.label ?? "排序";
}

function directionIcon(dir: SortState["dir"]) {
  return dir === "asc" ? <ArrowUp className="size-3.5" /> : <ArrowDown className="size-3.5" />;
}

/**
 * 账号列表工具栏：标题 + 计数 + 排序 + 紧凑模式 + 刷新全部积分/签到。
 *
 * 排序是「六项单选 + 同一项再点反转」：点击非当前项切到该维度并按默认方向，
 * 点击当前项只翻转方向且菜单不关闭（方便连点比较两个方向）。
 */
export function AccountListToolbar({
  accountCount,
  accountTypeLabel,
  compact,
  onToggleCompact,
  busy,
  refreshDisabled,
  refreshLabel,
  demoMode,
  onRefresh,
  sort,
  onSortChange,
  rateLimitEnabled,
}: Props) {
  const rateLimitSortDisabled = rateLimitEnabled === false;

  return (
    <div className="mb-4 flex flex-wrap items-center justify-between gap-3">
      <div className="flex items-center gap-2">
        <h2 id="accounts-list-title" className="text-base font-semibold tracking-tight">账号</h2>
        <Badge
          variant="secondary"
          className="h-6 min-w-6 rounded-full border-0 px-1.5 text-[11px] tabular-nums text-muted-foreground shadow-none"
          aria-label={`${accountCount} 个${accountTypeLabel}账号`}
        >
          {accountCount}
        </Badge>
      </div>
      <TooltipProvider delayDuration={400}>
        <div className="ml-auto flex items-center gap-1">
          <DropdownMenu>
            <Tooltip>
              <TooltipTrigger asChild>
                <DropdownMenuTrigger asChild>
                  <Button
                    variant="ghost"
                    size="sm"
                    className={cn("h-9 gap-1.5 rounded-lg px-2", sort.key !== "default" && "text-foreground")}
                    aria-label={`排序：${triggerLabel(sort)}${sort.key === "default" ? "" : sort.dir === "asc" ? "（升序）" : "（降序）"}`}
                  >
                    <ArrowUpDown />
                    {/* 窄屏只留图标：完整维度名在菜单里仍可见，tooltip 也带同一个文案。 */}
                    <span className="hidden sm:inline">{triggerLabel(sort)}</span>
                    {sort.key !== "default" && (
                      <span className="hidden text-muted-foreground sm:inline">{directionIcon(sort.dir)}</span>
                    )}
                  </Button>
                </DropdownMenuTrigger>
              </TooltipTrigger>
              <TooltipContent side="top">
                排序：{triggerLabel(sort)}
                {sort.key === "default" ? "" : sort.dir === "asc" ? "（升序）" : "（降序）"}
              </TooltipContent>
            </Tooltip>
            <DropdownMenuContent align="end" className="w-52">
              {SORT_OPTIONS.map((option) => {
                const active = sort.key === option.key;
                const disabled = option.key === "rateLimitRecovery" && rateLimitSortDisabled;
                const item = (
                  <DropdownMenuItem
                    key={option.key}
                    // 当前项重复点击只反转方向，菜单保持打开，便于在两个方向间比较。
                    onSelect={(event) => {
                      if (disabled) {
                        event.preventDefault();
                        return;
                      }
                      if (active && option.key !== "default") event.preventDefault();
                      onSortChange(option.key);
                    }}
                    aria-disabled={disabled || undefined}
                    aria-checked={active}
                    role="menuitemradio"
                    className={cn(
                      "justify-between",
                      active && "bg-accent font-medium text-accent-foreground",
                      disabled && "cursor-not-allowed opacity-50",
                    )}
                  >
                    <span className="flex min-w-0 items-center gap-2">
                      <span className="truncate">{option.label}</span>
                    </span>
                    {active && option.key !== "default" && (
                      <span className="shrink-0 text-muted-foreground">{directionIcon(sort.dir)}</span>
                    )}
                  </DropdownMenuItem>
                );

                if (!disabled) return item;
                // 置灰项仍可 hover：`aria-disabled` 而非 Radix `disabled`，否则 pointer-events-none 让 tooltip 失效。
                return (
                  <Tooltip key={option.key}>
                    <TooltipTrigger asChild>{item}</TooltipTrigger>
                    <TooltipContent side="left">需要先在设置里开启限额监听</TooltipContent>
                  </Tooltip>
                );
              })}
              <DropdownMenuSeparator />
              <div className="px-2.5 py-1 text-[11px] leading-4 text-muted-foreground">同一项再点一次可反转</div>
            </DropdownMenuContent>
          </DropdownMenu>
          <Tooltip>
            <TooltipTrigger asChild>
              <Button
                variant="ghost"
                size="icon"
                className={cn("size-9 rounded-lg", compact && "bg-accent text-accent-foreground")}
                onClick={onToggleCompact}
                aria-label={compact ? "切换为宽松模式" : "切换为紧凑模式"}
              >
                {compact ? <Rows3 /> : <Columns3 />}
              </Button>
            </TooltipTrigger>
            <TooltipContent side="top">{compact ? "切换为宽松模式" : "切换为紧凑模式"}</TooltipContent>
          </Tooltip>
          <Tooltip>
            <TooltipTrigger asChild>
              <span>
                <DemoAction>
                  <Button
                    variant="ghost"
                    size="icon"
                    className="size-9 rounded-lg"
                    disabled={refreshDisabled || busy}
                    onClick={onRefresh}
                    aria-label={refreshLabel}
                  >
                    <RefreshCw className={busy ? "animate-spin" : undefined} />
                  </Button>
                </DemoAction>
              </span>
            </TooltipTrigger>
            <TooltipContent side="top">{demoMode ? "演示模式下不可操作" : refreshLabel}</TooltipContent>
          </Tooltip>
        </div>
      </TooltipProvider>
    </div>
  );
}
