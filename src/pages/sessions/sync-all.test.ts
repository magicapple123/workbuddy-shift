import { describe, expect, it } from "vitest";

import { selectSyncAllCandidates, syncAllDialogCopy } from "./sync-all";
import type { SyncAllCandidateGroup } from "./sync-all";

/**
 * 「跨组安全批量同步」纯逻辑单测。
 *
 * 重点覆盖三件事：只把 `behind` 组放进候选（分歧 / 无法确认的组不能进批）、
 * 落后副本数按 `behindMemberCount` 汇总且老后端缺字段不崩，以及四个客户端的确认文案
 * 在「会不会关宿主」上的差异 —— 给 WorkBuddy 用户承诺关闭是空承诺，给 VS Code 用户
 * 隐瞒关闭会直接丢未保存内容。
 */

function group(
  groupId: string,
  summaryStatus: SyncAllCandidateGroup["summaryStatus"],
  behindMemberCount?: number,
): SyncAllCandidateGroup {
  return { groupId, summaryStatus, behindMemberCount };
}

describe("selectSyncAllCandidates", () => {
  it("只选 behind 组，并按 behindMemberCount 汇总副本数", () => {
    const candidates = selectSyncAllCandidates([
      group("g-behind-1", "behind", 2),
      group("g-diverge", "diverge", 0),
      group("g-latest", "latest", 0),
      group("g-behind-2", "behind", 1),
      group("g-unknown", "unknown", 0),
    ]);

    expect(candidates.groupIds).toEqual(["g-behind-1", "g-behind-2"]);
    expect(candidates.groupCount).toBe(2);
    expect(candidates.memberCount).toBe(3);
  });

  it("空列表返回 0 组 0 副本（页面据此隐藏入口）", () => {
    expect(selectSyncAllCandidates([])).toEqual({
      groupIds: [],
      groupCount: 0,
      memberCount: 0,
    });
  });

  it("缺 behindMemberCount 的老后端按 0 计，不影响是否进入候选", () => {
    const candidates = selectSyncAllCandidates([
      group("g-old", "behind"),
      group("g-new", "behind", 3),
    ]);

    expect(candidates.groupIds).toEqual(["g-old", "g-new"]);
    expect(candidates.groupCount).toBe(2);
    expect(candidates.memberCount).toBe(3);
  });
});

describe("syncAllDialogCopy", () => {
  it("WorkBuddy：只做安全快进、分歧组跳过，且不承诺关闭客户端", () => {
    const copy = syncAllDialogCopy("workbuddy", 2, 3);

    expect(copy.title).toContain("2");
    expect(copy.description).toContain("安全快进");
    expect(copy.description).toContain("跳过");
    expect(copy.description).not.toContain("关闭并重开");
  });

  it("CodeBuddy IDE：要求用户先退出，不承诺关闭", () => {
    const copy = syncAllDialogCopy("codebuddyIde", 1, 2);

    expect(copy.description).toContain("CodeBuddy IDE");
    expect(copy.description).not.toContain("关闭并重开");
  });

  it("VS Code：说明会关闭并重开，避免丢未保存内容", () => {
    const copy = syncAllDialogCopy("vscodeExt", 3, 4);

    expect(copy.description).toContain("关闭并重开");
    expect(copy.description).toContain("VS Code");
    expect(copy.title).toContain("3");
    expect(copy.busyText).toContain("VS Code");
  });

  it("JetBrains：说明会关闭并重开（IDEA / PyCharm 共用数据目录）", () => {
    const copy = syncAllDialogCopy("jetbrains", 1, 1);

    expect(copy.description).toContain("关闭并重开");
    expect(copy.description).toContain("IDEA");
    expect(copy.description).toContain("PyCharm");
  });
});
