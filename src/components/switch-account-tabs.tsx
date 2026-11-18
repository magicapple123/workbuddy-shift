/**
 * 切号弹窗共用的 tab 外观：下划线指示 + 可选计数徽标。
 *
 * WorkBuddy 切号弹窗与 VS Code / CodeBuddy IDE 切号弹窗同形，两处此前各写了一份；
 * 收敛到这里，改样式时两个弹窗保持一致。
 */
export const TAB_TRIGGER_CLASS =
  "-mb-px h-9 flex-none rounded-none border-b-2 border-transparent px-0.5 pb-2 text-sm font-medium text-muted-foreground hover:text-foreground data-[state=active]:border-primary data-[state=active]:bg-transparent data-[state=active]:text-foreground data-[state=active]:shadow-none";

/** tab 计数徽标：0 时不显示，避免出现空的「0」。 */
export function tabCount(count: number) {
  return count > 0 ? (
    <span className="ml-1 text-xs text-muted-foreground tabular-nums">{count}</span>
  ) : null;
}
