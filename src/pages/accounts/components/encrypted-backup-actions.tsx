import { useState } from "react";
import { FileKey2, LockKeyhole } from "lucide-react";

import { EncryptedExportDialog } from "@/components/encrypted-export-dialog";
import { EncryptedImportDialog } from "@/components/encrypted-import-dialog";
import { Button } from "@/components/ui/button";
import type { ImportResult } from "@/lib/types";

interface Props {
  /** 当前档位下的账号数：为 0 时禁用「加密导出」。 */
  accountCount: number;
  /** 加密导入完成后的回调（页面据此重拉账号列表）。 */
  onImported?: (report: ImportResult) => void;
}

/**
 * 「加密导入 / 加密导出」两个入口（issue #186），挂在账号页的「添加与迁移账号」面板里，
 * 与既有的「导入备份 / 导出」并排：同一件事的两个档位（明文 / 加密），用户不用换地方找。
 *
 * 为什么弹窗开关留在这里而不是塞进账号页控制器（`useAccountsPage`）：这两个弹窗只被本组件
 * 使用，加进那个 hook 只会让它继续膨胀；外部需要的唯一副作用是"导入后刷新列表"，
 * 通过 `onImported` 往上抛一次就够。
 *
 * 只在桌面端渲染（父级用 `api.isDesktop() && !api.isDemoMode()` 守卫）：两条命令都要本机
 * 绝对路径，WebUI 的浏览器沙箱里没有等价的文件系统入口。
 */
export function EncryptedBackupActions({ accountCount, onImported }: Props) {
  const [exportOpen, setExportOpen] = useState(false);
  const [importOpen, setImportOpen] = useState(false);

  return (
    <>
      <Button
        variant="ghost"
        size="sm"
        className="h-9 px-2.5"
        onClick={() => setImportOpen(true)}
        title="导入加密备份（需要导出时设置的口令）"
      >
        <FileKey2 />
        加密导入
      </Button>
      <Button
        variant="ghost"
        size="sm"
        className="h-9 px-2.5"
        onClick={() => setExportOpen(true)}
        disabled={accountCount === 0}
        title="用口令加密后导出全部账号"
      >
        <LockKeyhole />
        加密导出
      </Button>

      <EncryptedExportDialog
        open={exportOpen}
        onOpenChange={setExportOpen}
        accountCount={accountCount}
      />
      <EncryptedImportDialog
        open={importOpen}
        onOpenChange={setImportOpen}
        onImported={onImported}
      />
    </>
  );
}
