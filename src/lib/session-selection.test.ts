import { describe, expect, it } from "vitest";

import { toggleGroupSelection, toggleSessionSelection, toggleSetKey } from "./session-selection";

/**
 * 会话勾选的纯逻辑：四个弹窗（WorkBuddy / VS Code / CodeBuddy IDE / 新增关联会话）
 * 共用同一份实现，这里锁定单选切换、组头三态勾选与折叠开关的语义与不可变性。
 */

describe("toggleSessionSelection", () => {
  it("未选中时加入，已选中时移除", () => {
    const empty = new Set<string>();
    const added = toggleSessionSelection(empty, "a");
    expect([...added]).toEqual(["a"]);
    expect([...toggleSessionSelection(added, "a")]).toEqual([]);
  });

  it("只影响目标 id，且不修改入参", () => {
    const selected = new Set(["a", "b"]);
    const next = toggleSessionSelection(selected, "b");
    expect([...selected]).toEqual(["a", "b"]);
    expect([...next]).toEqual(["a"]);
    // 原集合保持顺序，新集合按原顺序保留剩余项
    expect(toggleSessionSelection(selected, "c")).toEqual(new Set(["a", "b", "c"]));
  });
});

describe("toggleGroupSelection", () => {
  it("整组未全选时补全整组", () => {
    const selected = new Set(["a"]);
    expect(toggleGroupSelection(selected, ["b", "c"])).toEqual(new Set(["a", "b", "c"]));
    expect(toggleGroupSelection(selected, ["a", "b"])).toEqual(new Set(["a", "b"]));
  });

  it("整组已全选时取消整组", () => {
    const selected = new Set(["a", "b", "c"]);
    expect(toggleGroupSelection(selected, ["a", "b"])).toEqual(new Set(["c"]));
  });

  it("空组是 no-op，且不修改入参", () => {
    const selected = new Set(["a"]);
    expect(toggleGroupSelection(selected, [])).toEqual(new Set(["a"]));
    expect([...selected]).toEqual(["a"]);
  });
});

describe("toggleSetKey", () => {
  it("按 key 切换折叠状态", () => {
    const collapsed = new Set<string>();
    const next = toggleSetKey(collapsed, "workspace-1");
    expect([...next]).toEqual(["workspace-1"]);
    expect([...toggleSetKey(next, "workspace-1")]).toEqual([]);
    expect([...collapsed]).toEqual([]);
  });

  it("只切换目标 key，且不修改入参", () => {
    const keys = new Set(["a", "b"]);
    const added = toggleSetKey(keys, "c");
    expect([...added]).toEqual(["a", "b", "c"]);
    expect([...keys]).toEqual(["a", "b"]);
  });
});
