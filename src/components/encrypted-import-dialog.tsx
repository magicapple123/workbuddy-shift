import { useEffect, useState } from "react";
import { FileUp, Loader2 } from "lucide-react";
import { toast } from "sonner";

import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import * as api from "@/lib/api";
import { MIN_PASSWORD_LENGTH, importSummary, submitEncryptedImport } from "@/lib/encrypted-backup";
import type { ImportResult } from "@/lib/types";

interface Props {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** 导入完成回调（后端返回的新增/覆盖/跳过计数），由页面用来刷新账号列表。 */
  onImported?: (report: ImportResult) => void;
}

/**
 * 加密导入弹框（issue #186）：选加密备份文件 → 输入口令 → 后端解密并合并。
 *
 * 与「导入备份」的区别：加密文件是**整份**账号集，因此没有预览勾选环节，
 * 合并语义完全一样（按 uid 去重覆盖、保留本地展示字段、缺 id 补齐）。
 *
 * 口令输入框 `type="password"`，不回显；提交中禁用按钮。口令不足 8 位时
 * `submitEncryptedImport` 直接返回 `invalid`，不会发起任何请求。
 */
export function EncryptedImportDialog({ open, onOpenChange, onImported }: Props) {
  const [filePath, setFilePath] = useState("");
  const [password, setPassword] = useState("");
  const [busy, setBusy] = useState(false);
  const [picking, setPicking] = useState(false);
  const [error, setError] = useState("");
  const [report, setReport] = useState<ImportResult | null>(null);

  useEffect(() => {
    if (open) {
      setFilePath("");
      setPassword("");
      setBusy(false);
      setPicking(false);
      setError("");
      setReport(null);
    }
  }, [open]);

  async function chooseFile() {
    if (busy || picking) return;
    setPicking(true);
    setError("");
    try {
      const path = await api.openFileDialog({
        title: "选择加密备份",
        multiple: false,
        directory: false,
        filters: [{ name: "加密备份", extensions: ["json"] }],
      });
      if (path) {
        setFilePath(path);
        // 换文件就清掉上一次的导入摘要，避免"新选的文件 + 旧结果"同时展示。
        setReport(null);
      }
    } catch (e) {
      setError(api.asError(e));
    } finally {
      setPicking(false);
    }
  }

  async function doImport() {
    if (busy || !filePath) return;
    setBusy(true);
    setError("");
    setReport(null);
    try {
      const outcome = await submitEncryptedImport(password, {
        // 文件已在上一步选定：这里只是把已有路径交给流程，保证"校验没过不发请求"仍成立。
        pickPath: async () => filePath || null,
        run: (path, secret) => api.importAccountsEncrypted(path, secret),
      });
      switch (outcome.status) {
        case "invalid":
          setError(outcome.error);
          return;
        case "canceled":
          return;
        case "failed":
          setError(api.asError(outcome.error));
          return;
        case "done": {
          setReport(outcome.report);
          setPassword("");
          toast.success("导入成功", { description: importSummary(outcome.report) });
          onImported?.(outcome.report);
          return;
        }
      }
    } catch (e) {
      setError(api.asError(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="min-w-0 overflow-x-hidden">
        <DialogHeader>
          <DialogTitle>导入加密备份</DialogTitle>
          <DialogDescription>
            选择本工具导出的加密备份文件，输入导出时设置的口令。
          </DialogDescription>
        </DialogHeader>

        <div className="space-y-2">
          <div className="flex items-center gap-2">
            <Button variant="outline" onClick={() => void chooseFile()} disabled={busy || picking}>
              {picking ? <Loader2 className="animate-spin" /> : <FileUp />}
              选择文件
            </Button>
            {filePath && <span className="truncate text-xs text-muted-foreground">{filePath}</span>}
          </div>

          <Label htmlFor="encrypted-import-password">备份口令</Label>
          <Input
            id="encrypted-import-password"
            type="password"
            autoComplete="current-password"
            value={password}
            onChange={(e) => setPassword(e.target.value)}
            placeholder={`至少 ${MIN_PASSWORD_LENGTH} 位`}
            disabled={busy}
          />
          <p className="text-xs leading-5 text-muted-foreground">
            口令只用于本机解密，不会上传。口令错误或文件损坏时不会导入任何账号。
          </p>
        </div>

        {error && (
          <Alert variant="destructive">
            <AlertDescription>{error}</AlertDescription>
          </Alert>
        )}

        {report && (
          <Alert>
            <AlertTitle>导入完成</AlertTitle>
            <AlertDescription className="space-y-1">
              <span className="block">{importSummary(report)}</span>
              <span className="block text-xs">token 可能已过期，切换后可能需要重新登录。</span>
            </AlertDescription>
          </Alert>
        )}

        <DialogFooter>
          <Button variant="outline" onClick={() => onOpenChange(false)} disabled={busy}>
            {report ? "完成" : "取消"}
          </Button>
          {!report && (
            <Button onClick={() => void doImport()} disabled={busy || !filePath}>
              {busy ? <Loader2 className="animate-spin" /> : null}
              {busy ? "解密中…" : "导入"}
            </Button>
          )}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
