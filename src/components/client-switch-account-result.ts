import { toast } from "sonner";

import type { ClientSwitchClient } from "@/components/client-switch-account-types";
import type {
  CodeBuddyCnIdeSwitchResult,
  SessionLinkPreviewGroup,
  VscodeExtSwitchResult,
  VscodeSessionSyncReport,
} from "@/lib/types";

/**
 * 客户端切号结果的两个客户端公共字段（生效说明的算法不同，按 `client` 分支）。
 *
 * VS Code 额外有 `restarted` / `closedByUs`；CodeBuddy IDE 固定关闭并重开，只有 `message`。
 */
export type ClientSwitchResult = Pick<CodeBuddyCnIdeSwitchResult, "sessionCopy" | "sessionSync" | "message"> &
  Partial<Pick<VscodeExtSwitchResult, "restarted" | "closedByUs">>;

export interface ClientSwitchReportInput {
  client: ClientSwitchClient;
  /** 目标账号昵称（toast 标题用）。 */
  nickname: string;
  result: ClientSwitchResult;
  /** 已勾选的关联会话（同步问题提示里回显会话名）。 */
  syncGroups: SessionLinkPreviewGroup[];
  /** 本次是否请求了同步（未勾选同步时不提示「没有收到同步结果」）。 */
  requestedSync: boolean;
}

/**
 * 切换结果的 toast 反馈：成功、复制失败、关联未建立、同步跳过/失败都要能看到。
 *
 * 逐字保留原两个弹窗的提示语义：不能只显示成功数，也不把跳过/未建立关联当成完整成功。
 */
export function reportClientSwitchResult({
  client,
  nickname,
  result,
  syncGroups,
  requestedSync,
}: ClientSwitchReportInput): void {
  const copied = result.sessionCopy?.copied.length ?? 0;
  const errors = result.sessionCopy?.errors ?? [];
  const linkErrors = result.sessionCopy?.linkErrors ?? [];
  const syncReport: VscodeSessionSyncReport | undefined = result.sessionSync;
  const syncedItems = syncReport?.synced ?? [];
  const skippedItems = syncReport?.skipped ?? [];
  const syncErrors = syncReport?.errors ?? [];
  // VS Code 生效方式提示：重载窗口读不到外部写入，不再提示；区分「已重开 / 重开失败 / 本来没运行」。
  // 重开失败时前端拿不到原因，用后端 message（含具体错误）兜底。
  const restartHint =
    client === "vscode"
      ? result.restarted
        ? "已切换并重新打开 VS Code"
        : result.closedByUs
          ? result.message || "已切换，但自动重新打开 VS Code 失败，请手动打开"
          : "已切换；请打开 VS Code 生效"
      : result.message || "已重启 CodeBuddy IDE";
  const copiedHint = copied > 0 ? `已复制 ${copied} 个会话` : null;
  const syncedHint = syncedItems.length > 0 ? `已同步 ${syncedItems.length} 个会话` : null;

  if (errors.length > 0) {
    toast.warning(`已切换至「${nickname}」，但部分会话未复制`, {
      description: [
        `已复制 ${copied} 个会话`,
        `${errors.length} 个失败：${errors.map((e) => e.error).join("；")}`,
        restartHint,
      ].join("；"),
    });
  } else {
    toast.success(`已切换至「${nickname}」`, {
      description: [copiedHint, syncedHint, restartHint].filter(Boolean).join("；"),
    });
  }
  // 复制成功但登记失败：复制本身有效，必须提示「未建立关联」而不是当成完整成功。
  if (linkErrors.length > 0) {
    toast.warning("部分会话已复制但未建立关联", {
      description: [
        ...linkErrors.map((item) => `${item.conversationId ?? "会话"}：${item.error}`),
        "未建立关联的会话不会出现在「关联会话」列表里。",
      ].join("；"),
    });
  }
  // 同步结果：成功、跳过、失败都要能看到，不能只显示成功数。
  const groupLabel = (groupId: string) =>
    syncGroups.find((group) => group.groupId === groupId)?.title || groupId;
  const syncIssues = [
    ...syncErrors.map(
      (item) => `${item.groupId ? groupLabel(item.groupId) : "全部会话"}：${item.error}`,
    ),
    // 预览过期/前置条件变化一律跳过并给出原因，绝不显示成已同步。
    ...skippedItems.map((item) => `${groupLabel(item.groupId)}：${item.message}`),
  ];
  if (syncIssues.length > 0) {
    if (syncErrors.length > 0) {
      toast.error("部分关联会话没有同步（失败或已跳过）", { description: syncIssues.join("；") });
    } else {
      toast.warning("有关联会话没有同步（已跳过）", { description: syncIssues.join("；") });
    }
  } else if (requestedSync && !syncReport) {
    toast.warning("关联会话未同步", {
      description: "没有收到同步结果：当前版本可能不支持同步关联会话；账号已切换，但会话没有同步。",
    });
  }
}
