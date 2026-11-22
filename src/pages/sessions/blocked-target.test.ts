import { describe, expect, it } from "vitest";
import type {
  SessionGroupMemberDetail,
  SessionGroupPairPreview,
  SessionGroupUnifyPlan,
} from "@/lib/types";
import {
  BLOCKED_POLICY_NOTICE,
  describeBlockedTargets,
  unifyConfirmBlocked,
} from "./blocked-target";

/** 预览结果：默认是可写的快进，逐项覆盖出各种「不能写」的形态。 */
function preview(overrides: Partial<SessionGroupPairPreview> = {}): SessionGroupPairPreview {
  return {
    client: "workbuddy",
    variantScope: "cn",
    groupId: "group-1",
    sourceMemberId: "member-a",
    targetMemberId: "member-b",
    verdict: "fastForward",
    availableModes: ["fastForward"],
    previewToken: "token-1",
    reason: "目标未偏离基线，可安全补齐",
    recordCount: { source: 12, target: 9, baseline: 9 },
    extraTargetCount: 0,
    ...overrides,
  };
}

/** 成员：`accountId` 参与「能不能给重建入口」的判定。 */
function member(overrides: Partial<SessionGroupMemberDetail> = {}): SessionGroupMemberDetail {
  return {
    memberId: "member-b",
    accountId: "account-b",
    uid: "uid-b",
    sessionId: "session-1",
    accountName: "账号 B",
    variant: "cn",
    linkState: "active",
    versionStatus: "behind",
    title: "会话",
    projectLabel: "项目",
    updatedAt: 0,
    recordCount: 10,
    contentState: "ready",
    reason: "",
    canBeSource: true,
    ...overrides,
  };
}

/** 一次「以 A 为准 → 统一到 B」的方案。 */
function plan(targetPreview: SessionGroupPairPreview | null, error: string | null = null): SessionGroupUnifyPlan {
  return {
    client: "workbuddy",
    groupId: "group-1",
    sourceMemberId: "member-a",
    sourceName: "账号 A",
    targets: [{ memberId: "member-b", accountName: "账号 B", preview: targetPreview, error }],
  };
}

const members: SessionGroupMemberDetail[] = [
  member({ memberId: "member-a", accountId: "account-a", accountName: "账号 A", versionStatus: "latest" }),
  member(),
];

describe("被阻塞目标的判定", () => {
  it("判定 unknown（没有任何可用模式）→ 阻塞，人话原因保留后端原文并说明不会覆盖", () => {
    const entries = describeBlockedTargets({
      plan: plan(preview({
        verdict: "unknown",
        availableModes: [],
        reason: "找不到上次同步的记录，无法确认两边内容",
      })),
      members,
    });

    expect(entries).toHaveLength(1);
    expect(entries[0].kind).toBe("noMode");
    expect(entries[0].reason).toContain("找不到上次同步的记录，无法确认两边内容");
    expect(entries[0].reason).toContain("无法确认它与另一份内容的先后");
    expect(entries[0].reason).toContain("出于安全不会覆盖");
    expect(BLOCKED_POLICY_NOTICE).toContain("不会覆盖");
  });

  it("unknown 时仍然给出一条可走通的重建路径（来源 = 当前保留的副本）", () => {
    const entries = describeBlockedTargets({
      plan: plan(preview({ verdict: "unknown", availableModes: [] })),
      members,
    });

    expect(entries[0].prefill).toEqual({ sourceAccountId: "account-a", targetAccountId: "account-b" });
    expect(entries[0].rebuildBlockedReason).toBeNull();
  });

  it("预览结果缺失（既没有结果也没有错误）→ 阻塞，原因不能是空话", () => {
    const entries = describeBlockedTargets({ plan: plan(null), members });

    expect(entries).toHaveLength(1);
    expect(entries[0].kind).toBe("missingPreview");
    expect(entries[0].reason.length).toBeGreaterThan(0);
    expect(entries[0].reason).toContain("重新检查");
  });

  it("预览请求失败 → 阻塞，原因带上后端原文", () => {
    const entries = describeBlockedTargets({
      plan: plan(null, "来源会话内容缺失或无法确认，不能复制"),
      members,
    });

    expect(entries).toHaveLength(1);
    expect(entries[0].kind).toBe("previewError");
    expect(entries[0].reason).toContain("来源会话内容缺失或无法确认，不能复制");
  });

  it("缺少核对凭据（previewToken 为空）→ 阻塞，且不把它当成可写", () => {
    const entries = describeBlockedTargets({
      plan: plan(preview({ previewToken: null })),
      members,
    });

    expect(entries).toHaveLength(1);
    expect(entries[0].kind).toBe("missingToken");
    expect(entries[0].reason).toContain("重新检查");
  });

  it("检查结果与当前会话不匹配 → 阻塞（客户端 / 会话组 / 来源 三项各自成立）", () => {
    const cases: SessionGroupPairPreview[] = [
      preview({ client: "codebuddyIde" }),
      preview({ groupId: "group-2" }),
      preview({ sourceMemberId: "member-other" }),
      preview({ targetMemberId: "member-other" }),
    ];

    for (const targetPreview of cases) {
      const entries = describeBlockedTargets({ plan: plan(targetPreview), members });
      expect(entries).toHaveLength(1);
      expect(entries[0].kind).toBe("mismatch");
    }
  });

  it("内容一致（identical）不进阻塞列表：不展示重建入口，确认路径不受影响", () => {
    const entries = describeBlockedTargets({
      plan: plan(preview({ verdict: "identical", availableModes: [], previewToken: null })),
      members,
    });

    expect(entries).toEqual([]);
    expect(unifyConfirmBlocked(entries)).toBe(false);
  });

  it("可写的目标（fastForward + 凭据齐全）不阻塞", () => {
    const entries = describeBlockedTargets({ plan: plan(preview()), members });

    expect(entries).toEqual([]);
  });

  it("有阻塞时确认按钮禁用：即便混着可写的目标也只按「有阻塞」处理", () => {
    const mixed: SessionGroupUnifyPlan = {
      ...plan(preview({ verdict: "unknown", availableModes: [] })),
      targets: [
        { memberId: "member-b", accountName: "账号 B", preview: preview({ verdict: "unknown", availableModes: [] }), error: null },
        { memberId: "member-c", accountName: "账号 C", preview: preview({ targetMemberId: "member-c" }), error: null },
      ],
    };
    const entries = describeBlockedTargets({ plan: mixed, members });

    expect(entries.map((entry) => entry.memberId)).toEqual(["member-b"]);
    expect(unifyConfirmBlocked(entries)).toBe(true);
  });
});

describe("重建入口的可用性", () => {
  it("目标账号没有 accountId → 不给入口，但要说明原因", () => {
    const entries = describeBlockedTargets({
      plan: plan(preview({ verdict: "unknown", availableModes: [] })),
      members: [members[0], member({ accountId: null })],
    });

    expect(entries[0].prefill).toBeNull();
    expect(entries[0].rebuildBlockedReason).toContain("账号库");
  });

  it("目标不再是有效成员（已解除关联）→ 不给入口", () => {
    const entries = describeBlockedTargets({
      plan: plan(preview({ verdict: "unknown", availableModes: [] })),
      members: [members[0], member({ linkState: "stale" })],
    });

    expect(entries[0].prefill).toBeNull();
    expect(entries[0].rebuildBlockedReason).toContain("有效成员");
  });

  it("找不到来源成员 → 不给入口，并提示先重新选择来源", () => {
    const entries = describeBlockedTargets({
      plan: plan(preview({ verdict: "unknown", availableModes: [] })),
      members: [member()],
    });

    expect(entries[0].prefill).toBeNull();
    expect(entries[0].rebuildBlockedReason).toContain("来源");
  });
});
