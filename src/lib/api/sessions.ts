import type {
  Session,
  SessionCopyReport,
  SessionLinksPreview,
  SessionSyncMode,
  SessionGroupClient,
  SessionGroupList,
  SessionGroupDetail,
  SessionGroupPairPreview,
  SessionGroupActionReport,
  SessionGroupPruneReport,
  WbVariant,
} from "../types";
import { DEMO_UNAVAILABLE_MESSAGE, demoModeEnabled } from "../demo-mode";
import { call, variantArgs } from "./core";

/** 当前登录态的会话列表；`variant` 缺省为国内版。 */
export function listSessions(variant?: WbVariant): Promise<{
  sessions: Session[];
  current: string | null;
}> {
  return call("list_sessions", variantArgs(variant));
}

/** 指定账号名下的会话列表（会话管理页的源账号视角）；账号不存在时后端返回明确错误。 */
export function listAccountSessions(accountId: string, client?: SessionGroupClient): Promise<{
  /** 插件侧会话没有 `cwd`（只有 `workspaceHash`）：调用方补齐默认值后再交给会话树。 */
  sessions: (Omit<Session, "cwd"> & { cwd?: string })[];
  current: string | null;
  variant?: WbVariant;
  /** 插件侧：插件数据仓根目录（`null` = 未找到目录，与「该账号无会话」区分）。 */
  dataRoot?: string | null;
  sourceUid?: string;
}> {
  return call("list_account_sessions", { accountId, ...(client ? { client } : {}) });
}

/**
 * 跨档把会话从**显式源账号**复制到**显式目标账号**（会话管理页用）。
 *
 * 与 `copySessions` 同形，只多一个 `sourceAccountId`：源可为国内版或国际版账号，
 * 不再要求源是当前登录账号；报告另带 `sourceVariant` / `targetVariant`。
 */
export function copySessionsCross(
  sourceAccountId: string,
  targetAccountId: string,
  sessionIds: string[],
): Promise<SessionCopyReport & { variant?: WbVariant }> {
  return call("copy_sessions_cross", { sourceAccountId, targetAccountId, sessionIds });
}

/**
 * 预览「当前账号 → 目标账号」可同步的关联会话（只读）。
 *
 * 默认勾选与可选模式都来自后端：前端只按 `defaultChecked` / `availableModes` 渲染，
 * 不自行扩大权限。`variant` 缺省由后端取目标账号自身档位。
 */
export function sessionLinksPreview(
  targetAccountId: string,
  variant?: WbVariant,
): Promise<SessionLinksPreview> {
  const args: Record<string, unknown> = { targetAccountId };
  if (variant === "ai") args.variant = variant;
  return call("session_links_preview", args);
}

export function listSessionGroups(client: SessionGroupClient, variantScope?: WbVariant): Promise<SessionGroupList> {
  return call("list_session_groups", { client, ...(variantScope ? { variantScope } : {}) });
}

export function getSessionGroup(client: SessionGroupClient, groupId: string, variantScope?: WbVariant): Promise<SessionGroupDetail> {
  return call("get_session_group", { client, groupId, ...(variantScope ? { variantScope } : {}) });
}

export function previewSessionGroupPair(args: {
  client: SessionGroupClient;
  groupId: string;
  sourceMemberId: string;
  targetMemberId: string;
  variantScope?: WbVariant;
}): Promise<SessionGroupPairPreview> {
  return call("preview_session_group_pair", args as unknown as Record<string, unknown>);
}

export function syncSessionGroupPair(args: {
  client: SessionGroupClient;
  groupId: string;
  sourceMemberId: string;
  targetMemberId: string;
  previewToken: string;
  mode: SessionSyncMode;
  variantScope?: WbVariant;
  /** 已获用户授权（确认框）时传 `true`：插件侧运行中允许关闭并重开 VS Code。 */
  restart?: boolean;
}): Promise<SessionGroupActionReport> {
  if (demoModeEnabled) return Promise.reject(new Error(DEMO_UNAVAILABLE_MESSAGE));
  return call("sync_session_group_pair", args as unknown as Record<string, unknown>);
}

export function syncSessionGroupUnify(args: {
  client: "workbuddy" | "vscodeExt";
  groupId: string;
  sourceMemberId: string;
  targets: { targetMemberId: string; previewToken: string; mode: SessionSyncMode }[];
  /** 已获用户授权（确认框）时传 `true`：插件侧运行中允许关闭并重开 VS Code。 */
  restart?: boolean;
}): Promise<SessionGroupActionReport> {
  if (demoModeEnabled) return Promise.reject(new Error(DEMO_UNAVAILABLE_MESSAGE));
  return call("sync_session_group_unify", args as unknown as Record<string, unknown>);
}

export function syncSessionGroupSafeBatch(client: SessionGroupClient, groupId: string, variantScope?: WbVariant, restart?: boolean): Promise<SessionGroupActionReport> {
  if (demoModeEnabled) return Promise.reject(new Error(DEMO_UNAVAILABLE_MESSAGE));
  return call("sync_session_group_safe_batch", { client, groupId, ...(variantScope ? { variantScope } : {}), ...(restart ? { restart: true } : {}) });
}

export function addSessionGroupMember(args: {
  client: SessionGroupClient;
  groupId: string;
  sourceMemberId: string;
  targetAccountId: string;
  variantScope?: WbVariant;
  /** 已获用户授权（确认框）时传 `true`：插件侧运行中允许关闭并重开 VS Code。 */
  restart?: boolean;
}): Promise<{ status: "linked" | "alreadyLinked" | "copiedUnlinked" | "failed"; editorError?: string; restartedEditor?: boolean; /** 内容缺失 / 索引丢失而被跳过的会话。 */ skipped?: { id: string; error: string }[]; [key: string]: unknown }> {
  if (demoModeEnabled) return Promise.reject(new Error(DEMO_UNAVAILABLE_MESSAGE));
  return call("add_session_group_member", args as unknown as Record<string, unknown>);
}

/** 插件：把来源账号的勾选会话复制到目标账号并登记关联（无现成组则新建关联组）。 */
export function copyLinkedSessions(args: {
  client: SessionGroupClient;
  sourceAccountId: string;
  targetAccountId: string;
  sessionIds: string[];
  /** 已获用户授权（确认框）时传 `true`：运行中允许关闭并重开 VS Code。 */
  restart?: boolean;
}): Promise<{ status: "linked" | "alreadyLinked" | "copiedUnlinked" | "failed"; editorError?: string; restartedEditor?: boolean; [key: string]: unknown }> {
  if (demoModeEnabled) return Promise.reject(new Error(DEMO_UNAVAILABLE_MESSAGE));
  return call("copy_linked_sessions", args as unknown as Record<string, unknown>);
}

/**
 * 取消关联：把成员从会话组移除（只解除管理关系，不删除账号内的会话内容）。
 *
 * `groupRemoved` 表示移除后组内已无成员，该组已被删除。
 */
export function unlinkSessionGroupMember(args: {
  client: SessionGroupClient;
  groupId: string;
  memberId: string;
  variantScope?: WbVariant;
}): Promise<{ status: "removed" | "groupRemoved"; client: SessionGroupClient; groupId: string; memberId: string; remaining: number }> {
  if (demoModeEnabled) return Promise.reject(new Error(DEMO_UNAVAILABLE_MESSAGE));
  return call("unlink_session_group_member", args as unknown as Record<string, unknown>);
}

/**
 * 删除会话组：组内所有成员一起解除关联（只解除管理关系，不删除账号内的会话内容）。
 */
export function deleteSessionGroup(args: {
  client: SessionGroupClient;
  groupId: string;
  variantScope?: WbVariant;
}): Promise<{ status: "groupRemoved"; client: SessionGroupClient; groupId: string; removed: number }> {
  if (demoModeEnabled) return Promise.reject(new Error(DEMO_UNAVAILABLE_MESSAGE));
  return call("delete_session_group", args as unknown as Record<string, unknown>);
}

/**
 * 批量清理失效关联组：组内**所有**成员指向的会话都已不存活时才删整组。
 *
 * `dryRun: true` 只统计不写盘（页面先用它取数量再弹确认框）；`dryRun: false` 会先备份
 * 关联记录再写盘。只删除关联关系，不删除任何账号内的会话内容。
 */
export function pruneSessionGroups(args: {
  client: SessionGroupClient;
  variantScope?: WbVariant;
  dryRun: boolean;
}): Promise<SessionGroupPruneReport> {
  if (demoModeEnabled) return Promise.reject(new Error(DEMO_UNAVAILABLE_MESSAGE));
  return call("prune_session_groups", args as unknown as Record<string, unknown>);
}
