import { ClientSwitchAccountDialog } from "@/components/client-switch-account-dialog";
import type { AccountMeta, CodeBuddyCnIdeStatus, WbVariant } from "@/lib/types";

interface Props {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** 目标账号 */
  account: AccountMeta | null;
  /**
   * 当前档位：决定会话列表 / 关联预览 / 切换走哪条通道。
   *
   * 国内版（`CodeBuddy CN.app`）与国际版（`CodeBuddy.app`）共用同一套会话存储与后端语义，
   * 通道分流由共享弹窗按 `variant` 处理。
   */
  variant: WbVariant;
  /** CodeBuddy IDE 状态（用于渲染空态与运行中提示）。 */
  ideStatus?: CodeBuddyCnIdeStatus | null;
  /** 切换完成后刷新列表 */
  onDone?: () => void;
}

/**
 * CodeBuddy IDE 会话切换弹窗：可勾选「当前 IDE 账号」的会话复制到目标账号。
 *
 * 国内版与国际版共用本组件（差异只有三处 API 通道，按 `variant` 分流），与 VS Code 插件弹窗
 * 同形（关联会话 / 复制会话两个 tab），差异：
 * - 切换固定走「关闭并重开 IDE」（`restart = true`），不提供自动关闭开关；
 * - 复制默认沿用会话 id，仅目标已有同 id 时改用新 id（后端决定，前端只提交引用）。
 */
export function CodebuddyIdeSwitchAccountDialog({ open, onOpenChange, account, variant, ideStatus, onDone }: Props) {
  return (
    <ClientSwitchAccountDialog
      client="codebuddy-ide"
      open={open}
      onOpenChange={onOpenChange}
      account={account}
      status={ideStatus}
      variant={variant}
      onDone={onDone}
    />
  );
}
