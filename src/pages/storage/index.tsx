import { AlertTriangle, Loader2, RefreshCw } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { DemoAction } from "@/components/demo-action";
import type { StorageEntryKind } from "@/lib/types";

import { CategoryTable } from "./category-table";
import { ConfirmCleanDialog } from "./confirm-dialog";
import { StorageOverview } from "./overview";
import { SessionList } from "./session-list";
import { TrashList } from "./trash-list";
import { useStoragePage } from "./use-storage-page";

/**
 * 存储管理页：看清体积构成、按分类 / 会话清理、回收站恢复、资源管理器定位。
 *
 * 破坏性动作统一「dry-run → 确认 → 执行」；客户端运行时会话类清理被服务端拒绝，
 * 页面只展示原因，不做前端二次判定。
 */
export default function StoragePage() {
  const page = useStoragePage();

  return (
    <div className="mx-auto w-full max-w-6xl px-4 py-6 sm:px-6 sm:py-8">
      <header className="mb-6 flex flex-wrap items-start justify-between gap-3">
        <div>
          <h1 className="text-2xl font-semibold tracking-tight">存储管理</h1>
          <p className="mt-2 max-w-2xl text-sm leading-6 text-muted-foreground">
            看清工具数据、客户端缓存与会话资产包的体积，逐项清理；删除先进回收站，可恢复。
            外部删除文件后切回本页会自动重校验状态。
          </p>
        </div>
        <div className="flex items-center gap-2">
          <DemoAction>
            <Button variant="outline" size="sm" onClick={() => void page.refresh()} disabled={page.loading}>
              <RefreshCw className={page.loading ? "size-4 animate-spin" : "size-4"} />
              重新扫描
            </Button>
          </DemoAction>
        </div>
      </header>

      {page.error ? (
        <div className="mb-4 flex gap-2 rounded-xl border border-danger-border bg-danger-soft p-3 text-sm text-danger">
          <AlertTriangle className="mt-0.5 size-4 shrink-0" />
          <div>
            <div className="font-medium">读取存储信息失败</div>
            <div className="text-xs">{page.error}</div>
          </div>
        </div>
      ) : null}

      {page.loading && !page.scan ? (
        <div className="flex items-center gap-2 py-16 text-sm text-muted-foreground">
          <Loader2 className="size-4 animate-spin" /> 正在扫描（首次可能需要几秒）…
        </div>
      ) : null}

      {page.scan ? (
        <div className="space-y-6">
          <StorageOverview scan={page.scan} />

          <Tabs defaultValue="categories">
            <TabsList>
              <TabsTrigger value="categories">分类明细</TabsTrigger>
              <TabsTrigger value="sessions">
                会话资产（{page.scan.sessions.counts.live + page.scan.sessions.counts.softDeleted + page.scan.sessions.counts.orphan}）
              </TabsTrigger>
              <TabsTrigger value="trash">回收站（{page.scan.trash.entries}）</TabsTrigger>
            </TabsList>

            <TabsContent value="categories" className="pt-4">
              <CategoryTable
                categories={page.scan.categories}
                busy={page.busy}
                onClean={(category) => void page.previewClean([{ kind: "category", id: category.id }])}
                onReveal={(path, mode) => void page.reveal(path, mode)}
              />
            </TabsContent>

            <TabsContent value="sessions" className="pt-4">
              <SessionList
                sessions={page.sessions}
                clientRunning={page.scan.clientRunning}
                busy={page.busy}
                presenceOverrides={page.presence}
                onClean={(sessionId, entries: StorageEntryKind[], deleteRow) =>
                  void page.previewClean([
                    { kind: "session", id: sessionId, entries, deleteRow },
                  ])
                }
                onReveal={(path, mode) => void page.reveal(path, mode)}
              />
            </TabsContent>

            <TabsContent value="trash" className="pt-4">
              {page.trash ? (
                <TrashList
                  trash={page.trash}
                  busy={page.busy}
                  onRestore={(id) => void page.restore(id)}
                  onPurge={(id) => void page.purge(id)}
                  onPurgeAll={() => void page.purgeAll()}
                />
              ) : null}
            </TabsContent>
          </Tabs>

          <div className="space-y-1 border-t border-border/50 pt-4 text-[11px] leading-5 text-muted-foreground">
            {page.scan.warnings.map((warning) => (
              <div key={warning}>· {warning}</div>
            ))}
            <div>
              · 受保护分类（运行时、插件、会话正文目录、图片附件）不提供整目录删除；会话正文请在
              「会话资产」页签逐条处理。
            </div>
            {page.lastRevalidatedAt ? (
              <div>· 最近一次外部删除校验：{new Date(page.lastRevalidatedAt).toLocaleTimeString("zh-CN")}</div>
            ) : null}
          </div>
        </div>
      ) : null}

      <ConfirmCleanDialog
        open={page.dialogOpen}
        onOpenChange={(open) => {
          if (!open) page.closeDialog();
        }}
        plan={page.plan}
        busy={page.busy}
        onConfirm={() => void page.confirmClean()}
      />
    </div>
  );
}
