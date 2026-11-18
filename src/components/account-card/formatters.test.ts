import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { formatRateLimitClock, formatRateLimitRemaining, formatTravelRemaining, travelTooltip } from "./formatters";

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
