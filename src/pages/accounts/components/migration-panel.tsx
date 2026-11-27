import { ExternalLink, QrCode } from "lucide-react";

import { DemoAction } from "@/components/demo-action";
import { Button } from "@/components/ui/button";
import type { WbVariant } from "@/lib/types";

import { AccountBackupActions } from "./account-backup-actions";

interface Props {
  variant: WbVariant;
  accountCount: number;
  onAdd: () => void;
  onImport: () => void;
  onExport: () => void;
  /** 加密导入完成后刷新账号列表（加密导入不经预览勾选，只能靠回调通知页面）。 */
  onEncryptedImported?: () => void;
}

/**
 * 「添加与迁移账号」面板：OAuth 接入 + 备份动作组（`[导入备份 |▾]`、`[导出 |▾]`）。
 *
 * 备份动作的交互结构（明文 / 加密两个档位、WebUI 降级、0 账号禁用、键盘可达性）
 * 全部收进 `AccountBackupActions`，本面板只留一行调用：这里是页面布局，那里是按钮行为，
 * 混在一起会让「改 ▾ 的可用性」和「改面板留白」互相拖后腿。
 */
export function AccountMigrationPanel({
  variant,
  accountCount,
  onAdd,
  onImport,
  onExport,
  onEncryptedImported,
}: Props) {
  return (
    <div className="relative mb-6 overflow-visible rounded-2xl border border-border bg-muted/30 px-5 py-5 shadow-[0_6px_20px_rgba(15,23,42,.025)]">
      <div className="pointer-events-none absolute inset-0 overflow-hidden rounded-2xl">
        <div className="absolute -right-12 -top-20 size-44 rounded-full border-[28px] border-slate-400/[0.035]" />
      </div>
      <div className="relative flex flex-wrap items-center gap-x-5 gap-y-4">
        <div className="min-w-[190px] flex-1">
          <h2 className="text-sm font-semibold text-foreground">添加与迁移账号</h2>
          <p className="mt-1 text-xs leading-5 text-muted-foreground">
            {variant === "ai"
              ? "快速接入国际版账号，或从已有环境恢复"
              : "快速接入新账号，或从已有环境恢复"}
          </p>
        </div>
        <div className="flex flex-wrap items-center gap-2.5">
          <DemoAction>
            {/* 全站唯一的 hero CTA：渐变 + 内侧高光只在这里出现。 */}
            <Button variant="brand" className="h-10 px-4" onClick={onAdd}>
              {variant === "ai" ? <ExternalLink /> : <QrCode />}
              {variant === "ai" ? "OAuth 登录" : "OAuth 扫码添加"}
            </Button>
          </DemoAction>
        </div>
        <AccountBackupActions
          accountCount={accountCount}
          onImport={onImport}
          onExport={onExport}
          onImported={onEncryptedImported}
        />
      </div>
    </div>
  );
}
