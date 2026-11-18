/**
 * 切号弹窗底部摘要的纯逻辑（WorkBuddy / VS Code 插件 / CodeBuddy IDE 共用）。
 *
 * 三个弹窗的「将复制 / 将同步 / 覆盖风险」文案完全一致，差异只在未选择任何会话时的
 * 补充说明（CodeBuddy IDE 国际版要提示确认后会重启或打开 IDE），因此抽成一份按输入参数化的实现。
 */

export interface SwitchAccountSummaryInput {
  /** 勾选要复制的会话数（复制开关打开时才 > 0）。 */
  copyCount: number;
  /** 勾选要同步的关联会话数。 */
  syncCount: number;
  /** 其中会覆盖目标账号完整内容的同步项数。 */
  overwriteCount: number;
  /** 「未选择复制或同步会话」时的补充说明；缺省用通用文案。 */
  idleHint?: string;
}

export interface SwitchAccountSummary {
  /** 主摘要：将复制 / 将同步 / 本次仅切换账号。 */
  main: string;
  /** 次摘要：覆盖风险或未选择说明；无内容时为 null。 */
  sub: string | null;
}

export function switchAccountSummary({
  copyCount,
  syncCount,
  overwriteCount,
  idleHint,
}: SwitchAccountSummaryInput): SwitchAccountSummary {
  const main =
    copyCount > 0 && syncCount > 0
      ? `将复制 ${copyCount} 个、同步 ${syncCount} 个关联会话`
      : copyCount > 0
        ? `将复制 ${copyCount} 个会话`
        : syncCount > 0
          ? `将同步 ${syncCount} 个关联会话`
          : "本次仅切换账号";
  const sub =
    overwriteCount > 0
      ? `其中 ${overwriteCount} 个会替换目标账号的完整内容`
      : copyCount === 0 && syncCount === 0
        ? (idleHint ?? "未选择复制或同步会话")
        : copyCount === 0
          ? "未选择复制会话"
          : syncCount === 0
            ? "未选择同步会话"
            : null;
  return { main, sub };
}
