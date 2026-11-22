import type { AccountMeta, SessionGroupClient, WbVariant } from "@/lib/types";
import { accountVariant, variantLabel } from "@/lib/variant";
import { restartPrecheckClient } from "./restart";

/**
 * 「新增关联会话」弹窗的客户端分派（纯函数，供弹窗与单测共用）。
 *
 * 四个客户端都支持这个入口，但**后端入口不同**，因此不能只留一句 `supported` 白名单：
 * - WorkBuddy：`copySessionsCross`（按账号档位选库，源与目标都可跨档）；
 * - CodeBuddy IDE：`copyLinkedSessions` + `variantScope`（两个 IDE 共用一棵会话树，
 *   档位决定运行态判定与关联组的档位）；
 * - VS Code / JetBrains 插件：`copyLinkedSessions`，运行中需「关闭并重开」授权。
 *
 * 抽成纯函数是为了让这三分支被单测钉住：历史上 #195 就是被一句
 * `client === "workbuddy" || client === "vscodeExt" || client === "jetbrains"` 的白名单
 * 挡在门外——后端早已就绪，界面上却永远禁用。
 */

/** 该客户端可用哪个后端入口完成「复制 + 建立关联」。 */
export type LinkedSessionCopyKind = "workbuddy" | "codebuddyIde" | "editorPlugin";

/**
 * 是否支持「新增关联会话」。
 *
 * 四个客户端全部支持；保留这个函数（而不是直接删掉判定）是为了让「谁可以点」只有一处
 * 事实来源，并有单测覆盖——再有人按客户端写白名单漏掉一个，测试会先失败。
 */
export function supportsAddLinkedSession(client: SessionGroupClient): boolean {
  return linkedSessionCopyKind(client) !== null;
}

/** 该客户端走哪个复制入口；`null` = 不支持（当前没有这种客户端）。 */
export function linkedSessionCopyKind(client: SessionGroupClient): LinkedSessionCopyKind | null {
  if (client === "workbuddy") return "workbuddy";
  if (client === "codebuddyIde") return "codebuddyIde";
  if (client === "vscodeExt" || client === "jetbrains") return "editorPlugin";
  return null;
}

/** 该客户端的写操作是否需要在执行前询问「关闭并重开宿主」。 */
export function linkedSessionNeedsRestartPrecheck(client: SessionGroupClient): boolean {
  return restartPrecheckClient(client) !== null;
}

/** 弹窗里「来源账号」的客户端称呼（占位符与空态文案用）。 */
export function linkedSessionAccountLabel(client: SessionGroupClient): string {
  if (client === "workbuddy") return "WorkBuddy 账号";
  if (client === "jetbrains") return "JetBrains 插件账号";
  if (client === "codebuddyIde") return "CodeBuddy IDE 账号";
  return "CodeBuddy 插件账号";
}

/** 宿主名（成功提示与重开失败提示里要指名是哪个客户端）。 */
export function linkedSessionEditorLabel(client: SessionGroupClient): string {
  if (client === "workbuddy") return "WorkBuddy";
  if (client === "jetbrains") return "JetBrains IDE";
  if (client === "codebuddyIde") return "CodeBuddy IDE";
  return "VS Code";
}

/**
 * 该客户端下「来源账号」是否还需要档位过滤。
 *
 * 只有 CodeBuddy IDE 需要：两个 IDE 共用一棵会话树，但关联组按档位隔离，跨档登记会让新组
 * 在刚打开的列表里看不见。WorkBuddy 的跨档复制是有意支持的能力，插件侧存储与地区无关。
 */
export function linkedSessionScopesTargets(client: SessionGroupClient): boolean {
  return client === "codebuddyIde";
}

/**
 * 目标账号候选：排除来源账号；CodeBuddy IDE 下再按档位过滤（其余客户端不按档位收窄）。
 *
 * 顺序沿用账号库顺序（最近使用优先由账号库自身维护），不做二次排序。
 */
export function linkedSessionTargetOptions(
  accounts: AccountMeta[],
  options: {
    client: SessionGroupClient;
    sourceAccountId: string;
    /** 仅 CodeBuddy IDE 生效的档位；未选定时不做过滤（后端会再校验并给出明确原因）。 */
    variantScope?: WbVariant;
  },
): AccountMeta[] {
  const { client, sourceAccountId, variantScope } = options;
  return accounts.filter((account) => {
    if (account.id === sourceAccountId) return false;
    if (!linkedSessionScopesTargets(client)) return true;
    if (!variantScope) return true;
    return accountVariant(account) === variantScope;
  });
}

/**
 * 目标账号选择器的说明文案；`null` = 不需要额外说明。
 *
 * 「没有可用目标账号」必须给原因：选择器此时是禁用的，不给原因用户只会以为界面坏了。
 */
export function linkedSessionTargetHint(options: {
  client: SessionGroupClient;
  sourceAccountId: string;
  targetCount: number;
  variantScope?: WbVariant;
}): string | null {
  const { client, sourceAccountId, targetCount, variantScope } = options;
  if (!sourceAccountId || targetCount > 0) return null;
  if (linkedSessionScopesTargets(client) && variantScope) {
    return `账号库里没有另一个${variantLabel(variantScope)}账号可以作为目标。`;
  }
  return "账号库里没有另一个账号可以作为目标：请先添加第二个账号。";
}

/**
 * 来源会话区的空态文案。
 *
 * `dataRoot === null` 表示**未找到客户端数据目录**（≠ 该账号没有会话）：这两种情况的处理
 * 完全不同，不能都显示「该账号没有可复制的会话」。
 */
export function linkedSessionEmptySessionsHint(options: {
  client: SessionGroupClient;
  dataRoot: string | null | undefined;
}): string {
  const { client, dataRoot } = options;
  if (dataRoot !== null) return "该账号没有可复制的会话。";
  if (client === "jetbrains") {
    return "未找到插件数据目录：请先打开 IDEA / PyCharm 并登录 CodeBuddy 插件。";
  }
  if (client === "codebuddyIde") {
    return "未找到 CodeBuddy IDE 数据目录：请先打开 CodeBuddy IDE 并登录。";
  }
  if (client === "vscodeExt") {
    return "未找到插件数据目录：请先打开 VS Code 并登录 CodeBuddy 插件。";
  }
  // WorkBuddy 的数据目录由档位决定，读不到时后端已给出具体错误，这里只兜底。
  return "该账号没有可复制的会话。";
}

/**
 * 复制失败时的提示文案（`description` 为后端原因）。
 *
 * 单独抽出来是因为「没建立关联」与「完全失败」的后果不同：前者会话已经复制进目标账号，
 * 用户必须自己决定要不要手动处理，不能被一句「复制失败」盖住。
 */
export function linkedSessionCopyFailureToast(reason: string): { title: string; description?: string } {
  const trimmed = reason.trim();
  return trimmed ? { title: "复制并关联失败", description: trimmed } : { title: "复制并关联失败" };
}
