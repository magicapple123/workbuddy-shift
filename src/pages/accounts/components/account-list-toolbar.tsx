import { Columns3, RefreshCw, Rows3 } from "lucide-react";

import { DemoAction } from "@/components/demo-action";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";

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
}

/** 账号列表工具栏：标题 + 计数 + 紧凑模式 + 刷新全部积分/签到。 */
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
}: Props) {
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
