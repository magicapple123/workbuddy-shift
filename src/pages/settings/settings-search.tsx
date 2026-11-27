import { Search, X } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";

interface Props {
  value: string;
  onChange: (next: string) => void;
  /** 命中数 / 总节数：搜索时显示「3 / 16 项」，清空后不显示。 */
  matched: number;
  total: number;
}

/**
 * 设置搜索框（受控）。
 *
 * 索引来自 `sections.ts` 的静态注册表，不是 DOM 文本：折叠着的行、加载失败的行
 * 也能被搜到，且搜索不会因为某张卡当前渲染成空态而漏掉整节。
 */
export function SettingsSearch({ value, onChange, matched, total }: Props) {
  const searching = value.trim().length > 0;

  return (
    <div className="w-full sm:max-w-xs">
      {/* 定位锚点只包输入框：外层还带「匹配 N / 16 项」提示行，若把 absolute 放在外层，
          `top-1/2` 会按整个容器（输入框 + 提示行）居中，图标实测比输入框中心低 10px。 */}
      <div className="relative">
        <Search className="pointer-events-none absolute left-2.5 top-1/2 size-4 -translate-y-1/2 text-muted-foreground" />
        <Input
          value={value}
          onChange={(event) => onChange(event.target.value)}
          placeholder="搜索设置项"
          aria-label="搜索设置项"
          className="h-9 pl-8 pr-9"
          spellCheck={false}
          autoComplete="off"
        />
        {searching ? (
          <Button
            type="button"
            variant="ghost"
            size="icon"
            className="absolute right-1 top-1/2 size-7 -translate-y-1/2"
            onClick={() => onChange("")}
            aria-label="清空搜索"
            title="清空搜索"
          >
            <X />
          </Button>
        ) : null}
      </div>
      <p className="mt-1 min-h-4 text-[11px] leading-4 text-muted-foreground" aria-live="polite">
        {searching ? `匹配 ${matched} / ${total} 项` : ""}
      </p>
    </div>
  );
}
