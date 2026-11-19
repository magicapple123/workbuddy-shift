import { ChevronDown, Download } from "lucide-react";
import type { ReactNode } from "react";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { DEMO_UNAVAILABLE_MESSAGE, demoModeEnabled } from "@/lib/demo-mode";

export interface ExportMenuItem {
  key: string;
  label: string;
  /** 菜单项右侧的补充说明（格式/范围）。 */
  description?: string;
  icon?: ReactNode;
  onSelect: () => void;
}

/**
 * 统一导出菜单（第四轮 02/05）。
 *
 * Token 页原来两个并列按钮（CSV / Excel），积分页只有 CSV；两页各写一套会让
 * 「导出什么格式、导出当前筛选还是全量」在不同页面给出不同答案。这里把触发按钮、
 * 下拉结构与演示模式拦截收敛到一处，页面只描述「有哪些出口」。
 */
export function ExportMenu({
  items,
  disabled = false,
  label = "导出",
  title,
}: {
  items: ExportMenuItem[];
  disabled?: boolean;
  label?: string;
  title?: string;
}) {
  function run(item: ExportMenuItem) {
    // 演示模式：导出会写到用户磁盘，和真实操作一样必须拦住。
    if (demoModeEnabled) {
      toast.info(DEMO_UNAVAILABLE_MESSAGE);
      return;
    }
    item.onSelect();
  }

  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button
          className="shrink-0"
          variant="outline"
          size="sm"
          disabled={disabled || items.length === 0}
          title={title}
          aria-label={`${label}（${items.length} 种格式）`}
        >
          <Download />
          {label}
          <ChevronDown className="size-3.5 opacity-60" />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="w-64">
        {items.map((item) => (
          <DropdownMenuItem key={item.key} onSelect={() => run(item)} className="gap-2">
            {item.icon}
            <span className="min-w-0 flex-1">
              <span className="block truncate">{item.label}</span>
              {item.description ? (
                <span className="block truncate text-[11px] text-muted-foreground">
                  {item.description}
                </span>
              ) : null}
            </span>
          </DropdownMenuItem>
        ))}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
