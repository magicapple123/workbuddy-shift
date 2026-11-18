import { useState } from "react";
import { toast } from "sonner";

import * as api from "@/lib/api";
import type {
  SessionGroupClient,
  SessionGroupMemberDetail,
  SessionGroupPruneReport,
  SessionGroupSummary,
  SessionSyncMode,
  WbVariant,
} from "@/lib/types";
import { variantLabel } from "@/lib/variant";

import { attemptPrune, pruneFailureDescription, runPrune } from "../prune";
import type { SessionDetailController } from "./use-session-detail";

export interface SessionActionsController {
  actionBusy: boolean;
  restartPrompt: { actionLabel: string; run: () => Promise<void> } | null;
  setRestartPrompt: (prompt: { actionLabel: string; run: () => Promise<void> } | null) => void;
  deleteTarget: SessionGroupSummary | null;
  setDeleteTarget: (target: SessionGroupSummary | null) => void;
  /** dry-run 统计结果：非空即打开「清理失效关联组」二次确认框。 */
  pruneTarget: SessionGroupPruneReport | null;
  setPruneTarget: (target: SessionGroupPruneReport | null) => void;
  prepareUnify: (source: SessionGroupMemberDetail) => Promise<void>;
  confirmUnify: () => Promise<void>;
  syncSafeBatch: () => Promise<void>;
  addMember: () => Promise<void>;
  confirmRestart: () => Promise<void>;
  unlinkMember: (member: SessionGroupMemberDetail) => Promise<void>;
  deleteGroup: () => Promise<void>;
  requestPrune: () => Promise<void>;
  confirmPrune: () => Promise<void>;
}

interface Options {
  client: SessionGroupClient;
  scope: WbVariant | undefined;
  /**
   * 清理被阻塞的原因（列表载荷的 `pruneBlocked`，`null` = 可清理）。
   *
   * 入口按钮已经据此禁用，这里再拦一次是为了「按钮还没重渲染就被点到」的竞态：
   * 后端一定会整次拒绝，先请求只会做一次无用的 dry-run 并制造一条误导性的失败提示。
   */
  pruneBlocked?: string | null;
  selectedGroupId: string | null;
  setSelectedGroupId: (id: string | null) => void;
  closeDetail: () => void;
  detail: SessionDetailController;
  reloadGroups: () => Promise<void>;
}

/**
 * 关联会话的写操作：统一、批量同步、添加副本、取消关联与删除整组。
 *
 * 每个动作都先做上下文/预览方案校验，再调用 API，成功后在动作内部刷新列表与详情；
 * 失败统一 toast，避免把原始错误抛到页面层。
 */
export function useSessionActions({
  client,
  scope,
  pruneBlocked,
  selectedGroupId,
  setSelectedGroupId,
  closeDetail,
  detail,
  reloadGroups,
}: Options): SessionActionsController {
  const [actionBusy, setActionBusy] = useState(false);
  const [deleteTarget, setDeleteTarget] = useState<SessionGroupSummary | null>(null);
  const [pruneTarget, setPruneTarget] = useState<SessionGroupPruneReport | null>(null);
  /** 插件侧「VS Code 运行中」的确认框：确认后执行 run（带 restart），取消则丢弃。 */
  const [restartPrompt, setRestartPrompt] = useState<{ actionLabel: string; run: () => Promise<void> } | null>(null);

  /**
   * 预检：本次写入目标是否包含当前登录账号（只有涉及才需要「关闭并重开」确认框）。
   * 探测失败按「不需要」处理：后端在各写入口仍有实时判定兜底，不会误关编辑器。
   */
  async function needsEditorRestart(targetAccountIds: string[]): Promise<boolean> {
    if (client !== "vscodeExt") return false;
    try {
      return (await api.vscodeRestartPrecheck(targetAccountIds)).required === true;
    } catch {
      return false;
    }
  }

  async function prepareUnify(source: SessionGroupMemberDetail) {
    await detail.prepareUnify(source, actionBusy);
  }

  async function confirmUnify() {
    const plan = detail.unifyPlan;
    const currentDetail = detail.detail;
    if (!plan || !currentDetail || plan.client !== client || plan.groupId !== currentDetail.groupId || actionBusy) return;
    const actions = plan.targets.flatMap((target) => {
      const preview = target.preview;
      if (!preview || preview.verdict === "identical") return [];
      const mode: SessionSyncMode | undefined = ["fastForward", "overwrite", "unifyOverwrite"].find(
        (candidate) => preview.availableModes.includes(candidate as SessionSyncMode),
      ) as SessionSyncMode | undefined;
      return mode && preview.previewToken && preview.client === client && preview.groupId === plan.groupId
        && preview.sourceMemberId === plan.sourceMemberId && preview.targetMemberId === target.memberId
        ? [{ target, preview, mode }] : [];
    });
    if (plan.targets.some((target) => target.error || !target.preview
      || target.preview.client !== client || target.preview.groupId !== plan.groupId
      || target.preview.sourceMemberId !== plan.sourceMemberId || target.preview.targetMemberId !== target.memberId
      || (target.preview.verdict !== "identical" && !actions.some((action) => action.target.memberId === target.memberId)))) return;
    detail.setUnifyPlan(null);
    setActionBusy(true);
    try {
      if ((client === "workbuddy" || client === "vscodeExt") && actions.length > 0) {
        // 插件侧：只有写入目标含当前登录账号才需要关闭/重开（统一确认框已提前告知）。
        const targetAccountIds = actions
          .map(({ target }) => currentDetail.members.find((member) => member.memberId === target.memberId)?.accountId)
          .filter((id): id is string => Boolean(id));
        const restart = client === "vscodeExt" && (await needsEditorRestart(targetAccountIds));
        const report = await api.syncSessionGroupUnify({
          client, groupId: plan.groupId, sourceMemberId: plan.sourceMemberId,
          targets: actions.map(({ target, preview, mode }) => ({ targetMemberId: target.memberId, previewToken: preview.previewToken!, mode })),
          ...(restart ? { restart: true } : {}),
        });
        notifyResult(report);
      } else {
        const combined: { synced: unknown[]; skipped: unknown[]; errors: { error: string }[]; needsRecovery: boolean } = { synced: [], skipped: [], errors: [], needsRecovery: false };
        for (const { target, preview, mode } of actions) {
          try {
            const report = await api.syncSessionGroupPair({
              client, groupId: plan.groupId, sourceMemberId: plan.sourceMemberId, targetMemberId: target.memberId,
              previewToken: preview.previewToken!, mode, variantScope: scope,
            });
            combined.synced.push(...report.synced);
            combined.skipped.push(...report.skipped);
            combined.errors.push(...report.errors);
            combined.needsRecovery ||= Boolean(report.needsRecovery);
            if (report.needsRecovery) break;
          } catch (error) {
            combined.errors.push({ error: `${target.accountName}：${api.asError(error)}` });
            break;
          }
        }
        notifyResult(combined);
      }
      await detail.reloadSelectedGroup();
    } catch (error) {
      toast.error("统一会话内容失败", { description: api.asError(error) });
    } finally {
      setActionBusy(false);
    }
  }

  async function syncSafeBatch() {
    const currentDetail = detail.detail;
    if (!currentDetail?.safeSourceMemberId || actionBusy) return;
    // 插件侧：写入目标含当前登录账号时才会被运行中的 VS Code 覆盖，先确认「关闭并重开」再执行。
    const targetAccountIds = (currentDetail.members ?? [])
      .filter((member) => member.linkState === "active" && member.versionStatus === "behind")
      .map((member) => member.accountId)
      .filter((id): id is string => Boolean(id));
    if (await needsEditorRestart(targetAccountIds)) {
      setRestartPrompt({ actionLabel: "关闭并同步", run: () => runSafeBatch(true) });
      return;
    }
    await runSafeBatch(false);
  }

  async function runSafeBatch(restart: boolean) {
    const currentDetail = detail.detail;
    if (!currentDetail?.safeSourceMemberId || actionBusy) return;
    setActionBusy(true);
    try {
      const report = await api.syncSessionGroupSafeBatch(client, currentDetail.groupId, scope, restart);
      notifyResult(report);
      await detail.reloadSelectedGroup();
    } catch (error) {
      toast.error("批量同步失败", { description: api.asError(error) });
    } finally {
      setActionBusy(false);
    }
  }

  async function addMember() {
    const currentDetail = detail.detail;
    if (!currentDetail || !detail.sourceMemberId || !detail.addTargetId || actionBusy) return;
    if (await needsEditorRestart([detail.addTargetId])) {
      setRestartPrompt({ actionLabel: "关闭并复制", run: () => runAddMember(true) });
      return;
    }
    await runAddMember(false);
  }

  async function runAddMember(restart: boolean) {
    const currentDetail = detail.detail;
    if (!currentDetail || !detail.sourceMemberId || !detail.addTargetId || actionBusy) return;
    setActionBusy(true);
    try {
      const report = await api.addSessionGroupMember({
        client,
        groupId: currentDetail.groupId,
        sourceMemberId: detail.sourceMemberId,
        targetAccountId: detail.addTargetId,
        variantScope: scope,
        restart,
      });
      if (report.status === "linked") toast.success("已复制并添加到关联组", report.restartedEditor ? { description: "已重新打开 VS Code" } : undefined);
      else if (report.status === "alreadyLinked") toast.info("该账号已在关联组中");
      else if (report.status === "copiedUnlinked") toast.warning("会话已复制，但没有建立关联", { description: JSON.stringify(report.linkErrors ?? []) });
      else toast.error("添加关联账号失败");
      if (report.editorError) toast.warning("VS Code 未能自动重新打开", { description: report.editorError });
      await detail.reloadSelectedGroup();
    } catch (error) {
      toast.error("添加关联账号失败", { description: api.asError(error) });
    } finally {
      setActionBusy(false);
    }
  }

  /** 确认框「关闭并继续」：执行动作，结束后关闭确认框（失败由动作内部 toast）。 */
  async function confirmRestart() {
    const prompt = restartPrompt;
    if (!prompt || actionBusy) return;
    try {
      await prompt.run();
    } finally {
      setRestartPrompt(null);
    }
  }

  async function unlinkMember(member: SessionGroupMemberDetail) {
    if (actionBusy) {
      const error = new Error("请等待当前操作完成");
      toast.error("取消关联失败", { description: error.message });
      throw error;
    }
    const currentDetail = detail.detail;
    if (!currentDetail || currentDetail.client !== client || currentDetail.groupId !== selectedGroupId) {
      const error = new Error("会话组已变化，请刷新后重试");
      toast.error("取消关联失败", { description: error.message });
      throw error;
    }
    const groupId = currentDetail.groupId;
    setActionBusy(true);
    try {
      const report = await api.unlinkSessionGroupMember({
        client,
        groupId,
        memberId: member.memberId,
        variantScope: scope,
      });
      if (report.status === "groupRemoved") {
        toast.success("已取消关联，该会话组已无成员，已删除");
        // 组已删除，再拉详情只会得到「不存在」。关掉弹窗并只刷新列表。
        closeDetail();
        setSelectedGroupId(null);
        await reloadGroups();
      } else {
        toast.success(`已取消「${member.accountName}」的关联`);
        await detail.reloadSelectedGroup();
      }
    } catch (error) {
      toast.error("取消关联失败", { description: api.asError(error) });
      throw error;
    } finally {
      setActionBusy(false);
    }
  }

  /** 删除整个会话组：组内所有成员一起解除关联，只解除管理关系，不删除会话内容。 */
  async function deleteGroup() {
    const group = deleteTarget;
    if (!group || actionBusy) return;
    setActionBusy(true);
    try {
      const report = await api.deleteSessionGroup({ client, groupId: group.groupId, variantScope: scope });
      toast.success(`已删除「${group.title}」的关联`, { description: `共解除 ${report.removed} 个账号的关联，账号内的会话内容不受影响` });
      setDeleteTarget(null);
      if (group.groupId === selectedGroupId) {
        closeDetail();
        setSelectedGroupId(null);
      }
      await reloadGroups();
    } catch (error) {
      // 失败时保留确认框，方便直接重试。
      toast.error("删除关联失败", { description: api.asError(error) });
    } finally {
      setActionBusy(false);
    }
  }

  /**
   * 检查有多少失效关联组（`dryRun` 只统计不写盘）。
   *
   * 数量为 0 时直接提示、不弹确认框：让用户对着「0 组」点一次「确认清理」没有意义。
   * 数量 > 0 时把 dry-run 结果存下来，确认框文案与「确认后真删」都用这一份。
   *
   * 被阻塞（`pruneBlocked` 非空）时**不发任何请求**：后端会用同一条原因整次拒绝，
   * 这里直接把原因讲清楚，入口本身也已经禁用。
   */
  async function requestPrune() {
    if (actionBusy) return;
    setActionBusy(true);
    try {
      const attempt = await attemptPrune({
        blockedReason: pruneBlocked,
        request: (dryRun) => api.pruneSessionGroups({ client, variantScope: scope, dryRun }),
      });
      if (attempt.status === "blocked") {
        toast.warning("暂时无法清理失效关联组", { description: attempt.reason });
        return;
      }
      if (attempt.status === "empty") {
        toast.info("没有失效的关联组", { description: "组内会话都还在，无需清理。" });
        return;
      }
      setPruneTarget(attempt.report);
    } catch (error) {
      // 后端的拒绝原因原样透传（例如「无法判定 VS Code 插件账号 X 的会话，已取消清理」）。
      toast.error("检查失效关联组失败", { description: pruneFailureDescription(api.asError(error)) });
    } finally {
      setActionBusy(false);
    }
  }

  /** 确认清理：`dryRun: false` 真删（后端写盘前会备份关联记录）。 */
  async function confirmPrune() {
    const target = pruneTarget;
    if (!target || actionBusy) return;
    setActionBusy(true);
    try {
      const { report, reloadError } = await runPrune(
        (dryRun) => api.pruneSessionGroups({ client, variantScope: scope, dryRun }),
        reloadGroups,
      );
      toast.success(`已清理 ${report.removedGroups} 组失效关联`, {
        description: `共解除 ${report.removedMembers} 个账号副本的关联，账号内的会话内容不受影响`,
      });
      setPruneTarget(null);
      // 只有被清理的组正好是当前详情时才关闭详情：其余情况保留用户正在看的组。
      if (selectedGroupId && report.groupIds.includes(selectedGroupId)) {
        closeDetail();
        setSelectedGroupId(null);
      }
      // 刷新失败只补一条独立提示：清理已经落盘，不能改写「已清理」的结论。
      if (reloadError) {
        toast.warning("列表刷新失败，请手动刷新查看最新列表", {
          description: api.asError(reloadError),
        });
      }
    } catch (error) {
      // 失败时保留确认框，方便直接重试（后端失败不写盘，重试是安全的）。
      // 拒绝原因（哪个账号、为什么无法判定）必须原样显示，不能吞成泛化的「清理失败」。
      toast.error("清理失效关联组失败", { description: pruneFailureDescription(api.asError(error)) });
    } finally {
      setActionBusy(false);
    }
  }

  return {
    actionBusy,
    restartPrompt,
    setRestartPrompt,
    deleteTarget,
    setDeleteTarget,
    pruneTarget,
    setPruneTarget,
    prepareUnify,
    confirmUnify,
    syncSafeBatch,
    addMember,
    confirmRestart,
    unlinkMember,
    deleteGroup,
    requestPrune,
    confirmPrune,
  };
}

function notifyResult(report: {
  synced?: unknown[];
  skipped?: unknown[];
  errors?: { error: string }[];
  needsRecovery?: boolean;
  temporaryFiles?: { reason: string }[];
  restartedVariants?: WbVariant[];
  restartedEditor?: boolean;
  editorError?: string;
}, targetName?: string) {
  const synced = report.synced?.length ?? 0;
  const skipped = report.skipped?.length ?? 0;
  const errors = report.errors ?? [];
  const restartHint = report.restartedVariants?.length
    ? `，已重新打开 ${report.restartedVariants.map(variantLabel).join("、")}`
    : report.restartedEditor ? "，已重新打开 VS Code" : "";
  if (synced > 0) toast.success(targetName ? `已同步到「${targetName}」` : `已同步 ${synced} 个会话`, { description: `完成 ${synced} 项${restartHint}` });
  if (skipped > 0) toast.warning(`有 ${skipped} 项跳过`, { description: "预览过期或复核后不再符合安全条件的项不会计为成功。" });
  if (errors.length > 0) toast.error("部分会话同步失败", { description: errors.map((item) => item.error).join("；") });
  if (report.needsRecovery) toast.error("会话操作待恢复", { description: report.temporaryFiles?.map((item) => item.reason).join("；") || "已保留恢复所需材料。" });
  if (report.editorError) toast.warning("VS Code 未能自动重新打开", { description: report.editorError });
  if (synced === 0 && skipped === 0 && errors.length === 0 && !report.needsRecovery) toast.info("当前没有需要同步的副本");
}
