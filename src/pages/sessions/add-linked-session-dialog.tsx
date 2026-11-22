import { useRef, useState } from "react";
import { Link2, Loader2 } from "lucide-react";
import { toast } from "sonner";
import { DemoAction } from "@/components/demo-action";
import { SessionTreeList } from "@/components/session-tree";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle, DialogTrigger } from "@/components/ui/dialog";
import { displayName } from "@/lib/account-display";
import * as api from "@/lib/api";
import { toggleGroupSelection, toggleSessionSelection, toggleSetKey } from "@/lib/session-selection";
import type {
  AccountMeta,
  Session,
  SessionGroupClient,
} from "@/lib/types";
import { accountVariant, variantLabel } from "@/lib/variant";
import { useAccountsStore } from "@/stores/accounts";
import { LINKED_TREE_MIN_H } from "./constants";

/**
 * 右上角「新增关联会话」弹窗（与切号弹窗同构）：来源账号 → 会话树勾选 → 目标账号，
 * 一次提交全部勾选。会话树复用 `SessionTreeList`（任务平铺 / 空间按文件夹分组 / 组头三态勾选）。
 *
 * 本次仅 WorkBuddy 客户端开放；其他客户端入口禁用（其复制 / 关联能力后续再补）。执行沿用
 * `copySessionsCross`：来源会话已属于某关联组则加入该组，未关联则新建关联组。
 */
export function AddLinkedSessionDialog({ client, disabled, onDone, requestRestartConfirm }: {
  client: SessionGroupClient;
  disabled: boolean;
  onDone: () => void;
  /** 插件侧运行中：请求页面统一的「关闭并重开」确认框；用户确认后执行 `run`。 */
  requestRestartConfirm: (actionLabel: string, run: () => Promise<void>) => void;
}) {
  const accounts = useAccountsStore((state) => state.accounts);
  const [open, setOpen] = useState(false);
  const [sourceAccountId, setSourceAccountId] = useState("");
  const [targetAccountId, setTargetAccountId] = useState("");
  const [sessions, setSessions] = useState<Session[]>([]);
  const [sessionsLoading, setSessionsLoading] = useState(false);
  const [sessionsError, setSessionsError] = useState("");
  /** 插件侧数据目录：`null` = 未找到（与「该账号无会话」区分）；`undefined` = 返回未携带该字段。 */
  const [dataRoot, setDataRoot] = useState<string | null | undefined>(undefined);
  /** 勾选 / 展开集合由调用方持有（会话树契约），执行时一次提交全部勾选。 */
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const [busy, setBusy] = useState(false);
  /**
   * 账本写入失败（可显式忽略）：`count` 是可忽略的会话数。
   * 忽略是**不可逆**选择——副本永久不含用量与中断记录，不做后台补写，所以必须点一次确认。
   */
  const [auxBlocked, setAuxBlocked] = useState<{ count: number } | null>(null);
  /**
   * 「同时复制工作记忆（MEMORY.md）」（issue #209）：**按操作勾选、默认不勾**。
   * 不勾时后端连源文件都不读，报告里也不会出现 `memory` 字段。
   */
  const [copyMemory, setCopyMemory] = useState(false);
  const requestId = useRef(0);
  /** 复制请求代号。关窗换代后，晚到的结果不得再关闭或改写当前这次弹窗。 */
  const copyRequestId = useRef(0);
  // 账号库就是本机 WorkBuddy 账号，国内版 / 国际版都可以作为来源或目标。
  const workbuddyAccounts = accounts;
  const sourceAccount = workbuddyAccounts.find((account) => account.id === sourceAccountId) ?? null;
  const targetAccount = workbuddyAccounts.find((account) => account.id === targetAccountId) ?? null;
  const targetOptions = workbuddyAccounts.filter((account) => account.id !== sourceAccountId);
  const supported = client === "workbuddy" || client === "vscodeExt" || client === "jetbrains";
  const unsupportedTitle = "当前客户端暂不支持新增关联会话";
  /** 宿主名（确认框 / 成功提示里要指名是哪个编辑器）。 */
  const editorLabel = client === "jetbrains" ? "JetBrains IDE" : "VS Code";
  /** 弹窗文案用：来源账号的客户端称呼。 */
  const clientAccountLabel = client === "workbuddy"
    ? "WorkBuddy 账号"
    : client === "jetbrains"
      ? "JetBrains 插件账号"
      : "CodeBuddy 插件账号";

  async function loadSessions(account: AccountMeta) {
    const id = ++requestId.current;
    setSessionsLoading(true);
    setSessionsError("");
    try {
      const result = await api.listAccountSessions(account.id, client);
      if (requestId.current !== id) return;
      // 插件会话没有工作目录概念（只有 workspaceHash）：补空 cwd，交给会话树平铺展示。
      setSessions(result.sessions.map((session) => ({ ...session, cwd: session.cwd ?? "" })));
      setDataRoot(result.dataRoot);
    } catch (error) {
      if (requestId.current !== id) return;
      setSessions([]);
      setSessionsError(api.asError(error));
    } finally {
      if (requestId.current === id) setSessionsLoading(false);
    }
  }
  // 换来源账号：作废在途请求，勾选 / 展开 / 目标账号全部重来，源会话按新账号重新加载。
  function changeSource(id: string) {
    setSourceAccountId(id);
    setTargetAccountId("");
    setSessions([]);
    setSessionsError("");
    setSelected(new Set());
    setExpanded(new Set());
    requestId.current += 1;
    setSessionsLoading(false);
    const account = workbuddyAccounts.find((item) => item.id === id);
    if (account) void loadSessions(account);
  }
  function blockDismissWhileBusy(event: { preventDefault: () => void }) {
    // 遮罩写明执行中不可关闭；只藏关闭按钮挡不住 Esc 和点遮罩。
    if (busy) event.preventDefault();
  }
  function openChange(next: boolean) {
    if (!next) {
      if (busy) return;
      // 关闭时作废在途的会话读取和复制回写：晚到结果不得写回下一次打开。
      requestId.current += 1;
      copyRequestId.current += 1;
      setSessionsLoading(false);
      setOpen(false);
      return;
    }
    // 重新打开回到干净状态：来源 / 目标 / 勾选都重选，不复用上一次的会话列表。
    requestId.current += 1;
    copyRequestId.current += 1;
    setSessionsLoading(false);
    setBusy(false);
    setSourceAccountId("");
    setTargetAccountId("");
    setSessions([]);
    setSessionsError("");
    setSelected(new Set());
    setExpanded(new Set());
    // 换来源账号 = 换一份工作记忆：勾选状态回到默认（不勾），避免误带上一份记忆。
    setCopyMemory(false);
    setOpen(true);
  }
  function toggleSession(id: string) {
    setSelected((prev) => toggleSessionSelection(prev, id));
  }
  function toggleGroup(ids: string[]) {
    setSelected((prev) => toggleGroupSelection(prev, ids));
  }
  function toggleExpanded(key: string) {
    setExpanded((prev) => toggleSetKey(prev, key));
  }
  /** 预检：按客户端各自的判定决定是否要「关闭并重开」宿主（失败按不需要处理，后端仍有实时判定兜底）。 */
  async function pluginNeedsRestart(): Promise<boolean> {
    try {
      if (client === "jetbrains") {
        return (await api.jetbrainsRestartPrecheck([targetAccountId])).required === true;
      }
      return (await api.vscodeRestartPrecheck([targetAccountId])).required === true;
    } catch {
      return false;
    }
  }
  async function copyAndLink() {
    if (!sourceAccount || !targetAccountId || sourceAccount.id === targetAccountId || selected.size === 0 || busy) return;
    if (client === "vscodeExt" || client === "jetbrains") {
      // 运行中的写入会被宿主覆盖：先走页面统一的确认框授权「关闭并重开」。
      if (await pluginNeedsRestart()) {
        requestRestartConfirm("关闭并复制", () => copyAndLinkPlugin(true));
        return;
      }
      await copyAndLinkPlugin(false);
      return;
    }
    await copyAndLinkWorkbuddy();
  }
  async function copyAndLinkPlugin(restart: boolean) {
    if (!sourceAccount || !targetAccountId || sourceAccount.id === targetAccountId || selected.size === 0 || busy) return;
    const copyId = ++copyRequestId.current;
    setBusy(true);
    try {
      const report = await api.copyLinkedSessions({
        client,
        sourceAccountId: sourceAccount.id,
        targetAccountId,
        sessionIds: [...selected],
        restart,
      });
      const targetName = targetAccount ? displayName(targetAccount) : "目标账号";
      const status = String(report.status);
      const skipped = Array.isArray(report.skipped) ? report.skipped : [];
      const copiedCount = (report.report as { copied?: unknown[] } | undefined)?.copied?.length ?? 0;
      if (status === "linked") {
        toast.success(`已复制 ${copiedCount} 个会话到「${targetName}」并建立关联`, report.restartedEditor ? { description: `已重新打开 ${editorLabel}` } : undefined);
      } else if (status === "copiedUnlinked") {
        toast.warning("会话已复制，但没有建立关联", { description: JSON.stringify(report.linkErrors ?? []) });
      } else {
        toast.error("复制并关联失败");
      }
      if (skipped.length > 0) toast.info(`有 ${skipped.length} 个会话因内容缺失被跳过`, { description: skipped.map((item) => item.error).join("；") });
      if (report.editorError) toast.warning("VS Code 未能自动重新打开", { description: report.editorError });
      // 关窗换代后，结果已经提示过；不要把新打开的弹窗关掉，也不要清掉新一次执行的 busy。
      if (copyRequestId.current !== copyId) {
        if (status === "linked") onDone();
        return;
      }
      if (status === "linked") {
        requestId.current += 1;
        setSessionsLoading(false);
        setOpen(false);
        onDone();
      }
    } catch (error) {
      // 换代后也要报失败：请求已经发出，静默会让用户以为没执行。
      toast.error("复制并关联失败", { description: api.asError(error) });
    } finally {
      if (copyRequestId.current === copyId) setBusy(false);
    }
  }
  async function copyAndLinkWorkbuddy(ignoreAux = false) {
    if (!sourceAccount || !targetAccountId || sourceAccount.id === targetAccountId || selected.size === 0 || busy) return;
    const copyId = ++copyRequestId.current;
    setBusy(true);
    setAuxBlocked(null);
    try {
      const report = await api.copySessionsCross(sourceAccount.id, targetAccountId, [...selected], ignoreAux, copyMemory);
      const copied = report.copied?.length ?? 0;
      const alreadyLinked = report.alreadyLinked?.length ?? 0;
      const errors = report.errors ?? [];
      const targetName = targetAccount ? displayName(targetAccount) : "目标账号";
      // 同一份报告可能同时含已复制、已存在与失败项：各分支各自提示，互不隐藏。
      if (copied > 0) toast.success(`已复制 ${copied} 个会话到「${targetName}」并建立关联`);
      if (alreadyLinked > 0) toast.info(`「${targetName}」已存在 ${alreadyLinked} 个会话的副本，未重复复制`);
      if (errors.length > 0) {
        const allFailed = copied === 0 && alreadyLinked === 0;
        toast.error(allFailed ? "复制失败" : "部分会话复制失败", {
          description: errors.map((item) => `${sessions.find((session) => session.id === item.id)?.title || item.id}：${item.error}`).join("；"),
        });
      }
      // 恢复重放补完的会话不会出现在 copied 里：不说明的话用户会以为「仍然复制」没生效。
      const recovered = report.recovered?.length ?? 0;
      if (recovered > 0) toast.success(`已补完 ${recovered} 个会话的复制`);
      if (ignoreAux && copied + recovered > 0) {
        toast.info("本次复制未带用量与中断记录", { description: "之后不会自动补上：目标账号里这些会话的「共消耗」仍会比源账号少。" });
      }
      // 账本写入失败是唯一带「显式忽略」出口的失败：不在报告层吞掉，摆在弹窗里让用户选。
      if (report.canIgnoreAux) setAuxBlocked({ count: report.auxFailed?.length ?? errors.length });
      if (report.needsRecovery) toast.error("会话操作待恢复", { description: "已保留恢复所需材料。" });
      // 工作记忆是增强数据：只在勾选时才出现在报告里，失败也只提示，不影响复制结果。
      if (report.memory) {
        if (report.memory.skippedReason) {
          toast.warning("工作记忆没有同步", { description: report.memory.skippedReason });
        } else if (report.memory.appendedLines > 0) {
          toast.info(`已向「${targetName}」追加 ${report.memory.appendedLines} 行工作记忆`);
        }
      }
      if (copied === 0 && alreadyLinked === 0 && recovered === 0 && errors.length === 0 && !report.needsRecovery) {
        toast.error("复制并关联失败", report.error ? { description: report.error } : undefined);
      }
      // 关窗换代后，结果已经提示过；不要把新打开的弹窗关掉，也不要清掉新一次执行的 busy。
      if (copyRequestId.current !== copyId) {
        if (copied > 0 || alreadyLinked > 0) onDone();
        return;
      }
      // 有成功复制，或没有 errors 的已存在副本：关闭并刷新。纯失败留在弹窗。
      if (copied > 0 || recovered > 0 || (alreadyLinked > 0 && errors.length === 0 && !report.canIgnoreAux)) {
        requestId.current += 1;
        setSessionsLoading(false);
        setOpen(false);
        onDone();
      }
    } catch (error) {
      // 换代后也要报失败：请求已经发出，静默会让用户以为没执行。
      toast.error("复制并关联失败", { description: api.asError(error) });
    } finally {
      if (copyRequestId.current === copyId) setBusy(false);
    }
  }
  // 页脚摘要固定带目标账号名：摘要随勾选数与目标变化，执行前就能确认去向。
  const summarySub = !sourceAccount
    ? "先选择来源账号"
    : !targetAccount
      ? "再选择目标账号"
      : selected.size > 0
        ? `复制并关联到「${displayName(targetAccount)}」`
        : `勾选要复制到「${displayName(targetAccount)}」的会话`;
  return <Dialog open={open} onOpenChange={openChange}>
    <span className="inline-flex" title={supported ? undefined : unsupportedTitle}>
      <DialogTrigger asChild>
        <Button size="sm" className="shrink-0 bg-brand text-brand-foreground hover:bg-brand/90" disabled={disabled || !supported} title={supported ? "新增关联会话" : unsupportedTitle}><Link2 />新增关联会话</Button>
      </DialogTrigger>
    </span>
    <DialogContent showCloseButton={!busy} onEscapeKeyDown={blockDismissWhileBusy} onPointerDownOutside={blockDismissWhileBusy} onInteractOutside={blockDismissWhileBusy} className="flex max-h-[min(90vh,calc(100vh-2rem))] min-w-0 flex-col overflow-hidden">
      <DialogHeader className="shrink-0">
        <DialogTitle>新增关联会话</DialogTitle>
        <DialogDescription>选择一个来源账号的会话，复制到目标账号并建立关联。</DialogDescription>
      </DialogHeader>
      {busy && <div className="absolute inset-0 z-50 flex flex-col items-center justify-center gap-3 rounded-lg bg-background/85 backdrop-blur-sm">
        <Loader2 className="size-8 animate-spin text-primary" />
        <p className="text-sm font-medium">正在复制并关联…</p>
        <p className="max-w-xs text-center text-xs text-muted-foreground">正在处理中，请勿关闭窗口</p>
      </div>}
      <div className="flex min-h-0 flex-col gap-3 overflow-x-hidden overflow-y-auto">
        <div className="min-w-0 space-y-1.5"><p className="text-xs font-medium">来源账号</p>
          <Select value={sourceAccountId} onValueChange={changeSource} disabled={busy}>
            <SelectTrigger size="sm" className="w-full min-w-0 bg-background text-xs" aria-label="选择来源账号"><SelectValue placeholder={`选择${clientAccountLabel}`} /></SelectTrigger>
            <SelectContent>{workbuddyAccounts.map((account) => <SelectItem key={account.id} value={account.id} className="text-xs">{displayName(account)} · {variantLabel(accountVariant(account))}</SelectItem>)}</SelectContent>
          </Select>
          {workbuddyAccounts.length === 0 && <p className="text-xs text-muted-foreground">账号库里还没有{clientAccountLabel}。</p>}
        </div>
        <div className="min-w-0 space-y-1.5"><p className="text-xs font-medium">来源会话</p>
          {!sourceAccountId
            ? <p className={`flex items-center justify-center px-3 text-center text-sm text-muted-foreground ${LINKED_TREE_MIN_H}`}>先选择来源账号，再从会话树里勾选要复制的会话。</p>
            : sessionsLoading
              ? <div className={`flex items-center justify-center gap-2 text-sm text-muted-foreground ${LINKED_TREE_MIN_H}`}><Loader2 className="animate-spin" />正在读取会话…</div>
              : sessionsError
                ? <div role="alert" className={`flex flex-col items-center justify-center gap-2 px-3 text-center ${LINKED_TREE_MIN_H}`}><p className="text-sm text-danger">{sessionsError}</p><Button variant="outline" size="sm" onClick={() => { if (sourceAccount) void loadSessions(sourceAccount); }}>重试</Button></div>
                : sessions.length === 0
                  ? <p className={`flex items-center justify-center px-3 text-center text-sm text-muted-foreground ${LINKED_TREE_MIN_H}`}>{(client === "vscodeExt" || client === "jetbrains") && dataRoot === null ? (client === "jetbrains" ? "未找到插件数据目录：请先打开 IDEA / PyCharm 并登录 CodeBuddy 插件。" : "未找到插件数据目录：请先打开 VS Code 并登录 CodeBuddy 插件。") : "该账号没有可复制的会话。"}</p>
                  : <SessionTreeList
                    sessions={sessions}
                    selected={selected}
                    expanded={expanded}
                    onToggleSession={toggleSession}
                    onToggleGroup={toggleGroup}
                    onToggleExpanded={toggleExpanded}
                    className={`max-h-[min(22rem,45vh)] overflow-y-auto pr-1 ${LINKED_TREE_MIN_H}`}
                  />}
        </div>
        {auxBlocked && <div role="alert" className="rounded-md border border-danger/40 bg-danger/5 p-2.5 text-xs">
          <p className="font-medium text-danger">有 {auxBlocked.count} 个会话的用量记录没能写入</p>
          <p className="mt-1 text-muted-foreground">可以仍然复制，但副本不带用量与中断记录：目标账号里这些会话的「共消耗」会比源账号少，<span className="font-medium text-foreground">之后不会自动补上</span>。</p>
          <Button variant="outline" size="sm" className="mt-2" onClick={() => void copyAndLinkWorkbuddy(true)} disabled={busy}>仍然复制（不带用量与中断记录）</Button>
        </div>}
        <div className="min-w-0 space-y-1.5"><p className="text-xs font-medium">目标账号</p>
          <Select value={targetAccountId} onValueChange={setTargetAccountId} disabled={busy || !sourceAccountId || targetOptions.length === 0}>
            <SelectTrigger size="sm" className="w-full min-w-0 bg-background text-xs" aria-label="选择目标账号"><SelectValue placeholder={sourceAccountId ? "选择目标账号" : "先选择来源账号"} /></SelectTrigger>
            <SelectContent>{targetOptions.map((account) => <SelectItem key={account.id} value={account.id} className="text-xs">{displayName(account)} · {variantLabel(accountVariant(account))}</SelectItem>)}</SelectContent>
          </Select>
        </div>
        <div className="min-w-0 space-y-1.5"><p className="text-xs font-medium">附加内容</p>
          <label className="flex min-w-0 items-start gap-2 text-xs">
            <Checkbox
              className="mt-0.5"
              checked={copyMemory}
              disabled={busy || client !== "workbuddy" || !sourceAccountId || !targetAccountId}
              onCheckedChange={(state) => setCopyMemory(state === true)}
              aria-label="同时复制工作记忆（MEMORY.md）"
            />
            <span className="min-w-0">
              <span className="font-medium">同时复制工作记忆（MEMORY.md）</span>
              <span className="mt-0.5 block text-muted-foreground">
                把来源账号积累的长期偏好搬到目标账号：<span className="font-medium text-foreground">只追加目标里没有的行，不覆盖目标已有内容</span>。默认不勾。
              </span>
            </span>
          </label>
        </div>
      </div>
      <DialogFooter className="shrink-0 sm:justify-between">
        <div className="min-w-0 space-y-0.5">
          <div className="text-sm font-medium">已选 {selected.size} 个会话</div>
          <div className="text-xs text-muted-foreground">{summarySub}</div>
        </div>
        <div className="flex shrink-0 gap-2">
          <Button variant="outline" onClick={() => openChange(false)} disabled={busy}>取消</Button>
          <DemoAction><Button onClick={() => void copyAndLink()} disabled={busy || !sourceAccount || !targetAccountId || sourceAccount.id === targetAccountId || selected.size === 0}>{busy ? <Loader2 className="animate-spin" /> : <Link2 />}{busy ? "处理中…" : "复制并关联"}</Button></DemoAction>
        </div>
      </DialogFooter>
    </DialogContent>
  </Dialog>;
}
