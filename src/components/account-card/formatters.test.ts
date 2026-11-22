import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import {
  activeMark,
  formatLastUsedAgo,
  formatMarkClock,
  formatMarkExpiryLine,
  formatMarkRemaining,
  formatRateLimitClock,
  formatRateLimitRemaining,
  formatTravelRemaining,
  isMarkExpired,
  lastUsedAriaLabel,
  lastUsedClientLabel,
  markAriaLabel,
  travelTooltip,
  truncateMarkText,
} from "./formatters";

describe("限额倒计时格式化", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date(2026, 8, 17, 12, 0, 0));
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it("按剩余时长选择小时/分钟文案", () => {
    const now = Date.now();
    expect(formatRateLimitRemaining(now + 30 * 60_000, now)).toBe("30m 后恢复");
    expect(formatRateLimitRemaining(now + 90 * 60_000, now)).toBe("1h30m 后恢复");
    expect(formatRateLimitRemaining(now + 2 * 3_600_000, now)).toBe("2h 后恢复");
    expect(formatRateLimitRemaining(now - 1, now)).toBe("即将恢复");
  });

  it("恢复时刻落在今天/明天/更晚时分别给出时钟、明天、日期", () => {
    const now = Date.now();
    expect(formatRateLimitClock(new Date(2026, 8, 17, 18, 30).getTime(), now)).toBe("18:30");
    expect(formatRateLimitClock(new Date(2026, 8, 18, 9, 5).getTime(), now)).toBe("明天 09:05");
    expect(formatRateLimitClock(new Date(2026, 8, 20, 9, 5).getTime(), now)).toBe("9/20 09:05");
  });
});

describe("旅行状态格式化", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date(2026, 8, 17, 12, 0, 0));
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it("兼容秒/毫秒两种到达时刻，并给出剩余时长", () => {
    const now = Date.now();
    expect(formatTravelRemaining(now + 90 * 60_000)).toBe("剩余 1 小时 30 分钟");
    expect(formatTravelRemaining(Math.floor((now + 60_000) / 1000))).toBe("剩余 1 分钟");
    expect(formatTravelRemaining(now - 1)).toBe("即将到达");
    expect(formatTravelRemaining(null)).toBeNull();
  });

  it("旅行中显示地点与预计奖励", () => {
    expect(
      travelTooltip({ label: "traveling", locationName: "巴黎", rewardCredit: 100, arriveAt: null }),
    ).toBe("巴黎 · 预计 +100");
  });

  it("结束/无 Buddy/未旅行分别给出可读文案", () => {
    expect(
      travelTooltip({ label: "finished", locationName: "巴黎", rewardCredit: 100, arriveAt: null }),
    ).toBe("巴黎 · +100");
    expect(travelTooltip({ label: "no-buddy", rewardCredit: null, arriveAt: null })).toBe("无 Buddy");
    expect(travelTooltip({ label: "untraveled", rewardCredit: null, arriveAt: null })).toBe("未旅行");
  });
});

/**
 * R5-04 账号标记的展示文案：剩余时长、到期行、读屏文案、过期判定与菜单截断。
 */
describe("账号标记格式化", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date(2026, 10, 20, 20, 50, 0));
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  const now = new Date(2026, 10, 20, 20, 50, 0).getTime();
  const mark = (expiresAt: number | null, createdAt = now) => ({
    text: "即将限额",
    color: "warning" as const,
    createdAt,
    expiresAt,
  });

  it("剩余时长按分钟向上取整，超过一天用「剩 N 天」", () => {
    expect(formatMarkRemaining(now + 32 * 60_000, now)).toBe("剩 32 分钟");
    expect(formatMarkRemaining(now + 2 * 3_600_000, now)).toBe("剩 2 小时");
    expect(formatMarkRemaining(now + 90 * 60_000, now)).toBe("剩 1 小时 30 分钟");
    expect(formatMarkRemaining(now + 3 * 86_400_000, now)).toBe("剩 3 天");
    // 剩余 30 秒也显示「剩 1 分钟」，不出现「剩 0 分钟」。
    expect(formatMarkRemaining(now + 30_000, now)).toBe("剩 1 分钟");
  });

  it("过期与手动解除的取值", () => {
    expect(formatMarkRemaining(now - 1, now)).toBe("已过期");
    expect(formatMarkRemaining(null, now)).toBeNull();
  });

  it("到期判定：null 永不过期，到期时刻当刻即算过期", () => {
    expect(isMarkExpired(mark(null), now)).toBe(false);
    expect(isMarkExpired(mark(now + 1), now)).toBe(false);
    expect(isMarkExpired(mark(now), now)).toBe(true);
    expect(isMarkExpired(mark(now - 1), now)).toBe(true);
  });

  it("卡片展示用 activeMark：过期后返回 null（图标隐藏，但记录仍在校验范围内）", () => {
    expect(activeMark({ mark: mark(null) }, now)?.text).toBe("即将限额");
    expect(activeMark({ mark: mark(now + 60_000) }, now)?.text).toBe("即将限额");
    expect(activeMark({ mark: mark(now - 60_000) }, now)).toBeNull();
    // 旧账号库没有 mark 字段 → 等价于「无标记」。
    expect(activeMark({}, now)).toBeNull();
    expect(activeMark({ mark: null }, now)).toBeNull();
  });

  it("绝对时刻与 tooltip 的到期行", () => {
    expect(formatMarkClock(now)).toBe("2026/11/20 20:50");
    expect(formatMarkExpiryLine(mark(now + 2 * 3_600_000), now)).toBe(
      "至 2026/11/20 22:50 · 剩 2 小时",
    );
    expect(formatMarkExpiryLine(mark(null), now)).toBe("手动解除");
    expect(formatMarkExpiryLine(mark(now - 60_000), now)).toBe("至 2026/11/20 20:49 · 已过期");
  });

  it("读屏文案包含文本与剩余时间，过期后改为「已过期」", () => {
    expect(markAriaLabel(mark(now + 2 * 3_600_000), now)).toBe("标记：即将限额，剩 2 小时，点击编辑");
    expect(markAriaLabel(mark(null), now)).toBe("标记：即将限额，手动解除，点击编辑");
    expect(markAriaLabel(mark(now - 1), now)).toBe("标记：即将限额，已过期，点击编辑");
  });

  it("菜单文案截断到 8 个字", () => {
    expect(truncateMarkText("即将限额")).toBe("即将限额");
    expect(truncateMarkText("一二三四五六七八")).toBe("一二三四五六七八");
    expect(truncateMarkText("一二三四五六七八九")).toBe("一二三四五六七八…");
  });
});

/**
 * 「最近切换」（issue #184）：相对时间、来源端文案与读屏文案。
 *
 * 关键约定是**从未记录 ≠ 显示「从未」**：`formatLastUsedAgo` 返回 `null`，
 * 调用方（`status-chips.tsx`）据此整块不渲染。
 */
describe("最近切换格式化", () => {
  const now = 1_800_000_000_000;

  it("按分钟/小时/天给相对时间，超一个月回落绝对时刻", () => {
    expect(formatLastUsedAgo(now - 10_000, now)).toBe("刚刚");
    expect(formatLastUsedAgo(now - 12 * 60_000, now)).toBe("12 分钟前");
    expect(formatLastUsedAgo(now - 3 * 3_600_000, now)).toBe("3 小时前");
    expect(formatLastUsedAgo(now - 2 * 86_400_000, now)).toBe("2 天前");
    expect(formatLastUsedAgo(now - 90 * 86_400_000, now)).toBe(formatMarkClock(now - 90 * 86_400_000));
  });

  it("缺失 / 0 / 非有限数一律返回 null（= 从未记录，不显示徽标）", () => {
    expect(formatLastUsedAgo(undefined, now)).toBeNull();
    expect(formatLastUsedAgo(null, now)).toBeNull();
    expect(formatLastUsedAgo(0, now)).toBeNull();
    expect(formatLastUsedAgo(Number.NaN, now)).toBeNull();
  });

  it("时钟回拨（时间戳比 now 还新）按「刚刚」处理，不出现负数", () => {
    expect(formatLastUsedAgo(now + 5 * 60_000, now)).toBe("刚刚");
  });

  it("来源端标签：已知取值给中文，未知取值降级为「其它客户端」，空值返回 null", () => {
    expect(lastUsedClientLabel("vscode")).toBe("VS Code 插件");
    expect(lastUsedClientLabel("rotate")).toBe("自动轮换");
    expect(lastUsedClientLabel("atom-plugin")).toBe("其它客户端");
    expect(lastUsedClientLabel(null)).toBeNull();
    expect(lastUsedClientLabel("  ")).toBeNull();
  });

  it("读屏文案带上来源端；没有来源端时只说时间", () => {
    const at = now - 3 * 3_600_000;
    expect(lastUsedAriaLabel(at, now, "vscode")).toBe("最近切换：3 小时前，来自 VS Code 插件");
    expect(lastUsedAriaLabel(at, now, null)).toBe("最近切换：3 小时前");
  });
});
