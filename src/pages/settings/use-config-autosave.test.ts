import { describe, expect, it } from "vitest";

import { enqueueSaveCommit, mergeSaveCommit, takeSaveCommit } from "./use-config-autosave";
import type { SaveCommit } from "./helpers";

interface DemoConfig {
  enabled: boolean;
  interval: number;
  cooldown: number;
}

describe("use-config-autosave 队列语义", () => {
  it("空队列入队后得到一份待提交快照", () => {
    const state = enqueueSaveCommit<DemoConfig>({ pending: null }, { interval: 5 }, { title: "间隔已保存" });
    expect(state.pending).toEqual({ edits: { interval: 5 }, success: { title: "间隔已保存" } });
  });

  it("连续编辑合并为一份最新快照，字段后写覆盖先写", () => {
    let state = enqueueSaveCommit<DemoConfig>({ pending: null }, { interval: 5 }, { title: "间隔已保存" });
    state = enqueueSaveCommit<DemoConfig>(state, { cooldown: 9 }, { title: "冷却已保存" });
    expect(state.pending).toEqual({
      edits: { interval: 5, cooldown: 9 },
      success: { title: "冷却已保存" },
    });
  });

  it("同一字段的重复编辑只保留最后一次值", () => {
    let state = enqueueSaveCommit<DemoConfig>({ pending: null }, { interval: 5 });
    state = enqueueSaveCommit<DemoConfig>(state, { interval: 15 });
    expect(state.pending?.edits).toEqual({ interval: 15 });
  });

  it("最新一次没带成功提示时沿用上一份提示", () => {
    let state = enqueueSaveCommit<DemoConfig>({ pending: null }, { interval: 5 }, { title: "间隔已保存" });
    state = enqueueSaveCommit<DemoConfig>(state, { cooldown: 3 });
    expect(state.pending?.success).toEqual({ title: "间隔已保存" });

    state = enqueueSaveCommit<DemoConfig>(state, { interval: 6 }, { title: "再次保存" });
    expect(state.pending?.success).toEqual({ title: "再次保存" });
  });

  it("出队返回当前快照并清空队列，便于提交期间继续入队", () => {
    const queued = enqueueSaveCommit<DemoConfig>({ pending: null }, { interval: 5 });
    const taken = takeSaveCommit(queued);
    expect(taken.commit).toEqual({ edits: { interval: 5 }, success: undefined });
    expect(taken.state.pending).toBeNull();
  });

  it("空队列出队得到 null，不产生空提交", () => {
    const taken = takeSaveCommit<DemoConfig>({ pending: null });
    expect(taken.commit).toBeNull();
    expect(taken.state.pending).toBeNull();
  });

  it("合并函数不修改入参队列（纯函数）", () => {
    const pending: SaveCommit<DemoConfig> = { edits: { interval: 5 }, success: { title: "间隔已保存" } };
    mergeSaveCommit<DemoConfig>(pending, { cooldown: 1 });
    expect(pending).toEqual({ edits: { interval: 5 }, success: { title: "间隔已保存" } });
  });
});