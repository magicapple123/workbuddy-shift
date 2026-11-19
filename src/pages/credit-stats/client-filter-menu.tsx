import { Check, MonitorSmartphone } from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";

import { CLIENT_VIEW_LABELS, type ClientViewOption, type CreditClientKey } from "./data";

/**
 * 积分页「客户端」筛选（第四轮 06）。
 *
 * 只列**明细里真实出现过**的客户端（含「未知」），不凭空列出空选项；
 * 与档位切换是两个正交维度，所以单独一个菜单而不是塞进档位 tab。
 */
export function ClientFilterMenu({
  options,
  value,
  onChange,
  counts,
}: {
  options: ClientViewOption[];
  value: CreditClientKey;
  onChange: (value: CreditClientKey) => void;
  /** 明细总条数（「全部客户端」右侧的角标）。 */
  counts: number;
}) {
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button
          variant="ghost"
          size="sm"
          className="h-8 max-w-[190px] gap-1.5 px-2.5 text-xs text-muted-foreground hover:text-foreground"
          aria-label="按客户端筛选"
        >
          <MonitorSmartphone className="size-3.5 shrink-0" />
          <span className="truncate">{CLIENT_VIEW_LABELS[value]}</span>
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="max-h-80 w-60 overflow-y-auto">
        <DropdownMenuItem onSelect={() => onChange("all")}>
          <span className="min-w-0 flex-1 truncate">{CLIENT_VIEW_LABELS.all}</span>
          <span className="text-[11px] tabular-nums text-muted-foreground">{counts}</span>
          {value === "all" && <Check className="ml-2 size-3.5 shrink-0" />}
        </DropdownMenuItem>
        <DropdownMenuSeparator />
        {options.map((option) => (
          <DropdownMenuItem key={option.key} onSelect={() => onChange(option.key)}>
            <span className="min-w-0 flex-1 truncate">{option.label}</span>
            <span className="text-[11px] tabular-nums text-muted-foreground">{option.count}</span>
            {value === option.key && <Check className="ml-2 size-3.5 shrink-0" />}
          </DropdownMenuItem>
        ))}
        {options.length === 0 && (
          <DropdownMenuItem disabled>明细里没有客户端字段</DropdownMenuItem>
        )}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
