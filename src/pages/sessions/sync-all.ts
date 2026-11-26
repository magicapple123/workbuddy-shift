import type { SessionGroupClient, SessionGroupSummary } from "@/lib/types";

/**
 * 「跨组安全批量同步」的纯逻辑（候选筛选与确认框文案）。
 *
 * 抽成纯函数是为了把两件事钉住：
 * 1. **只同步 `behind` 组**：有分歧 / 无法确认的组一律不进候选，用户想看原因就打开该组手动选；
 * 2. **按客户端的关闭承诺不同**：WorkBuddy 不会因同步而关闭客户端，VS Code / JetBrains 会在
 *    运行中先关闭并在结束后重开 —— 确认框必须如实说，不能给 WorkBuddy 用户一个空承诺。
 */

/**
 * 候选筛选用到的字段子集。
 *
 * `behindMemberCount` 是可选的：老后端没有这个字段，此时按 0 计（只影响确认框里的副本数，
 * 不影响能不能同步）。
 */
export type SyncAllCandidateGroup = Pick<SessionGroupSummary, "groupId" | "summaryStatus"> & {
  behindMemberCount?: number;
};

/** 本次批量同步的候选：要发给后端的组号，以及用于文案的两个汇总数。 */
export interface SyncAllCandidates {
  groupIds: string[];
  groupCount: number;
  memberCount: number;
}

/** 只保留「待同步」（`summaryStatus === "behind"`）的组，并汇总落后副本数。 */
export function selectSyncAllCandidates(
  groups: readonly SyncAllCandidateGroup[],
): SyncAllCandidates {
  const candidates = groups.filter((group) => group.summaryStatus === "behind");
  return {
    groupIds: candidates.map((group) => group.groupId),
    groupCount: candidates.length,
    memberCount: candidates.reduce(
      (total, group) => total + (group.behindMemberCount ?? 0),
      0,
    ),
  };
}

/** 确认框文案：`title` 说清范围，`description` 说清代价（是否会关宿主、跳过什么）。 */
export interface SyncAllDialogCopy {
  title: string;
  description: string;
  busyText: string;
}

/**
 * 批量同步确认框文案（按客户端分派）。
 *
 * 统一承诺两件事：只做**安全快进**、存在分歧的组**跳过**并在结果里给原因。
 * 差异只有一处——是否关闭宿主：WorkBuddy 与 CodeBuddy IDE 都不在同步过程中关闭，
 * VS Code / JetBrains 运行中必须先关闭（写入会被内存态覆盖）并在结束后重开。
 */
export function syncAllDialogCopy(
  client: SessionGroupClient,
  groupCount: number,
  memberCount: number,
): SyncAllDialogCopy {
  const scope = `将一次同步 ${memberCount} 个落后副本（共 ${groupCount} 个组），只做安全快进；存在分歧或无法确认来源的组会整组跳过，并在结果里说明原因。`;
  const title = `同步 ${groupCount} 个待同步组？`;
  if (client === "vscodeExt") {
    return {
      title,
      description: `${scope}运行中的 VS Code 会被关闭并重开：未保存内容由 VS Code 自身提示保护，同步完成后再自动重新打开。`,
      busyText: "正在关闭 VS Code 并同步，请勿关闭本窗口…",
    };
  }
  if (client === "jetbrains") {
    return {
      title,
      description: `${scope}运行中的 IDEA / PyCharm 会被关闭并重开：未保存内容由 IDE 自身提示保护（最多等待 60 秒），同步完成后再逐个自动重新打开。`,
      busyText: "正在关闭 JetBrains IDE 并同步，请勿关闭本窗口…",
    };
  }
  if (client === "codebuddyIde") {
    return {
      title,
      description: `${scope}同步前请完全退出 CodeBuddy IDE：它运行时会用内存态覆盖磁盘，本次不会替你关闭。`,
      busyText: "正在同步，请勿关闭本窗口…",
    };
  }
  // WorkBuddy：同步不关闭客户端（写入按会话粒度进行），因此不做任何关闭承诺。
  return {
    title,
    description: `${scope}本次不会关闭 WorkBuddy，正在使用的客户端不会被中断。`,
    busyText: "正在同步，请勿关闭本窗口…",
  };
}
