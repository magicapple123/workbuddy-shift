import { ExternalLink, FileDown, FileUp, QrCode } from "lucide-react";

import { DemoAction } from "@/components/demo-action";
import { Button } from "@/components/ui/button";
import * as api from "@/lib/api";
import type { WbVariant } from "@/lib/types";

import { EncryptedBackupActions } from "./encrypted-backup-actions";

interface Props {
  variant: WbVariant;
  accountCount: number;
  onAdd: () => void;
  onImport: () => void;
  onExport: () => void;
  /** 加密导入完成后刷新账号列表（加密导入不经预览勾选，只能靠回调通知页面）。 */
  onEncryptedImported?: () => void;
}

/** 「添加与迁移账号」面板：OAuth 接入、导入备份、导出账号（含加密导入/导出）。 */
export function AccountMigrationPanel({
  variant,
  accountCount,
  onAdd,
  onImport,
  onExport,
  onEncryptedImported,
}: Props) {
  // 加密导入/导出要本机绝对路径（保存/打开对话框产物），WebUI 与演示模式没有等价入口。
  const showEncrypted = api.isDesktop() && !api.isDemoMode();

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
        <div className="flex items-center gap-1">
          <DemoAction>
            <Button variant="ghost" size="sm" className="h-9 px-2.5" onClick={onImport} title="从备份文件导入账号">
              <FileUp />导入备份
            </Button>
          </DemoAction>
          <DemoAction>
            <Button
              variant="ghost"
              size="sm"
              className="h-9 px-2.5"
              onClick={onExport}
              disabled={accountCount === 0}
              title="导出账号备份"
            >
              <FileDown />导出
            </Button>
          </DemoAction>
          {/* issue #186：加密导入/导出只在桌面端出现（需要本机绝对路径）。 */}
          {showEncrypted ? (
            <EncryptedBackupActions accountCount={accountCount} onImported={onEncryptedImported} />
          ) : null}
        </div>
      </div>
    </div>
  );
}
