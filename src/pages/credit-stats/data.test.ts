import { describe, expect, it } from "vitest";

import { dateDaysAgo, dateKey } from "@/lib/format";
import type {
  CreditOfficialUsage,
  CreditOfficialUsageRequest,
  CreditStatistics,
  CreditStatsAccount,
  CreditStatsEvent,
  WbVariant,
} from "@/lib/types";

import {
  compareRequests,
  rangeUsage,
  recomputeLocalStats,
  recomputeOfficialUsage,
} from "./data";

const today = dateKey(new Date());
const daysAgo = (days: number) => dateDaysAgo(days);

function account(overrides: Partial<CreditStatsAccount> & { accountId: string }): CreditStatsAccount {
  return {
    accountName: overrides.accountId,
    isCurrent: false,
    currentRemaining: null,
    totalCapacity: null,
    lastSnapshotAt: null,
    usageToday: 0,
    usage7Days: 0,
    usageThisMonth: 0,
    checkedInToday: null,
    checkinStatusToday: null,
    lastCheckinAt: null,
    lastCheckinResult: null,
    ...overrides,
  };
}

function checkinEvent(
  accountId: string,
  result: string,
  variant: WbVariant,
  date = today,
): CreditStatsEvent {
  return {
    kind: "checkin",
    ts: 1,
    date,
    accountId,
    accountName: accountId,
    result,
    variant,
  };
}

const summary = {
  currentRemaining: 0,
  currentCapacity: 0,
  usageToday: 0,
  usage7Days: 0,
  usageThisMonth: 0,
  todayCheckedInAccounts: 0,
  todaySuccess: 0,
  todayAlready: 0,
  todayFailed: 0,
};

describe("credit-stats recomputeLocalStats", () => {
  const stats: CreditStatistics = {
    generatedAt: 1,
    retentionDays: 30,
    coverageStartAt: null,
    summary,
    daily: [
      { date: daysAgo(1), usage: 0 },
      { date: today, usage: 0 },
    ],
    accounts: [
      account({
        accountId: "a",
        isCurrent: true,
        currentRemaining: 100,
        totalCapacity: 200,
        usageToday: 1,
        usage7Days: 2,
        usageThisMonth: 3,
        daily: [
          { date: daysAgo(1), usage: 5 },
          { date: today, usage: 4 },
        ],
      }),
      account({
        accountId: "b",
        isCurrent: true,
        currentRemaining: 50,
        totalCapacity: 60,
        usageToday: 9,
        usage7Days: 9,
        usageThisMonth: 9,
        daily: [{ date: today, usage: 9 }],
      }),
    ],
    events: [
      checkinEvent("a", "success", "cn"),
      checkinEvent("a", "already", "cn"),
      checkinEvent("a", "error", "cn"),
      checkinEvent("b", "success", "ai"),
    ],
  };
  const variantByAccountId = new Map<string, WbVariant>([
    ["a", "cn"],
    ["b", "ai"],
  ]);

  it("cn / ai 视图按档位隔离事件，账号按调用方传入的账号集合过滤", () => {
    const cn = recomputeLocalStats(stats, new Set(["a"]), "cn", variantByAccountId);
    expect(cn.accounts.map((item) => item.accountId)).toEqual(["a"]);
    expect(cn.events).toHaveLength(3);
    expect(cn.summary.usageToday).toBe(1);
    expect(cn.summary.usage7Days).toBe(2);
    expect(cn.summary.usageThisMonth).toBe(3);

    const ai = recomputeLocalStats(stats, new Set(["b"]), "ai", variantByAccountId);
    expect(ai.accounts.map((item) => item.accountId)).toEqual(["b"]);
    expect(ai.events).toHaveLength(1);
    expect(ai.summary.usageToday).toBe(9);
  });

  it("当前剩余 / 容量只统计 isCurrent 账号", () => {
    const mixed = recomputeLocalStats(
      {
        ...stats,
        accounts: [
          account({ accountId: "a", isCurrent: true, currentRemaining: 100, totalCapacity: 200 }),
          account({ accountId: "b", isCurrent: false, currentRemaining: 50, totalCapacity: 60 }),
        ],
      },
      new Set(["a", "b"]),
      "cn",
      variantByAccountId,
    );
    expect(mixed.summary.currentRemaining).toBe(100);
    expect(mixed.summary.currentCapacity).toBe(200);
  });

  it("逐日曲线沿用后端日期轴，过滤后的账号按日期累加、缺失日期补 0", () => {
    const cn = recomputeLocalStats(stats, new Set(["a"]), "cn", variantByAccountId);
    expect(cn.daily).toEqual([
      { date: daysAgo(1), usage: 5 },
      { date: today, usage: 4 },
    ]);
  });

  it("今日签到按 success / already / error 计数，已签到账号去重", () => {
    const cn = recomputeLocalStats(stats, new Set(["a"]), "cn", variantByAccountId);
    expect(cn.summary.todaySuccess).toBe(1);
    expect(cn.summary.todayAlready).toBe(1);
    expect(cn.summary.todayFailed).toBe(1);
    expect(cn.summary.todayCheckedInAccounts).toBe(1);
  });

  it("非今日的签到事件不计入今日统计", () => {
    const withOld = recomputeLocalStats(
      {
        ...stats,
        events: [checkinEvent("a", "success", "cn", daysAgo(1))],
      },
      new Set(["a"]),
      "cn",
      variantByAccountId,
    );
    expect(withOld.summary.todaySuccess).toBe(0);
    expect(withOld.summary.todayCheckedInAccounts).toBe(0);
  });
});

function officialRequest(accountId: string, requestId: string, credit: number): CreditOfficialUsageRequest {
  return {
    accountId,
    accountName: accountId,
    requestId,
    credit,
    model: "m",
    client: "demo",
    requestTime: "2026-01-01 10:00:00",
  };
}

describe("credit-stats recomputeOfficialUsage", () => {
  const official: CreditOfficialUsage = {
    status: "complete",
    rangeStart: daysAgo(29),
    rangeEnd: today,
    summary: { usageToday: 12, usage7Days: 20, usageThisMonth: 30 },
    daily: [
      { date: daysAgo(1), usage: 0, models: [] },
      { date: today, usage: 0, models: [] },
    ],
    accounts: [
      {
        accountId: "a",
        accountName: "A",
        ok: true,
        requestCount: 1,
        detailTruncated: false,
        usageToday: 10,
        usage7Days: 20,
        usageThisMonth: 30,
        models: [{ model: "m1", requestCount: 2, credit: 5 }],
        daily: [{ date: today, usage: 10, models: [{ model: "m1", requestCount: 2, credit: 5 }] }],
      },
      {
        accountId: "b",
        accountName: "B",
        ok: false,
        requestCount: 1,
        detailTruncated: false,
        usageToday: 2,
        usage7Days: null,
        usageThisMonth: null,
        models: [{ model: "m2", requestCount: 1, credit: 1 }],
        daily: [{ date: today, usage: 2, models: [{ model: "m2", requestCount: 1, credit: 1 }] }],
      },
    ],
    requests: [officialRequest("a", "r-a", 5), officialRequest("b", "r-b", 1)],
    detailLimitPerAccount: 100,
    errors: [{ accountId: "b", accountName: "B", error: "超时" }],
  };

  it("按账号集合过滤账号 / 请求 / 错误，并重算汇总", () => {
    const onlyA = recomputeOfficialUsage(official, new Set(["a"]));
    expect(onlyA.accounts.map((item) => item.accountId)).toEqual(["a"]);
    expect(onlyA.requests.map((item) => item.requestId)).toEqual(["r-a"]);
    expect(onlyA.errors).toEqual([]);
    expect(onlyA.status).toBe("complete");
    expect(onlyA.summary).toEqual({ usageToday: 10, usage7Days: 20, usageThisMonth: 30 });
  });

  it("部分账号失败时状态为 partial，空账号集合为 unavailable", () => {
    const both = recomputeOfficialUsage(official, new Set(["a", "b"]));
    expect(both.status).toBe("partial");
    expect(both.summary.usageToday).toBe(12);
    expect(recomputeOfficialUsage(official, new Set()).status).toBe("unavailable");
  });

  it("模型按 credit 降序合并，日期轴保留官方序列并按日聚合模型", () => {
    const both = recomputeOfficialUsage(official, new Set(["a", "b"]));
    expect(both.models?.map((model) => model.model)).toEqual(["m1", "m2"]);
    expect(both.daily.map((point) => point.date)).toEqual([daysAgo(1), today]);
    expect(both.daily[0].models).toEqual([]);
    expect(both.daily[1].usage).toBe(12);
    expect(both.daily[1].models?.map((model) => model.model)).toEqual(["m1", "m2"]);
  });
});

describe("credit-stats rangeUsage", () => {
  const usageSummary = {
    ...summary,
    usageToday: 1,
    usage7Days: 2,
    usageThisMonth: 3,
  };
  const daily = [
    { date: daysAgo(30), usage: 11 },
    { date: daysAgo(29), usage: 7 },
    { date: daysAgo(10), usage: 5 },
    { date: daysAgo(1), usage: 3 },
    { date: today, usage: 2 },
  ];

  it("today / 7d / month 直接取汇总字段", () => {
    expect(rangeUsage(usageSummary, daily, "today")).toBe(1);
    expect(rangeUsage(usageSummary, daily, "7d")).toBe(2);
    expect(rangeUsage(usageSummary, daily, "month")).toBe(3);
  });

  it("30d 从逐日数据求和，只统计最近 30 天", () => {
    expect(rangeUsage(usageSummary, daily, "30d")).toBe(7 + 5 + 3 + 2);
  });
});

describe("credit-stats compareRequests", () => {
  const older = officialRequest("a", "r1", 1);
  older.requestTime = "2026-01-01 10:00:00";
  const newer = officialRequest("a", "r2", 3);
  newer.requestTime = "2026-01-02 10:00:00";

  it("按消耗排序：desc 大在前，asc 小在前", () => {
    expect(compareRequests(newer, older, "credit", "desc")).toBeLessThan(0);
    expect(compareRequests(newer, older, "credit", "asc")).toBeGreaterThan(0);
  });

  it("按时间排序：请求时间是可按字典序比较的文本", () => {
    expect(compareRequests(newer, older, "time", "desc")).toBeLessThan(0);
    expect(compareRequests(older, newer, "time", "asc")).toBeLessThan(0);
  });

  it("同值时用请求 ID 兜底保证稳定排序", () => {
    const same = officialRequest("a", "r9", 3);
    same.requestTime = newer.requestTime;
    expect(compareRequests(newer, same, "credit", "asc")).toBeLessThan(0);
  });
});
