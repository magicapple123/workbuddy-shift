import { describe, expect, it, vi } from "vitest";

import {
  attemptPrune,
  blockedPruneReason,
  canPruneGroups,
  probePrune,
  pruneConfirmDescription,
  pruneFailureDescription,
  runPrune,
  showPruneEntry,
  supportsPrune,
} from "./prune";
import type { PruneRequest } from "./prune";
import type { SessionGroupPruneReport } from "@/lib/types";

/**
 * 「清理失效关联组」入口的纯逻辑单测。
 *
 * 重点覆盖三件容易出错的事：入口在三个客户端都出现（后端按客户端分派存活判定）、
 * dry-run 的统计结果如何变成二次确认文案，以及后端拒绝原因如何原样透传到提示文案
 * （不能吞成泛化的「清理失败」）。
 */

describe("supportsPrune", () => {
  it("四个客户端都支持批量清理（判定依据由后端按客户端分派）", () => {
    expect(supportsPrune("workbuddy")).toBe(true);
    expect(supportsPrune("vscodeExt")).toBe(true);
    expect(supportsPrune("codebuddyIde")).toBe(true);
    // JetBrains 插件与 VS Code 插件同构：会话行在工作区索引里，后端按 spec 判定。
    expect(supportsPrune("jetbrains")).toBe(true);
  });
});

describe("canPruneGroups", () => {
  it("列表非空且空闲时可用", () => {
    expect(
      canPruneGroups({ client: "workbuddy", groupCount: 3, deadGroupCount: 0, busy: false }),
    ).toBe(true);
  });

  it("全部组都失效（列表为空但隐藏了失效组）时仍然可用", () => {
    // 否则用户永远点不到清理入口：这正是最需要清理的状态。
    expect(
      canPruneGroups({ client: "workbuddy", groupCount: 0, deadGroupCount: 2, busy: false }),
    ).toBe(true);
  });

  it("既没有可见组也没有失效组时不显示入口", () => {
    expect(
      canPruneGroups({ client: "workbuddy", groupCount: 0, deadGroupCount: 0, busy: false }),
    ).toBe(false);
  });

  it("有动作在跑时禁用，避免并发写关联存储", () => {
    expect(
      canPruneGroups({ client: "workbuddy", groupCount: 3, deadGroupCount: 1, busy: true }),
    ).toBe(false);
  });

  it("清理被阻塞（后端说点了必然整次失败）时禁用，并保留原因", () => {
    const blockedReason =
      "无法判定「VS Code 插件」账号 uid-ext 的会话：该账号的会话索引只有部分工作区可读，损坏的分片里可能还有该会话，已取消清理。";

    expect(
      canPruneGroups({ client: "vscodeExt", groupCount: 3, deadGroupCount: 1, busy: false, blockedReason }),
    ).toBe(false);
    // 入口仍然展示：只是禁用 + 说明原因，不能整片隐藏（否则用户以为没有失效组）。
    expect(
      showPruneEntry({ client: "vscodeExt", groupCount: 3, deadGroupCount: 1 }),
    ).toBe(true);
  });

  it("阻塞原因只是空白或缺失时按「可清理」处理", () => {
    expect(
      canPruneGroups({ client: "workbuddy", groupCount: 3, deadGroupCount: 0, busy: false, blockedReason: "   " }),
    ).toBe(true);
    expect(
      canPruneGroups({ client: "workbuddy", groupCount: 3, deadGroupCount: 0, busy: false, blockedReason: null }),
    ).toBe(true);
  });

  it("插件与 IDE 侧同样提供入口（后端已能按工作区索引判定）", () => {
    expect(
      canPruneGroups({ client: "vscodeExt", groupCount: 3, deadGroupCount: 0, busy: false }),
    ).toBe(true);
    expect(
      canPruneGroups({ client: "codebuddyIde", groupCount: 3, deadGroupCount: 0, busy: false }),
    ).toBe(true);
    // 没有可清理对象时仍然不显示。
    expect(
      canPruneGroups({ client: "vscodeExt", groupCount: 0, deadGroupCount: 0, busy: false }),
    ).toBe(false);
  });
});

describe("showPruneEntry（入口是否出现）", () => {
  it("有可清理对象就出现：有可见组或有被隐藏的失效组", () => {
    expect(showPruneEntry({ client: "workbuddy", groupCount: 2, deadGroupCount: 0 })).toBe(true);
    expect(showPruneEntry({ client: "vscodeExt", groupCount: 0, deadGroupCount: 3 })).toBe(true);
  });

  it("没有任何对象时不出现（与「能不能点」共用同一份可清理口径）", () => {
    expect(showPruneEntry({ client: "codebuddyIde", groupCount: 0, deadGroupCount: 0 })).toBe(false);
  });
});

describe("blockedPruneReason（阻塞原因归一化）", () => {
  it("非空字符串原样返回（含首尾空白已被去掉）", () => {
    expect(blockedPruneReason("  无法判定会话  ")).toBe("无法判定会话");
  });

  it("缺失 / 空串 / 纯空白都当作没有阻塞", () => {
    expect(blockedPruneReason(undefined)).toBeNull();
    expect(blockedPruneReason(null)).toBeNull();
    expect(blockedPruneReason("")).toBeNull();
    expect(blockedPruneReason("  \n ")).toBeNull();
  });
});

describe("attemptPrune（点击入口时的第一道闸）", () => {
  it("被阻塞时一次请求都不发（后端会整次拒绝，先请求只会制造误导性失败提示）", async () => {
    const { request, dryRuns } = recordingRequest([]);

    const attempt = await attemptPrune({ blockedReason: "该账号的会话索引只有部分工作区可读", request });

    expect(attempt.status).toBe("blocked");
    expect(attempt.status === "blocked" ? attempt.reason : "").toContain("只有部分工作区可读");
    expect(dryRuns).toEqual([]);
  });

  it("未阻塞时正常做 dry-run 统计", async () => {
    const { request, dryRuns } = recordingRequest([report(2, 3)]);

    const attempt = await attemptPrune({ blockedReason: null, request });

    expect(attempt.status).toBe("ready");
    expect(dryRuns).toEqual([true]);
  });

  it("未阻塞但没有失效组时给出 empty（不打开确认框）", async () => {
    const { request, dryRuns } = recordingRequest([report(0)]);

    const attempt = await attemptPrune({ blockedReason: "", request });

    expect(attempt.status).toBe("empty");
    expect(dryRuns).toEqual([true]);
  });
});

describe("pruneFailureDescription（后端拒绝原因透传）", () => {
  it("索引不可判定时原样展示后端原因（哪个账号、为什么判不了、已取消清理）", () => {
    const reason =
      "无法判定「VS Code 插件」账号 uid-ext 的会话：该账号的会话索引无法解析（index.json 缺失或已损坏），已取消清理。请先打开该客户端确认账号数据正常后重试。";

    const text = pruneFailureDescription(reason);

    // 逐字透传：替换成泛化的「清理失败」会让用户以为可以重试，真实原因永远看不到。
    expect(text).toBe(reason);
    expect(text).toContain("无法判定「VS Code 插件」账号 uid-ext 的会话");
    expect(text).toContain("已取消清理");
  });

  it("历史目录缺失（Absent）与数据根不可解析的原因同样透传", () => {
    const absent =
      "无法判定「CodeBuddy IDE」账号 uid-ide 的会话：找不到该账号的会话历史目录，已取消清理。请先打开该客户端确认账号数据正常后重试。";
    const noRoot =
      "未找到 VS Code CodeBuddy 插件的会话数据目录，无法判定会话是否仍然存在，已取消清理。请先打开 VS Code 并登录该插件后重试。";

    expect(pruneFailureDescription(absent)).toBe(absent);
    expect(pruneFailureDescription(noRoot)).toBe(noRoot);
  });

  it("去掉首尾空白；后端没给原因时给出可操作兜底，而不是空描述", () => {
    expect(pruneFailureDescription("  无法判定会话  \n")).toBe("无法判定会话");
    expect(pruneFailureDescription("   ")).toContain("请确认客户端数据目录可读后重试");
  });
});

describe("pruneConfirmDescription", () => {
  it("写明可清理的组数、账号副本数与「只删关联」承诺", () => {
    const text = pruneConfirmDescription({
      dryRun: true,
      removedGroups: 4,
      removedMembers: 9,
      groupIds: ["g-1", "g-2", "g-3", "g-4"],
    });
    expect(text).toContain("可清理 4 组");
    expect(text).toContain("9 个账号副本");
    expect(text).toContain("只删除关联关系，不删除任何账号内的会话内容");
    expect(text).toContain("备份");
  });

  it("后端返回异常负数时不显示负数量", () => {
    const text = pruneConfirmDescription({
      dryRun: true,
      removedGroups: -1,
      removedMembers: -2,
      groupIds: [],
    });
    expect(text).toContain("可清理 0 组");
    expect(text).toContain("0 个账号副本");
  });
});

// ---------------------------------------------------------------------------
// 确认流程：dry-run → 二次确认 → 真删 → 刷新
//
// 这五条断言对应页面上真实的两步交互（入口按钮 → AlertDialog → 确认按钮）。这里驱动的是
// 页面用的同一份流程函数（`use-session-actions` 只是把它们接到 React 状态与 toast 上），
// 因此不需要 DOM、也不依赖 React 内部结构。
// ---------------------------------------------------------------------------

const report = (removedGroups: number, removedMembers = 0): SessionGroupPruneReport => ({
  dryRun: true,
  removedGroups,
  removedMembers,
  groupIds: Array.from({ length: removedGroups }, (_, index) => `g-${index}`),
});

/** 记录每次请求的 dryRun，便于断言「只发了几次」「第二次是 false」。 */
function recordingRequest(replies: SessionGroupPruneReport[]): {
  request: PruneRequest;
  dryRuns: boolean[];
} {
  const dryRuns: boolean[] = [];
  const request: PruneRequest = async (dryRun) => {
    dryRuns.push(dryRun);
    const reply = replies.shift();
    if (!reply) throw new Error("没有更多预设响应");
    return reply;
  };
  return { request, dryRuns };
}

describe("probePrune（dry-run 统计）", () => {
  it("数量为 0 时返回 null：不打开确认框", async () => {
    const { request, dryRuns } = recordingRequest([report(0)]);

    await expect(probePrune(request)).resolves.toBeNull();
    expect(dryRuns).toEqual([true]);
  });

  it("数量大于 0 时返回统计结果，供确认框文案使用", async () => {
    const { request, dryRuns } = recordingRequest([report(128, 511)]);

    const target = await probePrune(request);

    expect(target?.removedGroups).toBe(128);
    expect(target?.removedMembers).toBe(511);
    expect(dryRuns).toEqual([true]);
  });

  it("取消确认框（丢弃 dry-run 结果）不会发出第二次请求", async () => {
    const { request, dryRuns } = recordingRequest([report(3, 5)]);
    const target = await probePrune(request);

    // 取消 = 用户丢弃这次统计结果，页面只把 pruneTarget 置空，不再调用流程。
    expect(target).not.toBeNull();
    expect(dryRuns).toEqual([true]);
  });
});

describe("runPrune（真删与刷新）", () => {
  it("确认清理：先 dry-run 再真删，共 2 次请求且第二次是 dryRun:false", async () => {
    const { request, dryRuns } = recordingRequest([
      report(2, 4),
      { ...report(2, 4), dryRun: false },
    ]);
    const reload = vi.fn().mockResolvedValue(undefined);

    // 入口按钮点击 → dry-run 统计
    const target = await probePrune(request);
    expect(target).not.toBeNull();
    // 确认框「确认清理」→ 真删 + 刷新
    const outcome = await runPrune(request, reload);

    expect(dryRuns).toEqual([true, false]);
    expect(outcome.report.removedGroups).toBe(2);
    expect(outcome.reloadError).toBeNull();
    expect(reload).toHaveBeenCalledTimes(1);
  });

  it("刷新列表失败不改写清理结论：不抛出，只回报 reloadError", async () => {
    const failure = new Error("list failed");
    const { request } = recordingRequest([{ ...report(1, 2), dryRun: false }]);

    const outcome = await runPrune(request, () => Promise.reject(failure));

    // 关键：清理已落盘，流程必须正常返回（否则页面会再弹一条「清理失败」）。
    expect(outcome.report.removedGroups).toBe(1);
    expect(outcome.reloadError).toBe(failure);
  });

  it("真删请求本身失败时向上抛出（由调用方提示失败并保留确认框）", async () => {
    const failure = new Error("prune failed");
    const request: PruneRequest = () => Promise.reject(failure);

    await expect(runPrune(request, () => Promise.resolve())).rejects.toBe(failure);
  });
});
