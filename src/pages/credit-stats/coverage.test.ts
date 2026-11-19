import { describe, expect, it } from "vitest";

import type {
  CreditLocalCoverage,
  CreditOfficialUsage,
  CreditStatistics,
} from "@/lib/types";

import {
  creditSourceBadge,
  localCoverageText,
  localWindowCoverage,
  usageWindowValue,
} from "./coverage";

/** 一份只有今天快照的本地统计（issue #225 现场：今日有值、7 天/本月曾被今日值冒充）。 */
function localStats(overrides: Partial<CreditStatistics> = {}): CreditStatistics {
  return {
    generatedAt: 1_700_000_000_000,
    retentionDays: 90,
    coverageStartAt: 1_699_999_000_000,
    coverageEndAt: 1_699_999_000_000,
    coverage: {
      startAt: 1_699_999_000_000,
      endAt: 1_699_999_000_000,
      days: 1,
      covers7Days: false,
      coversThisMonth: false,
    },
    summary: {
      currentRemaining: 2678.06,
      currentCapacity: 3000,
      usageToday: 28.7,
      usage7Days: 28.7,
      usageThisMonth: 28.7,
      todayCheckedInAccounts: 0,
      todaySuccess: 0,
      todayAlready: 0,
      todayFailed: 0,
    },
    daily: [],
    accounts: [],
    events: [],
    ...overrides,
  };
}

const coverage10Days: CreditLocalCoverage = {
  startAt: 1_699_000_000_000,
  endAt: 1_699_999_000_000,
  days: 10,
  covers7Days: true,
  coversThisMonth: true,
};

const officialUsage: CreditOfficialUsage = {
  status: "complete",
  rangeStart: "2026-10-21",
  rangeEnd: "2026-11-19",
  summary: { usageToday: 101.16, usage7Days: 129.86, usageThisMonth: 129.86 },
  daily: [],
  accounts: [],
  requests: [],
  detailLimitPerAccount: 1000,
  errors: [],
};

const failedOfficial: CreditOfficialUsage = {
  ...officialUsage,
  status: "unavailable",
  summary: { usageToday: 0, usage7Days: 0, usageThisMonth: 0 },
  errors: [{ accountId: "a1", accountName: "one@example.com", error: "timeout" }],
};

describe("积分口径判定", () => {
  it("官方不可用时回退本地观察，并标出数据来源", () => {
    const stats = localStats({ coverage: coverage10Days, coverageStartAt: coverage10Days.startAt });
    expect(creditSourceBadge(undefined)).toBe("本地观察");
    expect(creditSourceBadge(failedOfficial)).toBe("本地观察");
    expect(creditSourceBadge(officialUsage)).toBe("官方用量");

    const today = usageWindowValue("today", undefined, stats, coverage10Days);
    expect(today.text).toBe("28.7");
    expect(today.absenceReason).toBeNull();
  });

  it("本地只覆盖 1 天时 7 天/本月显示「—」，不退化成今日值", () => {
    const stats = localStats();
    const coverage = stats.coverage;
    const today = usageWindowValue("today", undefined, stats, coverage);
    const week = usageWindowValue("7d", undefined, stats, coverage);
    const month = usageWindowValue("month", undefined, stats, coverage);

    expect(today.text).toBe("28.7");
    expect(week.text).toBe("—");
    expect(month.text).toBe("—");
    expect(week.absenceReason).toContain("仅覆盖 1 天");
    // 关键回归点：三个数字不再相同。
    expect(new Set([today.text, week.text, month.text]).size).toBeGreaterThan(1);
  });

  it("本地覆盖满 10 天时 7 天/本月恢复为真实观察值", () => {
    const stats = localStats({ coverage: coverage10Days });
    expect(usageWindowValue("7d", undefined, stats, coverage10Days).text).toBe("28.7");
    expect(usageWindowValue("month", undefined, stats, coverage10Days).absenceReason).toBeNull();
  });

  it("官方可用时优先官方数值，partial 账号缺失值显示「—」", () => {
    const stats = localStats();
    expect(usageWindowValue("today", officialUsage, stats, undefined).text).toBe("101.16");
    expect(usageWindowValue("7d", officialUsage, stats, undefined).text).toBe("129.86");
    expect(usageWindowValue("month", officialUsage, stats, undefined).text).toBe("129.86");

    const partial: CreditOfficialUsage = {
      ...officialUsage,
      status: "partial",
      summary: { usageToday: 0, usage7Days: 0, usageThisMonth: 0 },
    };
    // partial 的总览是「已同步账号之和」，缺失账号在账号明细里显示「—」，这里不改写总额。
    const missing = usageWindowValue("today", partial, stats, undefined);
    expect(missing.text).toBe("0");
    expect(missing.absenceReason).toBeNull();
  });

  it("覆盖文案与旧后端回退", () => {
    expect(localCoverageText(undefined)).toBe("本地暂无快照");
    expect(localCoverageText({ startAt: null, endAt: null, days: 0, covers7Days: false, coversThisMonth: false })).toBe(
      "本地暂无快照",
    );
    expect(localCoverageText(localStats().coverage)).toContain("覆盖 1 天");
    // 旧后端没有 coverage 字段：按「有快照即覆盖」保持改造前行为。
    expect(localWindowCoverage(undefined, true)).toEqual({ week: true, month: true });
    expect(localWindowCoverage(undefined, false)).toEqual({ week: false, month: false });
    expect(localWindowCoverage(localStats().coverage, true)).toEqual({ week: false, month: false });
  });
});
