import { FolderSearch, ShieldAlert, Trash2 } from "lucide-react";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { DemoAction } from "@/components/demo-action";
import type { StorageCategory } from "@/lib/types";

import { formatBytes, formatCount } from "./format";

interface CategoryTableProps {
  categories: StorageCategory[];
  busy: boolean;
  onClean: (category: StorageCategory) => void;
  onReveal: (path: string, mode: "select" | "open") => void;
}

const GROUP_LABELS: Record<string, string> = {
  tool: "工具数据（~/.workbuddy-shift）",
  client: "客户端缓存（~/.workbuddy）",
};

/** 分类明细：体积 / 文件数 / 可清理单元数 + 单项清理入口。 */
export function CategoryTable({ categories, busy, onClean, onReveal }: CategoryTableProps) {
  const groups = ["tool", "client"] as const;

  return (
    <div className="space-y-6">
      {groups.map((group) => {
        const rows = categories.filter((category) => category.group === group);
        if (rows.length === 0) return null;
        return (
          <section key={group} className="space-y-2">
            <h2 className="text-xs font-semibold uppercase tracking-wide text-muted-foreground">
              {GROUP_LABELS[group]}
            </h2>
            <div className="overflow-hidden rounded-xl border border-border/60">
              <table className="w-full text-sm">
                <thead className="bg-muted/40 text-[11px] text-muted-foreground">
                  <tr>
                    <th className="px-3 py-2 text-left font-medium">分类</th>
                    <th className="px-3 py-2 text-right font-medium">可清理体积</th>
                    <th className="px-3 py-2 text-right font-medium">文件</th>
                    <th className="px-3 py-2 text-right font-medium">操作</th>
                  </tr>
                </thead>
                <tbody>
                  {rows.map((category) => (
                    <tr key={category.id} className="border-t border-border/50 align-top">
                      <td className="px-3 py-2.5">
                        <div className="flex items-center gap-2 font-medium">
                          {category.label}
                          {category.deletable ? null : (
                            <Badge variant="secondary" className="gap-1 text-[10px]">
                              <ShieldAlert className="size-3" />
                              受保护
                            </Badge>
                          )}
                        </div>
                        <div className="mt-0.5 break-all text-[11px] leading-4 text-muted-foreground">
                          {category.note}
                        </div>
                        <div className="mt-0.5 break-all font-mono text-[10px] text-muted-foreground/70">
                          {category.path}
                        </div>
                      </td>
                      <td className="whitespace-nowrap px-3 py-2.5 text-right tabular-nums">
                        <div>{formatBytes(category.bytes)}</div>
                        <div className="text-[10px] text-muted-foreground">
                          {category.deletable ? `${category.units} 个单元` : "不可删"}
                        </div>
                      </td>
                      <td className="whitespace-nowrap px-3 py-2.5 text-right text-xs tabular-nums text-muted-foreground">
                        {category.files > 0 ? formatCount(category.files) : "—"}
                      </td>
                      <td className="whitespace-nowrap px-3 py-2.5 text-right">
                        <div className="flex justify-end gap-1">
                          <DemoAction>
                            <Button
                              size="sm"
                              variant="ghost"
                              title="在文件夹中显示"
                              onClick={() => onReveal(category.path, "open")}
                            >
                              <FolderSearch className="size-3.5" />
                            </Button>
                          </DemoAction>
                          <DemoAction>
                            <Button
                              size="sm"
                              variant="outline"
                              disabled={!category.deletable || busy || category.units === 0}
                              title={
                                category.deletable
                                  ? `清理 ${category.units} 个单元`
                                  : category.protected ?? "不可直接删除"
                              }
                              onClick={() => onClean(category)}
                            >
                              <Trash2 className="size-3.5" />
                              清理
                            </Button>
                          </DemoAction>
                        </div>
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          </section>
        );
      })}
    </div>
  );
}
