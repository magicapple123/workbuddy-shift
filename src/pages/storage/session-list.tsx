import { useMemo, useState } from "react";
import { ChevronDown, ChevronRight, FolderSearch, Trash2 } from "lucide-react";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import { DemoAction } from "@/components/demo-action";
import type { StorageEntryKind, StorageSession } from "@/lib/types";
import { cn } from "@/lib/utils";

import {
  PRESENCE_LABELS,
  PRESENCE_TONES,
  ROW_STATUS_LABELS,
  formatBytes,
  formatCount,
  formatTime,
} from "./format";

interface SessionListProps {
  sessions: StorageSession[];
  clientRunning: boolean;
  busy: boolean;
  /** 已按外部校验刷新的 presence（外部删除后 focus 重校验的结果）。 */
  presenceOverrides: Record<string, string>;
  onClean: (sessionId: string, entries: StorageEntryKind[], deleteRow: boolean) => void;
  onReveal: (path: string, mode: "select" | "open") => void;
}

type StatusFilter = "all" | "live" | "softDeleted" | "orphan" | "redundant" | "missing";

/** 会话资产：可展开逐文件勾选；删除整条会话会同时删数据库行。 */
export function SessionList({
  sessions,
  clientRunning,
  busy,
  presenceOverrides,
  onClean,
  onReveal,
}: SessionListProps) {
  const [query, setQuery] = useState("");
  const [filter, setFilter] = useState<StatusFilter>("all");
  const [expanded, setExpanded] = useState<string | null>(null);
  const [selection, setSelection] = useState<Record<string, StorageEntryKind[]>>({});

  const visible = useMemo(() => {
    const text = query.trim().toLowerCase();
    return sessions.filter((session) => {
      const presence = presenceOverrides[session.id] ?? session.presence;
      if (filter === "live" && session.rowStatus !== "live") return false;
      if (filter === "softDeleted" && session.rowStatus !== "softDeleted") return false;
      if (filter === "orphan" && presence !== "orphan") return false;
      if (filter === "redundant" && !session.redundant) return false;
      if (filter === "missing" && presence === "present") return false;
      if (!text) return true;
      return (
        session.title.toLowerCase().includes(text) ||
        session.id.toLowerCase().includes(text) ||
        session.cwd.toLowerCase().includes(text)
      );
    });
  }, [sessions, query, filter, presenceOverrides]);

  const shown = visible.slice(0, 200);

  function toggleEntry(sessionId: string, kind: StorageEntryKind) {
    setSelection((current) => {
      const picked = current[sessionId] ?? [];
      const next = picked.includes(kind)
        ? picked.filter((item) => item !== kind)
        : [...picked, kind];
      return { ...current, [sessionId]: next };
    });
  }

  return (
    <div className="space-y-3">
      <div className="flex flex-wrap items-center gap-2">
        <Input
          value={query}
          onChange={(event) => setQuery(event.target.value)}
          placeholder="搜索标题 / 会话 id / 工作目录"
          className="h-9 w-full max-w-xs"
        />
        <div className="flex flex-wrap gap-1">
          {(
            [
              ["all", "全部"],
              ["live", "存活"],
              ["softDeleted", "软删除"],
              ["orphan", "孤儿"],
              ["redundant", "冗余副本"],
              ["missing", "不完整"],
            ] as [StatusFilter, string][]
          ).map(([value, label]) => (
            <Button
              key={value}
              size="sm"
              variant={filter === value ? "secondary" : "ghost"}
              onClick={() => setFilter(value)}
            >
              {label}
            </Button>
          ))}
        </div>
        <span className="text-xs text-muted-foreground">
          共 {visible.length} 条{shown.length < visible.length ? `，显示前 ${shown.length} 条` : ""}
        </span>
      </div>

      {clientRunning ? (
        <div className="rounded-lg border border-warning-border bg-warning-soft px-3 py-2 text-xs text-warning-foreground">
          客户端正在运行：会话清理会被拒绝。请先退出 WorkBuddy 客户端再操作（单文件勾选同样要求退出）。
        </div>
      ) : null}

      <div className="overflow-hidden rounded-xl border border-border/60">
        <table className="w-full text-sm">
          <thead className="bg-muted/40 text-[11px] text-muted-foreground">
            <tr>
              <th className="w-8 px-2 py-2" />
              <th className="px-3 py-2 text-left font-medium">会话</th>
              <th className="px-3 py-2 text-left font-medium">状态</th>
              <th className="px-3 py-2 text-right font-medium">体积</th>
              <th className="px-3 py-2 text-right font-medium">操作</th>
            </tr>
          </thead>
          <tbody>
            {shown.map((session) => {
              const presence = presenceOverrides[session.id] ?? session.presence;
              const isOpen = expanded === session.id;
              const picked = selection[session.id] ?? [];
              return (
                <tr key={session.id} className="border-t border-border/50 align-top">
                  <td className="px-2 py-2.5">
                    <button
                      type="button"
                      className="rounded p-1 text-muted-foreground hover:bg-foreground/5"
                      onClick={() => setExpanded(isOpen ? null : session.id)}
                      aria-label={isOpen ? "收起" : "展开"}
                      aria-expanded={isOpen}
                    >
                      {isOpen ? (
                        <ChevronDown className="size-4" />
                      ) : (
                        <ChevronRight className="size-4" />
                      )}
                    </button>
                  </td>
                  <td className="px-3 py-2.5">
                    <div className="flex flex-wrap items-center gap-1.5 font-medium">
                      {session.title}
                      {session.redundant ? (
                        <Badge variant="secondary" className="text-[10px]">
                          冗余 {session.liveCopies} 份
                        </Badge>
                      ) : null}
                    </div>
                    <div className="mt-0.5 break-all text-[11px] text-muted-foreground">
                      {formatTime(session.updatedAt)} · {session.cwd || "（无工作目录）"}
                    </div>
                    <div className="break-all font-mono text-[10px] text-muted-foreground/70">
                      {session.id}
                    </div>

                    {isOpen ? (
                      <div className="mt-2 space-y-1 rounded-lg border border-border/60 bg-muted/20 p-2">
                        {session.entries.map((entry) => (
                          <label
                            key={entry.kind}
                            className={cn(
                              "flex items-center gap-2 rounded px-1 py-1 text-xs",
                              entry.exists ? "cursor-pointer hover:bg-foreground/5" : "opacity-50",
                            )}
                          >
                            <Checkbox
                              checked={picked.includes(entry.kind)}
                              disabled={!entry.exists || busy}
                              onCheckedChange={() => toggleEntry(session.id, entry.kind)}
                            />
                            <span className="w-20 shrink-0">{entry.label}</span>
                            <span className="w-16 shrink-0 text-right tabular-nums text-muted-foreground">
                              {entry.exists ? formatBytes(entry.bytes) : "缺失"}
                            </span>
                            <span className="min-w-0 flex-1 truncate font-mono text-[10px] text-muted-foreground/70">
                              {entry.path}
                            </span>
                            {entry.exists ? (
                              <button
                                type="button"
                                className="text-muted-foreground hover:text-foreground"
                                title="在文件夹中显示"
                                onClick={(event) => {
                                  event.preventDefault();
                                  onReveal(entry.path, "select");
                                }}
                              >
                                <FolderSearch className="size-3.5" />
                              </button>
                            ) : null}
                          </label>
                        ))}
                        <div className="flex flex-wrap items-center gap-2 pt-1">
                          <Button
                            size="sm"
                            variant="outline"
                            disabled={busy || picked.length === 0}
                            onClick={() => onClean(session.id, picked, false)}
                          >
                            <Trash2 className="size-3.5" />
                            删除选中子项（{picked.length}）
                          </Button>
                          <span className="text-[11px] text-muted-foreground">
                            删正文 → 正文缺失；删其它项 → 部分缺失；都不会自动删数据库行
                          </span>
                        </div>
                      </div>
                    ) : null}
                  </td>
                  <td className="whitespace-nowrap px-3 py-2.5">
                    <div className="flex flex-col gap-1">
                      <span
                        className={cn(
                          "inline-flex w-fit items-center rounded-full border px-2 py-0.5 text-[10px]",
                          PRESENCE_TONES[presence] ?? "",
                        )}
                      >
                        {PRESENCE_LABELS[presence] ?? presence}
                      </span>
                      <span className="text-[10px] text-muted-foreground">
                        {ROW_STATUS_LABELS[session.rowStatus] ?? session.rowStatus}
                      </span>
                    </div>
                  </td>
                  <td className="whitespace-nowrap px-3 py-2.5 text-right text-xs tabular-nums">
                    <div>{formatBytes(session.bytes)}</div>
                    <div className="text-[10px] text-muted-foreground">
                      {formatCount(session.files)}
                    </div>
                  </td>
                  <td className="whitespace-nowrap px-3 py-2.5 text-right">
                    <div className="flex flex-col items-end gap-1">
                      <DemoAction>
                        <Button
                          size="sm"
                          variant="ghost"
                          disabled={busy}
                          onClick={() => onReveal(session.cwd || session.entries[0]?.path || "", "open")}
                        >
                          <FolderSearch className="size-3.5" />
                          工作目录
                        </Button>
                      </DemoAction>
                      <DemoAction>
                        <Button
                          size="sm"
                          variant="outline"
                          className="text-danger"
                          disabled={busy || session.rowStatus === "none"}
                          title={
                            session.rowStatus === "none"
                              ? "无数据库行，只能删除残留文件"
                              : "删除数据库行 + 全部资产 + 关联成员"
                          }
                          onClick={() => onClean(session.id, [], true)}
                        >
                          <Trash2 className="size-3.5" />
                          删除会话
                        </Button>
                      </DemoAction>
                    </div>
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      </div>
    </div>
  );
}
