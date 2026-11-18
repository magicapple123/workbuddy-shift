import { toast } from "sonner";

import { displayName } from "@/lib/account-display";
import { accountVariant, variantAppName } from "@/lib/variant";
import type {
  AccountMeta,
  Session,
  SessionLinkPreviewGroup,
  SwitchResult,
  TemporaryFileInfo,
} from "@/lib/types";

/** 临时备份残留的可读描述：优先标题，其次会话 id，最后操作 id。 */
function describeTemporaryFile(item: TemporaryFileInfo): string {
  const label = item.title || item.sessionId || item.operationId;
  return `${label}：${item.reason}`;
}

/** 同一操作只保留一条：复制 / 同步 / 恢复三个报告可能重复上报同一残留。 */
function dedupeTemporaryFiles(items: TemporaryFileInfo[]): TemporaryFileInfo[] {
  const seen = new Map<string, TemporaryFileInfo>();
  for (const item of items) {
    const key = `${item.operationId}:${item.state}`;
    const existing = seen.get(key);
    if (!existing || (existing.state === "cleanupPending" && item.state === "needsRecovery")) {
      seen.set(key, item);
    }
  }
  return [...seen.values()];
}

interface SwitchResultContext {
  result: SwitchResult;
  account: AccountMeta;
  /** 当前账号会话列表：复制失败提示用标题而不是 id。 */
  sessions: Session[];
  /** 已勾选的关联会话：同步失败/跳过提示用标题而不是 groupId。 */
  syncGroups: SessionLinkPreviewGroup[];
  requestedCopy: boolean;
  requestedSync: boolean;
}

/**
 * 切换结果提示：成功摘要 + 复制 / 同步 / 恢复 / 临时文件残留的逐项提示。
 *
 * 提示顺序与原实现一致；失败与跳过必须显式提示，不能静默当成成功。
 */
export function notifySwitchResult({
  result: res,
  account,
  sessions,
  syncGroups,
  requestedCopy,
  requestedSync,
}: SwitchResultContext): void {
  const nickname = displayName(account);
  const parts: string[] = [];
  const copyReport = res.sessionCopy;
  const copiedCount = copyReport?.copied?.length ?? 0;
  const linkedCount = copyReport?.alreadyLinked?.length ?? 0;
  const copyErrors = copyReport?.errors ?? [];
  if (copiedCount > 0) parts.push(`已复制 ${copiedCount} 个会话`);
  if (linkedCount > 0) parts.push(`已关联 ${linkedCount} 个之前复制过的会话`);
  if (copyErrors.length > 0) parts.push(`会话复制失败 ${copyErrors.length} 个`);
  const syncReport = res.sessionSync;
  const syncedItems = syncReport?.synced ?? [];
  const skippedItems = syncReport?.skipped ?? [];
  const syncErrors = syncReport?.errors ?? [];
  if (syncedItems.length > 0) parts.push(`已同步 ${syncedItems.length} 个会话`);
  if (skippedItems.length > 0) parts.push(`跳过 ${skippedItems.length} 个会话`);
  if (syncErrors.length > 0) parts.push(`会话同步失败 ${syncErrors.length} 个`);
  if (res.backup) parts.push(`备份: ${res.backup}`);
  // 成功清理：临时备份已回收，不再展示可还原路径；待清理项单独提示，不写进成功文案。
  toast.success(`已切换至「${nickname}」`, {
    description: parts.length ? parts.join("；") : `${variantAppName(accountVariant(account))} 已重启为目标账号。`,
  });
  // 复制失败或被后端跳过时必须显式提示，不能静默当成成功。
  if (copyReport?.error) {
    toast.error("会话复制未执行", { description: copyReport.error });
  } else if (copyErrors.length > 0) {
    // 失败提示必须带会话标识（能取标题时优先标题），不是只有原因。
    const sessionLabel = (id: string) =>
      sessions.find((item) => item.id === id)?.title || id;
    toast.error("部分会话未复制", {
      description: copyErrors
        .map((item) => `${sessionLabel(item.id)}：${item.error}`)
        .join("；"),
    });
  } else if (requestedCopy && !copyReport) {
    toast.warning("会话未复制", {
      description: "没有收到复制结果：当前版本可能不支持复制会话；账号已切换，但会话没有复制。",
    });
  }
  // 同步结果：成功、跳过、失败与恢复信息都要能看到，不能只显示成功数。
  const groupLabel = (groupId: string) =>
    syncGroups.find((group) => group.groupId === groupId)?.title || groupId;
  // 失败与跳过合并成一条提示：两者可能同时出现，不能只报其中一类。
  const syncIssues = [
    ...syncErrors.map(
      (item) => `${item.groupId ? groupLabel(item.groupId) : "全部会话"}：${item.error}`,
    ),
    // 预览过期/前置条件变化一律跳过并给出原因，绝不显示成已同步。
    ...skippedItems.map((item) => `${groupLabel(item.groupId)}：${item.message}`),
  ];
  if (syncIssues.length > 0) {
    if (syncErrors.length > 0) {
      toast.error("部分会话没有同步（失败或已跳过）", { description: syncIssues.join("；") });
    } else {
      toast.warning("有会话没有同步（已跳过）", { description: syncIssues.join("；") });
    }
  } else if (requestedSync && !syncReport) {
    toast.warning("会话同步未执行", {
      description: "没有收到同步结果：当前版本可能不支持同步会话，或本次切换没有重启应用。",
    });
  }
  if (syncReport?.needsRecovery) {
    toast.error("有会话同步没有完成", {
      description: "已保留操作记录与备份，下次切换会先恢复；恢复完成前不会再改动目标账号的内容。",
    });
  }
  // 未完成的会话写入：可重试项只提示，阻断项由后端直接返回错误。
  const recoveryIssues = res.sessionRecovery?.needsRecovery ?? [];
  if (recoveryIssues.length > 0) {
    toast.error("有会话操作没有完成", {
      description: recoveryIssues.map((issue) => issue.reason).join("；"),
    });
  }
  // 临时备份残留：待清理说明重试入口；待恢复说明必须保留材料并给出下一步。
  const temporaryFiles = dedupeTemporaryFiles([
    ...(copyReport?.temporaryFiles ?? []),
    ...(syncReport?.temporaryFiles ?? []),
    ...(res.sessionRecovery?.temporaryFiles ?? []),
  ]);
  const pendingFiles = temporaryFiles.filter((item) => item.state === "cleanupPending");
  const recoveryFiles = temporaryFiles.filter((item) => item.state === "needsRecovery");
  // 报告级 temporaryFiles 是权威来源；仅当旧后端没给该字段时，才回退到成功项上的 pending。
  const pendingCleanupReasons: string[] = [];
  if (temporaryFiles.length === 0) {
    for (const item of copyReport?.copied ?? []) {
      if (item.cleanupState === "pending") {
        pendingCleanupReasons.push(item.cleanupError ?? "临时文件待清理");
      }
    }
    for (const item of syncedItems) {
      if (item.cleanupState === "pending") {
        pendingCleanupReasons.push(item.cleanupError ?? "临时文件待清理");
      }
    }
  }
  if (pendingCleanupReasons.length > 0 || pendingFiles.length > 0) {
    const details = [...pendingCleanupReasons, ...pendingFiles.map(describeTemporaryFile)];
    toast.warning("会话已完成，部分临时文件待清理", {
      description: `${details.join("；")}。下次切号时会自动重试清理。`,
    });
  }
  if (recoveryFiles.length > 0) {
    toast.error("有临时备份需要确认", {
      description: `${recoveryFiles.map(describeTemporaryFile).join("；")}。已保留现场，请按提示处理后重试。`,
    });
  }
}