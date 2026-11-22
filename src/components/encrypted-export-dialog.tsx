import { useEffect, useState } from "react";
import { Download, Loader2 } from "lucide-react";
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
import { MIN_PASSWORD_LENGTH, submitEncryptedExport } from "@/lib/encrypted-backup";

interface Props {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** 将被导出的账号数（只用于文案：实际导出的是账号库里的全部记录）。 */
  accountCount: number;
  /** 导出完成回调（参数为保存路径与账号数），由页面用来刷新/提示。 */
  onExported?: (path: string, count: number) => void;
}

/** 加密备份文件名：workbuddy-shift-accounts-YYYY-MM-DD-encrypted.json */
function encryptedFileName(): string {
  const d = new Date();
  const pad = (n: number) => String(n).padStart(2, "0");
  return `workbuddy-shift-accounts-${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}-encrypted.json`;
}

/**
 * 加密导出弹框（issue #186）：口令 + 二次确认 → 保存对话框选路径 → 后端加密写入。
 *
 * 三条硬约束（都落在 `submitEncryptedExport` 里，不在 onClick 手写）：
 * 1. 口令不足 8 位或两次不一致 → 直接拦下，**不发请求、不弹对话框**；
 * 2. 输入框 `type="password"`，不做明文回显；
 * 3. 提交中禁用按钮，避免连点产生两次加密写盘。
 *
 * 与明文导出的区别只在"文件内容是信封"：路径选择、原子写、成功提示同一套交互。
 */
export function EncryptedExportDialog({ open, onOpenChange, accountCount, onExported }: Props) {
  const [password, setPassword] = useState("");
  const [confirm, setConfirm] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  /** 导出成功后的落盘信息（成功后按钮换成「完成」，不再重复提交）。 */
  const [saved, setSaved] = useState<{ path: string; count: number } | null>(null);

  useEffect(() => {
    if (open) {
      setPassword("");
      setConfirm("");
      setBusy(false);
      setError("");
      setSaved(null);
    }
  }, [open]);

  async function doExport() {
    if (busy) return;
    setBusy(true);
    setError("");
    try {
      const outcome = await submitEncryptedExport(password, confirm, {
        pickPath: () =>
          api.saveFileDialog({
            title: "导出加密备份",
            defaultPath: encryptedFileName(),
            filters: [{ name: "加密备份", extensions: ["json"] }],
          }),
        run: (path, secret) => api.exportAccountsEncrypted(path, secret),
      });
      switch (outcome.status) {
        case "invalid":
          setError(outcome.error);
          return;
        case "canceled":
          // 用户在保存对话框里取消：不是错误，什么都不提示。
          return;
        case "failed":
          setError(api.asError(outcome.error));
          return;
        case "done": {
          setSaved({ path: outcome.path, count: outcome.count });
          // 口令用完即清：它没有留在组件状态里的理由。
          setPassword("");
          setConfirm("");
          toast.success(`已加密导出 ${outcome.count} 个账号`, {
            description: "文件已用口令加密，忘记口令将无法恢复。",
          });
          onExported?.(outcome.path, outcome.count);
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
          <DialogTitle>加密导出账号</DialogTitle>
          <DialogDescription>
            用口令把全部 {accountCount} 个账号加密成一个文件，适合上传网盘或发给自己。
          </DialogDescription>
        </DialogHeader>

        <div className="space-y-2">
          <Label htmlFor="encrypted-export-password">设置口令</Label>
          <Input
            id="encrypted-export-password"
            type="password"
            autoComplete="new-password"
            value={password}
            onChange={(e) => setPassword(e.target.value)}
            placeholder={`至少 ${MIN_PASSWORD_LENGTH} 位`}
            disabled={busy}
          />
          <Label htmlFor="encrypted-export-confirm">再输入一次</Label>
          <Input
            id="encrypted-export-confirm"
            type="password"
            autoComplete="new-password"
            value={confirm}
            onChange={(e) => setConfirm(e.target.value)}
            placeholder="两次输入必须一致"
            disabled={busy}
          />
          <p className="text-xs leading-5 text-muted-foreground">
            口令不会被保存，也不会出现在导出文件里；忘记口令后这个文件无法恢复。
          </p>
        </div>

        {accountCount === 0 && (
          <Alert variant="warning">
            <AlertDescription>暂无账号可导出。</AlertDescription>
          </Alert>
        )}

        {error && (
          <Alert variant="destructive">
            <AlertDescription>{error}</AlertDescription>
          </Alert>
        )}

        {saved && (
          <Alert>
            <AlertTitle>导出成功（{saved.count} 个账号）</AlertTitle>
            <AlertDescription className="space-y-1">
              <span className="block break-all font-mono text-xs">{saved.path}</span>
              <span className="block text-xs">文件已加密，导入时需要同一个口令。</span>
            </AlertDescription>
          </Alert>
        )}

        <DialogFooter>
          <Button variant="outline" onClick={() => onOpenChange(false)} disabled={busy}>
            {saved ? "完成" : "取消"}
          </Button>
          {!saved && (
            <Button onClick={() => void doExport()} disabled={busy || accountCount === 0}>
              {busy ? <Loader2 className="animate-spin" /> : <Download />}
              {busy ? "加密中…" : "选择位置并导出"}
            </Button>
          )}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
