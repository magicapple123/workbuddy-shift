import { useState } from "react";
import { ChevronDown, ChevronRight, RotateCcw, Trash2 } from "lucide-react";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { DemoAction } from "@/components/demo-action";
import type { StorageTrashEntry, StorageTrashPayload } from "@/lib/types";

import { formatBytes, formatCount, formatTime } from "./format";

interface TrashListProps {
  trash: StorageTrashPayload;
  busy: boolean;
  onRestore: (id: string) => void;
  onPurge: (id: string) => void;
  onPurgeAll: () => void;
}

const STATE_LABELS: Record<string, string> = {
  restorable: "可恢复",
  conflict: "目标已存在",
  missing: "副本缺失",
};

/** 回收站：恢复（拒绝覆盖）与彻底删除（不可逆）。 */
export function TrashList({ trash, busy, onRestore, onPurge, onPurgeAll }: TrashListProps) {
  const [expanded, setExpanded] = useState<string | null>(null);

  if (trash.entries.length === 0) {
    return (
      <div className="rounded-xl border border-dashed border-border/60 px-4 py-10 text-center text-sm text-muted-foreground">
        回收站是空的。清理后的文件会先进入这里，恢复或彻底删除都在这个页签完成。
      </div>
    );
  }

  return (
    <div className="space-y-3">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <span className="text-xs text-muted-foreground">
          {trash.entries.length} 个条目 · {formatBytes(trash.bytes)} / {formatCount(trash.files)}
          （回收站不释放空间，彻底删除才会）
        </span>
        <DemoAction>
          <Button size="sm" variant="outline" className="text-danger" disabled={busy} onClick={onPurgeAll}>
            <Trash2 className="size-3.5" />
            清空回收站
          </Button>
        </DemoAction>
      </div>

      {trash.problems.length > 0 ? (
        <div className="rounded-lg border border-warning-border bg-warning-soft px-3 py-2 text-xs text-warning-foreground">
          {trash.problems.map((problem) => (
            <div key={problem}>{problem}</div>
          ))}
        </div>
      ) : null}

      <div className="space-y-2">
        {trash.entries.map((entry: StorageTrashEntry) => {
          const isOpen = expanded === entry.id;
          return (
            <div key={entry.id} className="overflow-hidden rounded-xl border border-border/60">
              <div className="flex flex-wrap items-center gap-2 bg-muted/30 px-3 py-2">
                <button
                  type="button"
                  className="rounded p-1 text-muted-foreground hover:bg-foreground/5"
                  onClick={() => setExpanded(isOpen ? null : entry.id)}
                  aria-label={isOpen ? "收起" : "展开"}
                  aria-expanded={isOpen}
                >
                  {isOpen ? <ChevronDown className="size-4" /> : <ChevronRight className="size-4" />}
                </button>
                <span className="font-mono text-xs">{entry.id}</span>
                <span className="text-[11px] text-muted-foreground">{formatTime(entry.createdAt)}</span>
                <Badge variant="secondary" className="text-[10px]">
                  {entry.items.length} 项 · {formatBytes(entry.bytes)}
                </Badge>
                {entry.rows > 0 ? (
                  <Badge variant="secondary" className="text-[10px]">
                    数据库行 {entry.rows}
                  </Badge>
                ) : null}
                <div className="ml-auto flex gap-1">
                  <DemoAction>
                    <Button size="sm" variant="outline" disabled={busy} onClick={() => onRestore(entry.id)}>
                      <RotateCcw className="size-3.5" />
                      恢复
                    </Button>
                  </DemoAction>
                  <DemoAction>
                    <Button
                      size="sm"
                      variant="ghost"
                      className="text-danger"
                      disabled={busy}
                      onClick={() => onPurge(entry.id)}
                    >
                      <Trash2 className="size-3.5" />
                      彻底删除
                    </Button>
                  </DemoAction>
                </div>
              </div>

              {isOpen ? (
                <div className="max-h-64 overflow-y-auto border-t border-border/50 bg-background px-3 py-2">
                  {entry.items.map((item) => (
                    <div
                      key={`${item.originalPath}-${item.category}`}
                      className="flex flex-wrap items-center gap-2 border-b border-border/30 py-1 text-[11px] last:border-0"
                    >
                      <span className="w-24 shrink-0 text-muted-foreground">{item.category}</span>
                      <span className="min-w-0 flex-1 break-all font-mono">{item.originalPath}</span>
                      <span className="w-16 shrink-0 text-right tabular-nums">{formatBytes(item.bytes)}</span>
                      <span className="w-20 shrink-0 text-right text-muted-foreground">
                        {STATE_LABELS[item.state] ?? item.state}
                      </span>
                    </div>
                  ))}
                  {entry.dbBackups.length > 0 ? (
                    <div className="mt-2 space-y-0.5 border-t border-border/40 pt-2 text-[11px] text-muted-foreground">
                      <div>删除数据库行前的整库备份：</div>
                      {entry.dbBackups.map((path) => (
                        <div key={path} className="break-all font-mono">
                          {path}
                        </div>
                      ))}
                    </div>
                  ) : null}
                </div>
              ) : null}
            </div>
          );
        })}
      </div>
    </div>
  );
}
