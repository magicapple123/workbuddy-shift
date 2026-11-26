import { describe, expect, it } from "vitest";

import { buildSyncToasts } from "./notify";
import type { SyncToastReport } from "./notify";

/**
 * 同步结果提示文案的单测。
 *
 * 这块逻辑原来内联在 `use-session-actions` 的 `notifyResult` 里、直接调 sonner，因此只能靠
 * 手动点界面验收。抽出纯函数后，这里钉住四件最容易回退的事：
 * 1. 跨组成功说的是「副本 / 覆盖 N 个组」，单组说的是「会话 / 完成 N 项」，不能混；
 * 2. 覆盖组数按**实际同步成功的条目**去重，不是按准备阶段的 `groups.length`（写盘途中 `break`
 *    后，未执行的组不该被算成已覆盖）；
 * 3. 整组跳过只展开 2 条原因，其余只报数量；
 * 4. 提示的**顺序与 tone** 固定（成功 → 跳过 → 整组跳过 → 失败 → 待恢复 → 重开失败 → 空态）。
 */

const group = (groupId: string, title: string) => ({ groupId, title, targetCount: 1 });

describe("buildSyncToasts（跨组批量同步）", () => {
  it("跨组成功：说「副本」并且覆盖组数按实际条目去重（不是 groups.length）", () => {
    const toasts = buildSyncToasts({
      synced: [{ groupId: "g-1" }, { groupId: "g-2" }],
      groups: [group("g-1", "组一"), group("g-2", "组二"), group("g-3", "组三")],
    });

    // g-3 在准备清单里但没有真正写盘（例如前面出错 break），不能说「覆盖 3 个组」。
    expect(toasts).toHaveLength(1);
    expect(toasts[0]).toEqual({
      tone: "success",
      title: "已同步 2 个副本",
      description: "覆盖 2 个组",
    });
  });

  it("跨组成功：同一组的多条同步条目只算一个覆盖组", () => {
    const toasts = buildSyncToasts({
      synced: [{ groupId: "g-1" }, { groupId: "g-1" }, { groupId: "g-2" }],
      groups: [group("g-1", "组一"), group("g-2", "组二")],
    });

    expect(toasts[0]).toEqual({
      tone: "success",
      title: "已同步 3 个副本",
      description: "覆盖 2 个组",
    });
  });

  it("跨组成功：条目都没带 groupId 时退化成 groups.length（老后端）", () => {
    const toasts = buildSyncToasts({
      synced: [{}, {}],
      groups: [group("g-1", "组一"), group("g-2", "组二")],
    });

    expect(toasts[0].description).toBe("覆盖 2 个组");
  });
});

describe("buildSyncToasts（单组同步）", () => {
  it("单组成功：说「会话」并报完成项数", () => {
    const toasts = buildSyncToasts({ synced: [{}, {}, {}] });

    expect(toasts).toEqual([
      { tone: "success", title: "已同步 3 个会话", description: "完成 3 项" },
    ]);
  });

  it("给了目标名时 headline 改成「已同步到「…」」，完成项数仍在描述里", () => {
    const toasts = buildSyncToasts({ synced: [{ groupId: "g-1" }] }, "工作号");

    expect(toasts).toEqual([
      { tone: "success", title: "已同步到「工作号」", description: "完成 1 项" },
    ]);
  });
});

describe("buildSyncToasts（宿主重开与待恢复）", () => {
  it("插件侧重开：JetBrains 客户端指名 JetBrains IDE，未指定客户端时按 VS Code", () => {
    const jetbrains = buildSyncToasts({ synced: [{}], restartedEditor: true, client: "jetbrains" });
    const fallback = buildSyncToasts({ synced: [{}], restartedEditor: true });

    expect(jetbrains[0].description).toBe("完成 1 项，已重新打开 JetBrains IDE");
    // 缺省 VS Code：WorkBuddy / CodeBuddy IDE 的报告不该走到这条分支，兜底也必须是可读的宿主名。
    expect(fallback[0].description).toBe("完成 1 项，已重新打开 VS Code");
  });

  it("档位重开提示优先于宿主编号：restartedVariants 非空时不提编辑器", () => {
    const toasts = buildSyncToasts({
      synced: [{ groupId: "g-1" }, { groupId: "g-2" }],
      groups: [group("g-1", "组一"), group("g-2", "组二")],
      restartedVariants: ["cn", "ai"],
      restartedEditor: true,
      client: "vscodeExt",
    });

    expect(toasts[0].description).toBe("覆盖 2 个组，已重新打开 国内版、国际版");
  });

  it("整组跳过：只展开前 2 条原因，其余只报数量", () => {
    const toasts = buildSyncToasts({
      skippedGroups: [
        { groupId: "g-1", title: "组一", reasonCode: "diverge", message: "存在分歧" },
        { groupId: "g-2", title: "组二", reasonCode: "noSafeSource", message: "没有可用来源" },
        { groupId: "g-3", title: "", reasonCode: "detailFailed", message: "读取详情失败" },
      ],
    });

    expect(toasts).toHaveLength(1);
    expect(toasts[0].tone).toBe("warning");
    expect(toasts[0].title).toBe("跳过 3 个组");
    // 第三条落到「另有」，标题为空时退回 groupId，避免出现「：读取详情失败」。
    expect(toasts[0].description).toBe("组一：存在分歧；组二：没有可用来源；另有 1 个组跳过");
  });

  it("失败与待恢复各产生一条 error，待恢复优先用临时文件原因、没有原因时给兜底", () => {
    const withReasons = buildSyncToasts({
      errors: [{ error: "目标账号会话写入失败" }],
      needsRecovery: true,
      temporaryFiles: [{ reason: "备份目录未清理" }, { reason: "待恢复的写入未完成" }],
    });
    const noReasons = buildSyncToasts({ needsRecovery: true, temporaryFiles: [] });

    expect(withReasons).toEqual([
      { tone: "error", title: "部分会话同步失败", description: "目标账号会话写入失败" },
      { tone: "error", title: "会话操作待恢复", description: "备份目录未清理；待恢复的写入未完成" },
    ]);
    expect(noReasons).toEqual([
      { tone: "error", title: "会话操作待恢复", description: "已保留恢复所需材料。" },
    ]);
  });

  it("editorError 用客户端标签，不能把 JetBrains 用户说成 VS Code", () => {
    const toasts = buildSyncToasts({ synced: [{}], editorError: "IDE 启动超时", client: "jetbrains" });

    expect(toasts).toEqual([
      { tone: "success", title: "已同步 1 个会话", description: "完成 1 项" },
      { tone: "warning", title: "JetBrains IDE 未能自动重新打开", description: "IDE 启动超时" },
    ]);
  });
});

describe("buildSyncToasts（顺序与空态）", () => {
  it("顺序固定：成功 → 单条跳过 → 整组跳过 → 失败 → 待恢复 → 重开失败", () => {
    const report: SyncToastReport = {
      synced: [{ groupId: "g-1" }],
      skipped: [{}],
      skippedGroups: [{ groupId: "g-2", title: "组二", reasonCode: "diverge", message: "存在分歧" }],
      errors: [{ error: "一个失败" }],
      needsRecovery: true,
      temporaryFiles: [{ reason: "待恢复" }],
      editorError: "未能重开",
      client: "vscodeExt",
      groups: [group("g-1", "组一")],
    };

    const toasts = buildSyncToasts(report);

    expect(toasts.map((item) => item.tone)).toEqual([
      "success",
      "warning",
      "warning",
      "error",
      "error",
      "warning",
    ]);
    expect(toasts.map((item) => item.title)).toEqual([
      "已同步 1 个副本",
      "有 1 项跳过",
      "跳过 1 个组",
      "部分会话同步失败",
      "会话操作待恢复",
      "VS Code 未能自动重新打开",
    ]);
    expect(toasts[1].description).toBe("预览过期或复核后不再符合安全条件的项不会计为成功。");
  });

  it("什么都没发生时只返回一条 info 空态（不叠任何其它提示）", () => {
    expect(buildSyncToasts({})).toEqual([{ tone: "info", title: "当前没有需要同步的副本" }]);
    // 显式空数组 / undefined 字段与缺省等价。
    expect(buildSyncToasts({ synced: [], skipped: [], errors: [], groups: [], skippedGroups: [] }))
      .toEqual([{ tone: "info", title: "当前没有需要同步的副本" }]);
  });

  it("有跳过但没有成功时不出空态（空态会盖住真实结论）", () => {
    const toasts = buildSyncToasts({ skipped: [{}] });

    expect(toasts).toHaveLength(1);
    expect(toasts[0].tone).toBe("warning");
  });
});
