import { describe, expect, it } from "vitest";

import type { CreditOfficialUsage, CreditStatistics } from "@/lib/types";

import { buildCreditStatsCsv } from "./export";
import { buildCreditStatsXlsx, creditStatsSheets, creditStatsXlsxFileName } from "./export-xlsx";

const stats: CreditStatistics = {
  generatedAt: Date.UTC(2026, 10, 19, 10, 0),
  retentionDays: 30,
  coverageStartAt: Date.UTC(2026, 10, 10),
  coverageEndAt: Date.UTC(2026, 10, 19),
  summary: {
    currentRemaining: 120,
    currentCapacity: 500,
    usageToday: 3,
    usage7Days: 9,
    usageThisMonth: 21,
    todayCheckedInAccounts: 2,
    todaySuccess: 1,
    todayAlready: 1,
    todayFailed: 0,
  },
  daily: [{ date: "2026-11-19", usage: 3 }],
  accounts: [
    {
      accountId: "acc-1",
      accountName: "林小凤",
      isCurrent: true,
      currentRemaining: 120,
      totalCapacity: 500,
      usageToday: 3,
      usage7Days: 9,
      usageThisMonth: 21,
      variant: "cn",
      lastSnapshotAt: Date.UTC(2026, 10, 19),
      checkedInToday: true,
      checkinStatusToday: "success",
      lastCheckinAt: Date.UTC(2026, 10, 19),
      lastCheckinResult: "success",
    },
  ],
  events: [],
};

const official: CreditOfficialUsage = {
  status: "complete",
  rangeStart: "2026-10-20",
  rangeEnd: "2026-11-19",
  summary: { usageToday: 4, usage7Days: 10, usageThisMonth: 22 },
  daily: [{ date: "2026-11-19", usage: 4 }],
  accounts: [
    {
      accountId: "acc-1",
      accountName: "林小凤",
      ok: true,
      requestCount: 2,
      detailTruncated: false,
      usageToday: 4,
      usage7Days: 10,
      usageThisMonth: 22,
    },
  ],
  requests: [
    {
      accountId: "acc-1",
      accountName: "林小凤",
      requestId: "r1",
      credit: 2.5,
      model: "deepseek-v4-flash",
      client: "VSCode",
      requestTime: "2026-11-19 10:00:00",
    },
    {
      accountId: "acc-1",
      accountName: "林小凤",
      requestId: "r2",
      credit: 1.5,
      model: "deepseek-v4-flash",
      client: "JetBrainsPyCharm",
      requestTime: "2026-11-19 11:00:00",
    },
  ],
  models: [{ model: "deepseek-v4-flash", requestCount: 2, credit: 4 }],
  detailLimitPerAccount: 1000,
  errors: [],
};

describe("creditStatsSheets", () => {
  it("7 个 sheet，名称与区块一致，最后一行是说明", () => {
    const sheets = creditStatsSheets(stats, official, { range: "30d" });
    expect(sheets.map((sheet) => sheet.name)).toEqual([
      "总览",
      "按账号",
      "按日",
      "按模型",
      "按客户端",
      "官方请求明细",
      "本地事件",
    ]);
    for (const sheet of sheets) {
      const last = sheet.rows[sheet.rows.length - 1]?.[0];
      expect(String(last)).toMatch(/^说明：/);
    }
    // 表头仍是第 1 行：说明不污染数据区。
    expect(sheets[0].header).toEqual(["指标", "数值"]);
  });

  it("数值保持 number 类型（Excel 可求和）", () => {
    const sheets = creditStatsSheets(stats, official, { range: "30d" });
    const overview = sheets[0];
    const today = overview.rows.find((row) => row[0] === "今日消耗");
    expect(typeof today?.[1]).toBe("number");
    const clients = sheets[4];
    expect(clients.rows.map((row) => row[0])).toEqual(["VSCode", "JetBrainsPyCharm", "说明：客户端分组来自官方逐请求明细；范围：近 30 天（官方窗口）"]);
  });

  it("全部范围用账本客户端分组，并在说明里写明口径", () => {
    const ledger = {
      status: "ok" as const,
      rows: 12,
      total: 30,
      startAt: "2026-10-20",
      endAt: "2026-11-19",
      summary: { usageToday: 3, usage7Days: 9, usageThisMonth: 21 },
      daily: [{ date: "2026-11-19", usage: 3 }],
      models: [{ model: "deepseek-v4-flash", requestCount: 12, credit: 30 }],
      clients: [{ client: "JetBrainsPyCharm", requestCount: 4, credit: 10 }],
      accounts: [{ accountId: "acc-1", accountName: "林小凤", requestCount: 12, credit: 30 }],
    };
    const sheets = creditStatsSheets(stats, undefined, { range: "all", ledger });
    expect(sheets[4].rows[0]).toEqual(["JetBrainsPyCharm", 4, 10]);
    expect(String(sheets[4].rows[sheets[4].rows.length - 1]?.[0])).toContain("本地账本");
  });

  it("xlsx 产物非空，文件名后缀正确", () => {
    const bytes = buildCreditStatsXlsx(stats, official);
    expect(bytes.length).toBeGreaterThan(1000);
    expect(creditStatsXlsxFileName("cn")).toMatch(/\.xlsx$/);
  });
});

describe("CSV 与 Excel 字段一致", () => {
  it("CSV 仍包含 6 个区块且与 Excel 同名", () => {
    const csv = buildCreditStatsCsv(stats, official);
    for (const title of ["积分统计 · 总览", "按账号", "按日", "按模型", "官方请求明细", "本地事件"]) {
      expect(csv).toContain(title);
    }
  });
});
