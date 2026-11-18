import { useEffect, useLayoutEffect, useState } from "react";
import { CircleAlert, Loader2 } from "lucide-react";

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
import { DialogAutoHeight } from "@/components/ui/dialog-auto-height";
import { Skeleton } from "@/components/ui/skeleton";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { SessionSyncSection, type SessionLinksMeta } from "@/components/session-sync-section";
import { LIST_MIN_H } from "@/components/session-link-shared";
import { SessionTreeList } from "@/components/session-tree";
import { SwitchAccountPermissionAlert } from "@/components/switch-account-dialog/permission-alert";
import { notifySwitchResult } from "@/components/switch-account-dialog/result-notice";
import { TAB_TRIGGER_CLASS, tabCount } from "@/components/switch-account-tabs";
import * as api from "@/lib/api";
import { displayName } from "@/lib/account-display";
import { toggleGroupSelection, toggleSessionSelection, toggleSetKey } from "@/lib/session-selection";
import { switchAccountSummary } from "@/lib/switch-account-summary";
import { useAuthPermissionGuard } from "@/hooks/use-auth-permission-guard";
import { useSwitchAccountProgress } from "@/hooks/use-switch-account-progress";
import { accountVariant, variantAppName } from "@/lib/variant";
import type {
  AccountMeta,
  Session,
  SessionLinkPreviewGroup,
  SessionSyncSelection,
} from "@/lib/types";

interface Props {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** 目标账号 */
  account: AccountMeta | null;
  /** 切换完成后刷新列表 */
  onDone?: () => void;
}

/** 切换账号弹窗：可勾选当前账号的会话复制到目标账号（路径 B）。 */
export function SwitchAccountDialog({ open, onOpenChange, account, onDone }: Props) {
  const [sessions, setSessions] = useState<Session[]>([]);
  const [loadingSessions, setLoadingSessions] = useState(false);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  /** 展开的节点：任务 / 空间 / 文件夹。默认全部收起。 */
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [currentUid, setCurrentUid] = useState<string | null>(null);
  const [progress, setProgress] = useSwitchAccountProgress();
  /** 会话同步：勾选结果（默认值来自后端 defaultChecked）与预览组（结果反馈回显用）。 */
  const [syncSelections, setSyncSelections] = useState<SessionSyncSelection[]>([]);
  const [syncGroups, setSyncGroups] = useState<SessionLinkPreviewGroup[]>([]);
  /** 当前 tab：默认关联会话；该 tab 不可用时回落到复制会话。 */
  const [tab, setTab] = useState<"links" | "copy">("links");
  /** 关联会话区块上报的状态：tab 徽标与常驻提示用。 */
  const [linksMeta, setLinksMeta] = useState<SessionLinksMeta | null>(null);

  const variant = accountVariant(account);
  const accountId = account?.id;

  // 常驻错误提示里的权限引导：自检、跳系统设置与「无权限」自动轮询都在 hook 内。
  const { permCheck, setPermCheck, runPermissionCheck, openPermissionSettings } = useAuthPermissionGuard({
    open,
    error,
    variant,
    onError: setError,
  });

  // 立即打开弹框；首次绘制前重置旧内容，两个 tab 各自加载数据。
  useLayoutEffect(() => {
    if (!open || !accountId) return;
    let cancelled = false;
    setSelected(new Set());
    setExpanded(new Set());
    setPermCheck(null);
    setError("");
    setSessions([]);
    setCurrentUid(null);
    setSyncSelections([]);
    setSyncGroups([]);
    setLinksMeta(null);
    setTab("links");
    setLoadingSessions(true);
    void api.listSessions(variant)
      .then((res) => {
        if (cancelled) return;
        setSessions(res.sessions);
        setCurrentUid(res.current);
      })
      .catch((e) => {
        if (!cancelled) setError(api.asError(e));
      })
      .finally(() => {
        if (!cancelled) setLoadingSessions(false);
      });
    // 关闭时只取消回写，保留内容到退出动画结束。
    return () => { cancelled = true; };
  }, [open, accountId, variant]);

  function toggleSession(id: string) {
    setSelected((prev) => toggleSessionSelection(prev, id));
  }

  function toggleFolder(ids: string[]) {
    setSelected((prev) => toggleGroupSelection(prev, ids));
  }

  function toggleExpanded(key: string) {
    setExpanded((prev) => toggleSetKey(prev, key));
  }

  async function doSwitch() {
    if (!account) return;
    setBusy(true);
    setProgress("正在切换账号…");
    setError("");
    const requestedCopy = selected.size > 0;
    const requestedSync = syncSelections.length > 0;
    try {
      const res = await api.switchAccount({
        accountId: account.id,
        copySessionIds: requestedCopy ? [...selected] : undefined,
        // 勾选绑定预览凭据；执行前后端会重新校验，版本变化则跳过该项。
        syncSelections: requestedSync ? syncSelections : undefined,
      });
        // 结果提示按复制 / 同步 / 恢复 / 临时备份的顺序逐项提示（与原实现一致）。
        notifySwitchResult({ result: res, account, sessions, syncGroups, requestedCopy, requestedSync });
      onOpenChange(false);
      onDone?.();
    } catch (e) {
      setError(api.asError(e));
    } finally {
      setBusy(false);
      setProgress("");
    }
  }


  const copyCount = selected.size;
  const syncCount = syncSelections.length;
  /** 覆盖项数量：底部摘要据此提示风险。 */
  const overwriteCount = syncSelections.filter((item) => item.mode === "overwrite").length;
  const linksAvailable = linksMeta?.available ?? true;
  const { main: summaryMain, sub: summarySub } = switchAccountSummary({
    copyCount,
    syncCount,
    overwriteCount,
  });
  const needsPermission = error.includes("无权限");
  const sessionsEmpty = !loadingSessions && sessions.length === 0;
  const copyHint = loadingSessions
    ? "正在加载会话…"
    : error && sessionsEmpty
      ? "无法加载会话列表，暂不能复制"
      : sessionsEmpty
        ? currentUid
          ? "当前账号暂无会话，无法复制"
          : "未检测到当前登录账号，无法列出会话"
        : "把当前账号勾选的会话复制给目标账号（复制后归属目标账号）；已经复制过的不会重复复制";

  // 关联 tab 不可用（国际版能力判定不通过）时回落到复制会话，避免停在空 tab。
  useEffect(() => {
    if (!linksAvailable && tab === "links") setTab("copy");
  }, [linksAvailable, tab]);

  // 「复制会话」tab 内容：勾选即意图，提交结果由底部摘要兜底确认。
  const copyTabContent = (
    <>
      {loadingSessions ? (
        // 两行高度对齐真实提示文案（有会话时折成两行）。
        <div className="px-1">
          <Skeleton className="h-4 w-full" />
          <Skeleton className="h-4 w-3/4" />
        </div>
      ) : (
        <p
          className={
            sessionsEmpty
              ? "px-1 text-xs text-warning"
              : "px-1 text-xs text-muted-foreground"
          }
        >
          {copyHint}
        </p>
      )}
      {loadingSessions ? (
        <div
          className={`flex items-center justify-center gap-2 text-sm text-muted-foreground ${LIST_MIN_H}`}
        >
          <Loader2 className="animate-spin" /> 加载会话…
        </div>
      ) : sessions.length === 0 ? (
        <p
          className={`flex items-center justify-center px-3 text-center text-sm text-muted-foreground ${LIST_MIN_H}`}
        >
          {currentUid ? "当前账号暂无会话" : "未检测到当前登录账号，无法列出会话"}
        </p>
      ) : (
        <SessionTreeList
          sessions={sessions}
          selected={selected}
          expanded={expanded}
          onToggleSession={toggleSession}
          onToggleGroup={toggleFolder}
          onToggleExpanded={toggleExpanded}
          className={`max-h-[min(22rem,45vh)] overflow-y-auto pr-1 ${LIST_MIN_H}`}
        />
      )}
    </>
  );

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent
        showCloseButton={!busy}
        className="flex max-h-[min(90vh,calc(100vh-2rem))] min-w-0 flex-col gap-0 overflow-hidden p-0"
      >
        <DialogAutoHeight>
        <div className="flex max-h-[calc(min(90vh,100vh-2rem)-2px)] min-w-0 flex-col gap-4 p-6">
        <DialogHeader className="shrink-0">
          <DialogTitle>切换到「{account ? displayName(account) : "该账号"}」</DialogTitle>
          <DialogDescription>切换时将重启 {variantAppName(accountVariant(account))}。</DialogDescription>
        </DialogHeader>

        {busy && (
          <div className="absolute inset-0 z-50 flex flex-col items-center justify-center gap-3 rounded-lg bg-background/85 backdrop-blur-sm">
            <Loader2 className="size-8 animate-spin text-primary" />
            <p className="text-sm font-medium">{progress || "正在切换账号…"}</p>
            <p className="max-w-xs text-center text-xs text-muted-foreground">
              正在处理中，请勿关闭窗口
            </p>
          </div>
        )}

        {/* min-h-0：矮窗口下压缩本区而不是裁切页脚。列表按自身 max-h 滚动；矮窗口或权限大卡时 tab 内容区可滚，避免裁切。仅当常驻提示超过剩余空间时本层滚动。 */}
        <div className="flex min-h-0 flex-col gap-3 overflow-x-hidden overflow-y-auto">
          {/* 常驻提示区：切换错误 / 权限 / 关联检查失败不随 tab 切换隐藏。 */}
          <SwitchAccountPermissionAlert
            error={error}
            needsPermission={needsPermission}
            permCheck={permCheck}
            onOpenPermissionSettings={openPermissionSettings}
            onRunPermissionCheck={runPermissionCheck}
          />
          {linksMeta?.error && (
            <Alert variant="destructive" className="min-w-0 shrink-0">
              <CircleAlert />
              <AlertTitle>无法检查会话</AlertTitle>
              <AlertDescription className="min-w-0 break-all">{linksMeta.error}</AlertDescription>
            </Alert>
          )}
          {linksMeta?.storeStatus === "unavailable" && (
            <Alert variant="warning" className="min-w-0 shrink-0">
              <CircleAlert />
              <AlertTitle>同步记录不可用</AlertTitle>
              <AlertDescription className="min-w-0 break-all">
                {`${linksMeta.storeError || "原因未知"}；本次不会同步任何会话，请处理后重试。`}
              </AlertDescription>
            </Alert>
          )}

          {linksAvailable ? (
            <Tabs
              value={tab}
              onValueChange={(value) => setTab(value as "links" | "copy")}
              className="flex min-h-0 flex-col gap-3 overflow-hidden"
            >
              <TabsList className="h-auto w-full shrink-0 justify-start gap-5 rounded-none border-b border-border bg-transparent p-0">
                <TabsTrigger value="links" className={TAB_TRIGGER_CLASS}>
                  关联会话
                  {tabCount(linksMeta?.groupCount ?? 0)}
                </TabsTrigger>
                <TabsTrigger value="copy" className={TAB_TRIGGER_CLASS}>
                  复制会话
                  {tabCount(copyCount)}
                </TabsTrigger>
              </TabsList>
              {/* forceMount：切换 tab 不得卸载另一侧，否则关联勾选会被预览重拉重置。 */}
              <TabsContent
                value="links"
                forceMount
                className="min-h-0 overflow-y-auto data-[state=inactive]:hidden data-[state=active]:animate-in data-[state=active]:fade-in-0 data-[state=active]:duration-150"
              >
                <SessionSyncSection
                  open={open}
                  account={account}
                  disabled={busy}
                  // WorkBuddy 侧默认勾选叠加「自动同步归档」偏好（#165 方案 3）。
                  autoArchiveDefault
                  onChange={(state) => {
                    setSyncSelections(state.selections);
                    setSyncGroups(state.groups);
                  }}
                  onMetaChange={setLinksMeta}
                />
              </TabsContent>
              <TabsContent
                value="copy"
                forceMount
                className="min-h-0 space-y-2 overflow-y-auto data-[state=inactive]:hidden data-[state=active]:animate-in data-[state=active]:fade-in-0 data-[state=active]:duration-150"
              >
                {copyTabContent}
              </TabsContent>
            </Tabs>
          ) : (
            <div className="min-h-0 space-y-2 overflow-y-auto">{copyTabContent}</div>
          )}
        </div>

        <DialogFooter className="shrink-0 sm:justify-between">
          <div className="min-w-0 space-y-0.5">
            <div className="text-sm font-medium">{summaryMain}</div>
            {summarySub && <div className="text-xs text-muted-foreground">{summarySub}</div>}
          </div>
          <div className="flex shrink-0 gap-2">
            <Button variant="outline" onClick={() => onOpenChange(false)} disabled={busy}>
              取消
            </Button>
            <Button onClick={doSwitch} disabled={busy}>
              {busy ? "切换中…" : "确认切换"}
            </Button>
          </div>
        </DialogFooter>
        </div>
        </DialogAutoHeight>
      </DialogContent>
    </Dialog>
  );
}
