import { ClientSwitchAccountDialog } from "@/components/client-switch-account-dialog";
import type { AccountMeta, VscodeExtStatus } from "@/lib/types";

interface Props {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** 目标账号 */
  account: AccountMeta | null;
  /** VS Code 扩展状态（用于渲染空态与运行中提示）。 */
  vscodeExtStatus?: VscodeExtStatus | null;
  /** 切换完成后刷新列表 */
  onDone?: () => void;
}

/** VS Code 扩展会话切换弹窗：可勾选「当前扩展账号」的会话复制到目标账号。 */
export function VscodeSwitchAccountDialog({ open, onOpenChange, account, vscodeExtStatus, onDone }: Props) {
  return (
    <ClientSwitchAccountDialog
      client="vscode"
      open={open}
      onOpenChange={onOpenChange}
      account={account}
      status={vscodeExtStatus}
      onDone={onDone}
    />
  );
}
