import { describe, expect, it } from "vitest";

import {
  buildSessionTree,
  groupSessionsBySpace,
  isWorkBuddyTimestampDir,
  sessionCwdTitle,
  sessionFolderLabel,
} from "./session-tree";
import type { Session } from "@/lib/types";

/**
 * 会话树分组纯函数单测（T1：按 `space_id` 分组，修「日期空间」）。
 *
 * 只覆盖不依赖 DOM 的逻辑：同空间多 cwd 合组、无 `spaceId` 回退 cwd、同名空间不合并、
 * 无空间的客户端时间戳目录归「未分组」、任务 / 空间分类仍只看 `isPlayground`。
 */

function session(overrides: Partial<Session> & { id: string }): Session {
  return {
    title: `会话 ${overrides.id}`,
    cwd: "/ws/a",
    updatedAt: 1_700_000_000_000,
    hasHistory: true,
    isPlayground: false,
    ...overrides,
  };
}

const DATE_DIR = "D:/WorkBuddy_data/spaces/2026_11_16_13-46-13";
/** 客户端给「无空间」会话分配的临时工作目录（真实库形状）。 */
const LEGACY_WORKSPACE_DIR = "D:/WorkBuddy_data/2026-11-15-01-27-23";

describe("groupSessionsBySpace", () => {
  it("同一空间的会话 cwd 不同也合成一组", () => {
    const groups = groupSessionsBySpace([
      session({ id: "s1", cwd: "D:/repos/workbuddy-shift", spaceId: "space-1", spaceName: "workbuddy-shift" }),
      session({ id: "s2", cwd: "D:/repos/workbuddy-shift", spaceId: "space-1", spaceName: "workbuddy-shift" }),
      session({ id: "s3", cwd: DATE_DIR, spaceId: "space-1", spaceName: "workbuddy-shift" }),
    ]);

    expect(groups).toHaveLength(1);
    expect(groups[0].label).toBe("workbuddy-shift");
    expect(groups[0].sessions.map((item) => item.id)).toEqual(["s1", "s2", "s3"]);
    // 修复前这里会出现一个名为日期目录的空间。
    expect(groups.some((group) => group.label === "2026_11_16_13-46-13")).toBe(false);
  });

  it("没有 spaceId 时回退 cwd 文件夹分组（老库行为不变）", () => {
    const groups = groupSessionsBySpace([
      session({ id: "a", cwd: "/Users/me/projects/notes" }),
      session({ id: "b", cwd: "/Users/me/archive/notes" }),
      session({ id: "c", cwd: "" }),
    ]);

    // 两个 cwd 末段同名但路径不同：老库按完整 cwd 分成两组，标签都一样。
    expect(groups.map((group) => group.label)).toEqual(["notes", "notes", "未分组"]);
    expect(groups.map((group) => group.sessions.length)).toEqual([1, 1, 1]);
    expect(sessionFolderLabel("")).toBe("未分组");
  });

  it("老库不启用「未分组」：时间戳目录照旧按文件夹名分组", () => {
    const groups = groupSessionsBySpace([
      session({ id: "legacy", cwd: LEGACY_WORKSPACE_DIR }),
    ]);

    expect(groups.map((group) => group.label)).toEqual(["2026-11-15-01-27-23"]);
  });

  it("空白 spaceId 视为缺失，回退 cwd 文件夹名", () => {
    const groups = groupSessionsBySpace([
      session({ id: "blank", cwd: "/ws/x", spaceId: "   ", spaceName: "被忽略的名字" }),
    ]);

    expect(groups).toHaveLength(1);
    expect(groups[0].label).toBe("x");
  });

  it("无空间的时间戳工作目录归「未分组」，不产生日期空间", () => {
    const groups = groupSessionsBySpace([
      // 真实库形状：space_id 为空 + cwd 是数据根下的纯时间戳目录。
      session({ id: "ts1", cwd: LEGACY_WORKSPACE_DIR, spaceId: null, spaceName: null }),
      session({ id: "w1", cwd: "D:/repos/workbuddy-shift", spaceId: "space-w", spaceName: "workbuddy-shift" }),
      session({ id: "ts2", cwd: "D:/WorkBuddy_data/2026-11-09-16-00-55", spaceId: null }),
    ]);

    expect(groups.map((group) => group.label)).toEqual(["workbuddy-shift", "未分组"]);
    // 两条时间戳会话合进同一格，且「未分组」固定排最后（尽管它出现在最前面）。
    expect(groups[1].sessions.map((item) => item.id)).toEqual(["ts1", "ts2"]);
    expect(groups.some((group) => group.label === "2026-11-15-01-27-23")).toBe(false);
  });

  it("父目录不是数据根的时间戳目录仍按文件夹名分组", () => {
    const groups = groupSessionsBySpace([
      // 用户自己名叫时间戳的项目：不能被吞进「未分组」。
      session({ id: "project", cwd: "D:/projects/2026-11-15-01-27-23", spaceId: null }),
      // 真实空间目录多一层且用下划线命名：同样不是客户端临时目录。
      session({ id: "spaceDir", cwd: DATE_DIR, spaceId: null }),
    ]);

    expect(groups.map((group) => group.label)).toEqual([
      "2026-11-15-01-27-23",
      "2026_11_16_13-46-13",
    ]);
    expect(groups.every((group) => group.label !== "未分组")).toBe(true);
  });

  it("同名但 id 不同的空间不合并、也不加后缀", () => {
    const groups = groupSessionsBySpace([
      session({ id: "a", cwd: "/x/one", spaceId: "space-1", spaceName: "workbuddy-shift" }),
      session({ id: "b", cwd: "/x/two", spaceId: "space-2", spaceName: "workbuddy-shift" }),
    ]);

    expect(groups).toHaveLength(2);
    expect(groups.map((group) => group.label)).toEqual(["workbuddy-shift", "workbuddy-shift"]);
    expect(groups.map((group) => group.sessions.map((item) => item.id))).toEqual([["a"], ["b"]]);
  });

  it("空间行缺失（spaceName 为 null）时整组回退 cwd 文件夹名", () => {
    const groups = groupSessionsBySpace([
      session({ id: "a", cwd: "D:/repos/workbuddy-shift", spaceId: "space-1", spaceName: null }),
      session({ id: "b", cwd: DATE_DIR, spaceId: "space-1", spaceName: "workbuddy-shift" }),
    ]);

    // 同组的第二条有真名：整组仍用真名，不因为首条缺名字就降级成目录名。
    expect(groups).toHaveLength(1);
    expect(groups[0].label).toBe("workbuddy-shift");

    const onlyNull = groupSessionsBySpace([
      session({ id: "c", cwd: "D:/repos/workbuddy-shift", spaceId: "space-9", spaceName: null }),
    ]);
    expect(onlyNull[0].label).toBe("workbuddy-shift");
  });

  it("空间顺序沿用入参（最近活动在前）", () => {
    const groups = groupSessionsBySpace([
      session({ id: "new", spaceId: "space-b", spaceName: "B" }),
      session({ id: "old", spaceId: "space-a", spaceName: "A" }),
      session({ id: "newer", spaceId: "space-b", spaceName: "B" }),
    ]);

    expect(groups.map((group) => group.label)).toEqual(["B", "A"]);
    expect(groups[0].sessions.map((item) => item.id)).toEqual(["new", "newer"]);
  });
});

describe("buildSessionTree", () => {
  it("任务 / 空间只按 isPlayground 分类，spaceId 不参与判定", () => {
    const tree = buildSessionTree([
      // playground 会话也带 spaceId：仍然算「任务」，不能因为空间字段变成空间会话。
      session({ id: "t1", isPlayground: true, spaceId: "space-1", spaceName: "workbuddy-shift", cwd: "/x/t" }),
      session({ id: "s1", spaceId: "space-1", spaceName: "workbuddy-shift", cwd: "/x/a" }),
      session({ id: "s2", cwd: "/x/b" }),
    ]);

    expect(tree.map((kind) => [kind.key, kind.label, kind.count])).toEqual([
      ["task", "任务", 1],
      ["space", "空间", 2],
    ]);
    expect(tree[0].sessions.map((item) => item.id)).toEqual(["t1"]);
    expect(tree[1].spaces?.map((group) => group.label)).toEqual(["workbuddy-shift", "b"]);
  });

  it("空间计数从 cwd 数变成真实空间数（实测 3 → 2）", () => {
    const sessions: Session[] = [
      session({ id: "r1", cwd: "D:/repos/ResumeForge", spaceId: "space-r", spaceName: "ResumeForge" }),
      session({ id: "w1", cwd: "D:/repos/workbuddy-shift", spaceId: "space-w", spaceName: "workbuddy-shift" }),
      session({ id: "w2", cwd: DATE_DIR, spaceId: "space-w", spaceName: "workbuddy-shift" }),
    ];

    // 改造前：按 cwd 分组 = 3 个空间（其中一个叫日期时间戳）。
    const before = new Set(sessions.map((item) => item.cwd.trim())).size;
    const tree = buildSessionTree(sessions);
    expect(before).toBe(3);
    expect(tree).toHaveLength(1);
    expect(tree[0].count).toBe(2);
    expect(tree[0].spaces?.map((group) => [group.label, group.sessions.length])).toEqual([
      ["ResumeForge", 1],
      ["workbuddy-shift", 2],
    ]);
  });

  it("没有会话时不产生空分组", () => {
    expect(buildSessionTree([])).toEqual([]);
  });
});

describe("isWorkBuddyTimestampDir", () => {
  it("只认数据根下一层的纯时间戳目录", () => {
    expect(isWorkBuddyTimestampDir(LEGACY_WORKSPACE_DIR)).toBe(true);
    expect(isWorkBuddyTimestampDir("D:\\WorkBuddy_data\\2026-11-15-01-27-23")).toBe(true);
    expect(isWorkBuddyTimestampDir("/Users/me/WorkBuddy AI/2026-09-18-01-33-29")).toBe(true);
    // 真实空间目录、用户项目、层级不对的目录都不算。
    expect(isWorkBuddyTimestampDir(DATE_DIR)).toBe(false);
    expect(isWorkBuddyTimestampDir("D:/projects/2026-11-15-01-27-23")).toBe(false);
    expect(isWorkBuddyTimestampDir("D:/WorkBuddy_data/spaces/2026-11-15-01-27-23")).toBe(false);
    expect(isWorkBuddyTimestampDir("2026-11-15-01-27-23")).toBe(false);
    expect(isWorkBuddyTimestampDir("D:/WorkBuddy_data/2026-11-15")).toBe(false);
    expect(isWorkBuddyTimestampDir("")).toBe(false);
  });
});

describe("sessionCwdTitle", () => {
  it("工作目录放进悬停文案，cwd 为空时只显示标题", () => {
    expect(sessionCwdTitle(session({ id: "a", title: "改查询", cwd: "/ws/a" }))).toBe("改查询\n/ws/a");
    expect(sessionCwdTitle(session({ id: "b", title: "改查询", cwd: "   " }))).toBe("改查询");
  });
});
