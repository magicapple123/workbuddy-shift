import { describe, expect, it } from "vitest";

import {
  COMPACT_STATUS_SLOT_COUNT,
  hiddenChipsAriaLabel,
  selectCompactStatusChips,
  STATUS_PRIORITY,
  type StatusChipKey,
} from "./status-selection";

/** 构造一个状态描述（priority 默认取固定表）。 */
function chip(key: StatusChipKey, extra: { pinned?: boolean; priority?: number } = {}) {
  return {
    key,
    priority: extra.priority ?? STATUS_PRIORITY[key],
    detail: key,
    node: null,
    pinned: extra.pinned,
  };
}

/** 汇总成 `key(+pinned)` 形式，便于断言顺序。 */
function keys(chips: readonly { key: StatusChipKey; pinned?: boolean }[]): string[] {
  return chips.map((item) => (item.pinned ? `${item.key}*` : item.key));
}

describe("selectCompactStatusChips", () => {
  it("槽数上限固定为 3（清单决策）", () => {
    expect(COMPACT_STATUS_SLOT_COUNT).toBe(3);
  });

  it("不超过 3 个时全部渲染，不出现 +N", () => {
    const chips = [chip("checkinDone"), chip("travel")];
    const { visible, hidden } = selectCompactStatusChips(chips);
    expect(keys(visible)).toEqual(["checkinDone", "travel"]);
    expect(hidden).toEqual([]);
  });

  it("没有标记时按优先级取前 3 个系统状态", () => {
    // 视觉顺序：企业版 → 自动签到关闭 → 今日已签到 → 旅行 → 限额
    const chips = [
      chip("enterprise"),
      chip("autoCheckinOff"),
      chip("checkinDone"),
      chip("travel"),
      chip("rateLimit"),
    ];
    const { visible, hidden } = selectCompactStatusChips(chips);
    // 选中项仍按视觉顺序渲染（企业版在前的语义不变），只是「选谁」由优先级决定。
    expect(keys(visible)).toEqual(["autoCheckinOff", "checkinDone", "rateLimit"]);
    expect(keys(hidden)).toEqual(["enterprise", "travel"]);
  });

  it("有标记时标记优先保留，且永远不进 +N", () => {
    const chips = [
      chip("enterprise"),
      chip("checkinPending"),
      chip("travel"),
      chip("rateLimit"),
      chip("mark", { pinned: true }),
    ];
    const { visible, hidden } = selectCompactStatusChips(chips);
    // 标记被钉住，必进 visible；其余按优先级补齐 2 个，整体仍按视觉顺序排列。
    expect(keys(visible)).toEqual(["checkinPending", "rateLimit", "mark*"]);
    expect(keys(hidden)).toEqual(["enterprise", "travel"]);
    expect(keys(hidden)).not.toContain("mark*");
  });

  it("钉住项优先于更高优先级的普通项（即便槽位紧张）", () => {
    const chips = [
      chip("relogin"),
      chip("rateLimit"),
      chip("checkinPending"),
      chip("mark", { pinned: true }),
    ];
    const { visible, hidden } = selectCompactStatusChips(chips);
    expect(keys(visible)).toEqual(["relogin", "rateLimit", "mark*"]);
    expect(keys(hidden)).toEqual(["checkinPending"]);
  });

  it("只有属性类 chip 时不会溢出", () => {
    const chips = [chip("enterprise"), chip("multiTool")];
    const { visible, hidden } = selectCompactStatusChips(chips);
    expect(keys(visible)).toEqual(["enterprise", "multiTool"]);
    expect(hidden).toEqual([]);
  });

  it("同级按视觉顺序稳定取舍", () => {
    const chips = [
      chip("travel", { priority: 5 }),
      chip("enterprise", { priority: 5 }),
      chip("multiTool", { priority: 5 }),
      chip("checkinDone", { priority: 5 }),
    ];
    const { visible } = selectCompactStatusChips(chips);
    expect(keys(visible)).toEqual(["travel", "enterprise", "multiTool"]);
  });

  it("空列表与 cap=0 的边界", () => {
    expect(selectCompactStatusChips([])).toEqual({ visible: [], hidden: [] });
    const chips = [chip("checkinDone")];
    expect(keys(selectCompactStatusChips(chips, 0).hidden)).toEqual(["checkinDone"]);
  });
});

describe("hiddenChipsAriaLabel", () => {
  it("列出被收起的全部 detail", () => {
    expect(
      hiddenChipsAriaLabel([{ detail: "今日已签到" }, { detail: "正在旅行" }]),
    ).toBe("还有 2 个状态：今日已签到、正在旅行");
  });

  it("没有收起项时返回空串", () => {
    expect(hiddenChipsAriaLabel([])).toBe("");
  });
});
