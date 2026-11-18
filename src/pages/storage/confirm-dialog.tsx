import { AlertTriangle, Loader2 } from "lucide-react";

import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import type { StorageCleanPlan } from "@/lib/types";

import { formatBytes, formatCount } from "./format";

interface ConfirmCleanDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  plan: StorageCleanPlan | null;
  busy: boolean;
  onConfirm: () => void;
}

/**
 * 清理确认：先展示 dry-run 计划（条目数、体积、将删除的数据库行），再执行。
 *
 * 计划由服务端给出；页面不预演第二遍，避免「确认的内容」与「实际删除的内容」不一致。
 */
export function ConfirmCleanDialog({
  open,
  onOpenChange,
  plan,
  busy,
  onConfirm,
}: ConfirmCleanDialogProps) {
  const preview = plan?.items.slice(0, 12) ?? [];
  const more = (plan?.items.length ?? 0) - preview.length;

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-2xl">
        <DialogHeader>
          <DialogTitle>确认清理</DialogTitle>
          <DialogDescription>
            文件会移入回收站（可恢复）；勾选删除数据库行的会话，恢复时会按原 id 重新插入。
          </DialogDescription>
        </DialogHeader>

        {plan ? (
          <div className="space-y-3 text-sm">
            <div className="flex flex-wrap gap-x-4 gap-y-1 text-xs text-muted-foreground">
              <span>目标：{plan.labels.join("、")}</span>
              <span>
                {plan.items.length} 项 · {formatBytes(plan.bytes)} · {formatCount(plan.files)}
              </span>
              {plan.rows.length > 0 ? (
                <span className="text-danger">将删除 {plan.rows.length} 条数据库行</span>
              ) : null}
            </div>

            <div className="max-h-56 overflow-y-auto rounded-lg border border-border/60 bg-muted/30 p-2 font-mono text-[11px] leading-5">
              {preview.length === 0 ? (
                <div className="px-1 text-muted-foreground">没有实际存在的文件（可能已被外部删除）</div>
              ) : (
                preview.map((item) => (
                  <div key={item.path} className="break-all px-1">
                    {item.path}
                  </div>
                ))
              )}
              {more > 0 ? <div className="px-1 text-muted-foreground">…另有 {more} 项</div> : null}
            </div>

            {plan.warnings.length > 0 ? (
              <div className="flex gap-2 rounded-lg border border-warning-border bg-warning-soft p-3 text-xs leading-5 text-warning-foreground">
                <AlertTriangle className="mt-0.5 size-4 shrink-0" />
                <div className="space-y-1">
                  {plan.warnings.map((warning) => (
                    <div key={warning}>{warning}</div>
                  ))}
                </div>
              </div>
            ) : null}
          </div>
        ) : (
          <div className="flex items-center gap-2 py-6 text-sm text-muted-foreground">
            <Loader2 className="size-4 animate-spin" /> 正在计算清理计划…
          </div>
        )}

        <DialogFooter>
          <Button variant="outline" onClick={() => onOpenChange(false)} disabled={busy}>
            取消
          </Button>
          <Button onClick={onConfirm} disabled={busy || !plan || plan.items.length === 0}>
            {busy ? <Loader2 className="size-4 animate-spin" /> : null}
            确认清理
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
