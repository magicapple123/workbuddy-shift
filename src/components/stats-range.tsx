import { statsRangeOptions, type StatsRangeKey } from "@/lib/stats-range";
import { cn } from "@/lib/utils";

/**
 * 统计页唯一的「统计范围」控件（积分统计 / Token 统计共用）。
 *
 * 为什么抽成一个组件而不是各页自绘：改造前两页共 6 处外观相同的范围按钮（页面顶部
 * 取数窗口 + 卡片内展示区间），默认值和可选项都不一致。抽出来后每页的 `aria-label="统计范围"`
 * 只有一处（由 `ariaLabel` 显式传入，默认值相同），验收时每页恰好一个不会被卡片内的
 * 重复控件打破。
 */
export function StatsRangeControl({
  value,
  include90d,
  onChange,
  disabled = false,
  hints,
  ariaLabel = "统计范围",
}: {
  value: StatsRangeKey;
  /** 是否渲染「近 90 天」第 6 项（数据不支持时不渲染，而不是渲染成禁用）。 */
  include90d: boolean;
  onChange: (next: StatsRangeKey) => void;
  disabled?: boolean;
  /** 每个选项的悬停说明（口径来源）。 */
  hints?: Partial<Record<StatsRangeKey, string>>;
  /** 无障碍名称；默认「统计范围」，页面里应保持全页唯一。 */
  ariaLabel?: string;
}) {
  const options = statsRangeOptions(include90d);

  return (
    <div
      className="flex max-w-full flex-wrap gap-1 rounded-lg bg-muted p-1"
      role="tablist"
      aria-label={ariaLabel}
    >
      {options.map((option) => {
        const active = option.key === value;
        return (
          <button
            key={option.key}
            type="button"
            role="tab"
            aria-selected={active}
            disabled={disabled}
            title={hints?.[option.key]}
            className={cn(
              "cursor-pointer rounded-md px-2.5 py-1.5 text-xs transition-colors",
              active
                ? "bg-background font-medium text-foreground shadow-sm"
                : "text-muted-foreground hover:text-foreground",
              disabled && "cursor-not-allowed opacity-50",
            )}
            onClick={() => onChange(option.key)}
          >
            {option.label}
          </button>
        );
      })}
    </div>
  );
}
