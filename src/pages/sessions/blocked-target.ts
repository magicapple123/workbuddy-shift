import type {
  SessionGroupMemberDetail,
  SessionGroupPairPreview,
  SessionGroupUnifyPlan,
  SessionSyncMode,
} from "@/lib/types";

/**
 * 「以此为准」统一确认弹窗里，**某个目标账号为什么不能写**的判定与人话翻译
 * （纯函数，供弹窗与单测共用）。
 *
 * 三件事，一件都不许在这里让步：
 * 1. 判定口径必须与 `useSessionActions.confirmUnify` 的执行前复核逐条一致——这里把目标
 *    判成「可写」而内核拒绝，用户就会看到「点了没反应」；反过来把可写的判成阻塞，则会让
 *    本来能收敛的分歧永远修不掉；
 * 2. 只翻译原因，**不放宽任何许可**：`unknown`（基线丢失 / 内容不可验证）在后端不匹配任何
 *    写入模式，覆盖不能绕过未知（`SyncVerdict::allows`），前端也不给它补一条模式；
 * 3. 被阻塞时给一条**能走通**的路（重新复制并重建关联），而不是只留一句「无法写入」。
 *
 * 死胡同体验的根因（issue #3）：`unknown` 时确认按钮不可点，界面上却没有任何出路，
 * 用户只能反复点「以此为准」然后什么也不发生。
 */

/**
 * 阻塞类别：只用于分派文案与单测断言，不参与任何权限判断。
 *
 * - `previewError`：预览请求本身失败（后端已给出原因）；
 * - `missingPreview`：既没有结果也没有错误（理论上不该发生，但绝不能静默放过）；
 * - `mismatch`：手上的检查结果不是这次会话 / 这次来源的（换组、换来源后未重新检查）；
 * - `missingToken`：缺少核对凭据，写入时会被内核拒绝；
 * - `noMode`：没有任何可用的正文写入模式（`unknown` 是主要来源：基线丢失或内容不可验证）。
 */
export type BlockedTargetKind =
  | "previewError"
  | "missingPreview"
  | "mismatch"
  | "missingToken"
  | "noMode";

/** 「重新复制并重建关联」要预填的两个账号：来源 = 当前保留的副本，目标 = 被阻塞的成员。 */
export interface RebuildLinkPrefill {
  sourceAccountId: string;
  targetAccountId: string;
}

/** 详情页发起一次「重新复制并重建关联」的请求（`prefill` 一定可用）。 */
export interface RebuildLinkRequest {
  memberId: string;
  accountName: string;
  prefill: RebuildLinkPrefill;
}

/** 一个被阻塞的目标：原因 + 是否给得出一条重建路径。 */
export interface BlockedTargetEntry {
  memberId: string;
  accountName: string;
  kind: BlockedTargetKind;
  /** 人话原因，直接上屏（含后端原文的细节）。 */
  reason: string;
  /** 重建关联所需的预填账号；`null` = 给不出这条路，见 `rebuildBlockedReason`。 */
  prefill: RebuildLinkPrefill | null;
  /** 为什么给不出重建入口；`prefill` 非空时必须为 `null`。 */
  rebuildBlockedReason: string | null;
}

/** 正文写入模式白名单：与 `confirmUnify` 里挑模式的顺序保持同一份。 */
const WRITABLE_MODES: readonly SessionSyncMode[] = ["fastForward", "overwrite", "unifyOverwrite"];

/** 统一确认弹窗里「系统为什么不覆盖」的统一措辞（被阻塞账号的公共说明）。 */
export const BLOCKED_POLICY_NOTICE =
  "系统无法确认两份内容的先后时，出于安全不会覆盖任何一份；下面这些账号因此暂时不能写入，其它账号不受影响。";

/**
 * 逐个目标算出「被阻塞」的项；`identical` 与可写的目标不会出现在结果里。
 *
 * 无阻塞时返回空数组——这正是确认按钮可用的判据（见 `unifyConfirmBlocked`）。
 */
export function describeBlockedTargets(input: {
  plan: SessionGroupUnifyPlan;
  /** 当前详情的成员：重建入口要用成员的 `accountId`，非 active 成员给不出。 */
  members: SessionGroupMemberDetail[];
}): BlockedTargetEntry[] {
  const { plan, members } = input;
  const sourceAccountId = members.find((member) => member.memberId === plan.sourceMemberId)?.accountId ?? null;
  const entries: BlockedTargetEntry[] = [];
  for (const target of plan.targets) {
    const blocked = blockingOf(plan, target);
    if (!blocked) continue;
    const member = members.find((item) => item.memberId === target.memberId) ?? null;
    const targetAccountId = member?.accountId ?? null;
    const rebuild = rebuildPath({
      member,
      sourceAccountId,
      targetAccountId,
    });
    entries.push({
      memberId: target.memberId,
      accountName: target.accountName,
      kind: blocked.kind,
      reason: blocked.reason,
      prefill: rebuild.prefill,
      rebuildBlockedReason: rebuild.reason,
    });
  }
  return entries;
}

/** 有阻塞项时确认按钮必须禁用：用户看得到按钮、也看得到不能点的原因。 */
export function unifyConfirmBlocked(entries: readonly BlockedTargetEntry[]): boolean {
  return entries.length > 0;
}

/** 单个目标的阻塞判定；`null` = 可写（或 `identical`，本来就不需要写）。 */
function blockingOf(
  plan: SessionGroupUnifyPlan,
  target: SessionGroupUnifyPlan["targets"][number],
): { kind: BlockedTargetKind; reason: string } | null {
  if (target.error) {
    return { kind: "previewError", reason: `这次检查失败了：${target.error}` };
  }
  const preview = target.preview;
  if (!preview) {
    // 没有结果也没有错误：不能当成「内容一致」放过。
    return { kind: "missingPreview", reason: "没有拿到这个账号的检查结果，请重新检查后再试。" };
  }
  if (!matchesCurrentPlan(plan, target.memberId, preview)) {
    return { kind: "mismatch", reason: "这份检查结果不是当前这次比较的，请重新检查后再试。" };
  }
  if (preview.verdict === "identical") return null;
  if (!preview.previewToken) {
    return { kind: "missingToken", reason: "这次检查的核对凭据已失效，请重新检查后再试。" };
  }
  if (!preview.availableModes.some((mode) => WRITABLE_MODES.includes(mode))) {
    return { kind: "noMode", reason: noModeReason(preview) };
  }
  return null;
}

/** 检查结果是否属于「当前这次」比较：换客户端 / 换组 / 换来源 / 换目标都算过期。 */
function matchesCurrentPlan(
  plan: SessionGroupUnifyPlan,
  targetMemberId: string,
  preview: SessionGroupPairPreview,
): boolean {
  return preview.client === plan.client
    && preview.groupId === plan.groupId
    && preview.sourceMemberId === plan.sourceMemberId
    && preview.targetMemberId === targetMemberId;
}

/**
 * 没有任何可写模式的说明：`unknown` 用「先后无法确认」，其余用「没有可用的更新方式」。
 *
 * 后端 `reason` 已经是能读懂的一句话（例如「找不到上次同步的记录，无法确认两边内容」），
 * 原样带出来，用户才知道是基线丢了还是内容读不全。
 */
function noModeReason(preview: SessionGroupPairPreview): string {
  const detail = preview.reason.trim();
  const head = preview.verdict === "unknown"
    ? "系统无法确认它与另一份内容的先后"
    : "系统没有给出可用的更新方式";
  const tail = "出于安全不会覆盖它的内容。";
  return detail ? `${head}（${detail}），${tail}` : `${head}，${tail}`;
}

/**
 * 重建路径的可用性：要同时拿到来源与目标的账号 ID，且目标必须是组里的有效成员。
 *
 * 拿不到就不给按钮，并把原因说清楚——留一个点了没反应的按钮，等于把死胡同换个位置。
 */
function rebuildPath(input: {
  member: SessionGroupMemberDetail | null;
  sourceAccountId: string | null;
  targetAccountId: string | null;
}): { prefill: RebuildLinkPrefill | null; reason: string | null } {
  const { member, sourceAccountId, targetAccountId } = input;
  if (!member || member.linkState !== "active") {
    return { prefill: null, reason: "这个账号已不是组里的有效成员，刷新详情后重试。" };
  }
  if (!targetAccountId) {
    return { prefill: null, reason: "账号库里找不到这个账号的记录，无法自动重新复制。" };
  }
  if (!sourceAccountId) {
    return { prefill: null, reason: "找不到作为来源的账号副本，请先在上方重新选择要保留的一份。" };
  }
  if (sourceAccountId === targetAccountId) {
    return { prefill: null, reason: "来源与目标是同一个账号，无法重新复制。" };
  }
  return { prefill: { sourceAccountId, targetAccountId }, reason: null };
}
