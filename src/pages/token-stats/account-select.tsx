import { Users } from "lucide-react";

import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";

import type { AccountOption } from "./accounts";

/**
 * Token 统计的账号下拉：每个来源一个，各自记忆上次选择（第 7 条决策）。
 *
 * 选项来自账号库（按来源/档位过滤）+ 「所有账号」+「未归属」；零用量账号也列出来，
 * 选中后显示 0 与空态，不发请求、不回骨架屏。
 */
export function TokenAccountSelect({
  options,
  value,
  onChange,
  disabled = false,
  notice,
}: {
  options: AccountOption[];
  value: string;
  onChange: (value: string) => void;
  disabled?: boolean;
  /** 该来源的账号维度限制说明（例如 CLI 日志不记录账号）。 */
  notice?: string | null;
}) {
  return (
    <div className="flex min-w-0 flex-wrap items-center gap-2">
      {notice && (
        <span className="min-w-0 max-w-md text-[11px] leading-5 text-muted-foreground">{notice}</span>
      )}
      <Select value={value} onValueChange={onChange} disabled={disabled}>
        <SelectTrigger className="h-9 w-[200px] max-w-full gap-2" aria-label="按账号筛选">
          <Users className="size-4 shrink-0 opacity-70" aria-hidden="true" />
          <SelectValue placeholder="所有账号" />
        </SelectTrigger>
        <SelectContent>
          {options.map((option) => (
            <SelectItem key={option.value} value={option.value}>
              {option.label}
              {option.hasUsage ? null : (
                <span className="ml-1 text-[11px] text-muted-foreground">（无用量）</span>
              )}
            </SelectItem>
          ))}
        </SelectContent>
      </Select>
    </div>
  );
}
