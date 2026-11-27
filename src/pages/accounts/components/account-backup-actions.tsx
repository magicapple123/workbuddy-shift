import { useState } from "react";
import { ChevronDown, FileDown, FileKey2, FileUp, LockKeyhole } from "lucide-react";

import { DemoAction } from "@/components/demo-action";
import { EncryptedExportDialog } from "@/components/encrypted-export-dialog";
import { EncryptedImportDialog } from "@/components/encrypted-import-dialog";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import * as api from "@/lib/api";
import type { ImportResult } from "@/lib/types";

interface Props {
  /** 当前档位下的账号数：为 0 时「导出」与「加密导出…」同步禁用。 */
  accountCount: number;
  /** 打开明文导入弹窗（由账号页持有）。 */
  onImport: () => void;
  /** 打开明文导出弹窗（由账号页持有）。 */
  onExport: () => void;
  /** 加密导入完成后的回调（页面据此重拉账号列表）。 */
  onImported?: (report: ImportResult) => void;
}

/** 分体按钮里 ▾ 的公共外观：约 24px 宽，负左边距贴住主按钮，形成「一组」的读法。 */
const SPLIT_CARET_CLASS =
  "-ml-1 h-9 w-6 rounded-md px-0 text-muted-foreground data-[state=open]:bg-accent data-[state=open]:text-accent-foreground";

/**
 * 账号页的备份动作组：`[导入备份 |▾]` 与 `[导出 |▾]`。
 *
 * 为什么合并成一个组件（T07）：明文与加密是**同一件事的两个档位**，此前平铺成 4 个
 * 按钮（导入备份 / 导出 / 加密导入 / 加密导出），在窄屏上把整行挤成两排，且「加密导出」
 * 与「导出」看起来毫无关系。现在加密项收进各自主按钮的 ▾ 菜单里，主按钮行为与改造前
 * 逐字一致（点主体直接打开原弹窗），老用户不需要重新学。
 *
 * 三条硬约束：
 * 1. **WebUI / 演示模式不渲染 ▾**（`showEncrypted === false`）：两条加密命令都要本机绝对
 *    路径，浏览器沙箱里没有等价入口；此时退回改造前的单按钮，行为完全不变。
 * 2. `accountCount === 0` 时「导出」主按钮与「加密导出…」**同步**禁用：两个入口导出的是
 *    同一份账号库，只禁一个会让用户点了菜单项才发现没账号。
 * 3. ▾ 是**独立可聚焦按钮**（各自带 aria-label），Tab 能分别停在主体与 ▾ 上；
 *    它不是主按钮的一部分，键盘用户不需要模拟鼠标点击。
 */
export function AccountBackupActions({ accountCount, onImport, onExport, onImported }: Props) {
  const [importOpen, setImportOpen] = useState(false);
  const [exportOpen, setExportOpen] = useState(false);
  // 加密导入/导出要本机绝对路径（保存/打开对话框产物），WebUI 与演示模式没有等价入口。
  const showEncrypted = api.isDesktop() && !api.isDemoMode();
  const exportDisabled = accountCount === 0;

  return (
    <>
      <div className="flex items-center gap-1">
        <div className="flex items-center gap-1">
          <DemoAction>
            <Button
              variant="ghost"
              size="sm"
              className="h-9 px-2.5"
              onClick={onImport}
              title="从备份文件导入账号"
            >
              <FileUp />
              导入备份
            </Button>
          </DemoAction>
          {showEncrypted ? (
            <DropdownMenu>
              <DropdownMenuTrigger asChild>
                <Button
                  variant="ghost"
                  size="icon"
                  className={SPLIT_CARET_CLASS}
                  aria-label="更多导入方式"
                  title="更多导入方式"
                >
                  <ChevronDown />
                </Button>
              </DropdownMenuTrigger>
              <DropdownMenuContent align="end" className="w-56">
                <DropdownMenuItem onSelect={() => setImportOpen(true)}>
                  <FileKey2 />
                  加密导入…
                </DropdownMenuItem>
              </DropdownMenuContent>
            </DropdownMenu>
          ) : null}
        </div>

        <div className="flex items-center gap-1">
          <DemoAction>
            <Button
              variant="ghost"
              size="sm"
              className="h-9 px-2.5"
              onClick={onExport}
              disabled={exportDisabled}
              title="导出账号备份"
            >
              <FileDown />
              导出
            </Button>
          </DemoAction>
          {showEncrypted ? (
            <DropdownMenu>
              <DropdownMenuTrigger asChild>
                <Button
                  variant="ghost"
                  size="icon"
                  className={SPLIT_CARET_CLASS}
                  aria-label="更多导出方式"
                  title="更多导出方式"
                >
                  <ChevronDown />
                </Button>
              </DropdownMenuTrigger>
              <DropdownMenuContent align="end" className="w-56">
                <DropdownMenuItem
                  disabled={exportDisabled}
                  onSelect={() => {
                    if (exportDisabled) return;
                    setExportOpen(true);
                  }}
                >
                  <LockKeyhole />
                  加密导出…
                </DropdownMenuItem>
              </DropdownMenuContent>
            </DropdownMenu>
          ) : null}
        </div>
      </div>

      <EncryptedExportDialog open={exportOpen} onOpenChange={setExportOpen} accountCount={accountCount} />
      <EncryptedImportDialog open={importOpen} onOpenChange={setImportOpen} onImported={onImported} />
    </>
  );
}
