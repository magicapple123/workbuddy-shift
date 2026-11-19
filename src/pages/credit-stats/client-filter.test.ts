import { describe, expect, it } from "vitest";

import { dateDaysAgo, dateKey } from "@/lib/format";
import type { CreditOfficialUsage, CreditOfficialUsageRequest } from "@/lib/types";

import { clientKeyOf, clientViewOptions, filterOfficialByClient } from "./data";

function request(
  id: string,
  client: string,
  credit: number,
  date: string,
  accountId = "acc-1",
): CreditOfficialUsageRequest {
  return {
    accountId,
    accountName: accountId,
    requestId: id,
    credit,
    model: "deepseek-v4-flash",
    client,
    requestTime: `${date} 10:00:00`,
  };
}

function official(requests: CreditOfficialUsageRequest[]): CreditOfficialUsage {
  const today = dateKey(new Date());
  return {
    status: "complete",
    rangeStart: dateDaysAgo(29),
    rangeEnd: today,
    summary: { usageToday: 999, usage7Days: 999, usageThisMonth: 999 },
    daily: [{ date: today, usage: 999, models: [] }],
    accounts: [
      {
        accountId: "acc-1",
        accountName: "acc-1",
        ok: true,
        requestCount: requests.length,
        detailTruncated: false,
        usageToday: 999,
        usage7Days: 999,
        usageThisMonth: 999,
      },
    ],
    requests,
    detailLimitPerAccount: 1000,
    errors: [],
  };
}

describe("clientKeyOf", () => {
  it("按官方 client 原文归一，JetBrains 用前缀匹配", () => {
    expect(clientKeyOf("WorkBuddy")).toBe("workbuddy");
    expect(clientKeyOf("VSCode")).toBe("vscode");
    expect(clientKeyOf("VS Code")).toBe("vscode");
    expect(clientKeyOf("JetBrainsPyCharm")).toBe("jetbrains");
    expect(clientKeyOf("JetBrainsIntelliJ")).toBe("jetbrains");
    expect(clientKeyOf("CodeBuddyIDE")).toBe("ide");
    expect(clientKeyOf("CodeBuddy IDE")).toBe("ide");
    expect(clientKeyOf("CodeBuddyCLI")).toBe("cli");
    expect(clientKeyOf("—")).toBe("unknown");
    expect(clientKeyOf("")).toBe("unknown");
    expect(clientKeyOf(null)).toBe("unknown");
  });
});

describe("clientViewOptions", () => {
  it("只列出现过的值，顺序固定，未知值不丢", () => {
    const options = clientViewOptions([
      request("1", "VSCode", 1, "2026-11-19"),
      request("2", "JetBrainsPyCharm", 1, "2026-11-19"),
      request("3", "VSCode", 1, "2026-11-19"),
      request("4", "—", 1, "2026-11-19"),
    ]);
    expect(options.map((option) => option.key)).toEqual(["vscode", "jetbrains", "unknown"]);
    expect(options[0].count).toBe(2);
    expect(options[2].label).toBe("未知");
  });
});

describe("filterOfficialByClient", () => {
  it("据明细汇总：只留该客户端，四窗口都从明细重算", () => {
    const today = dateKey(new Date());
    const usage = filterOfficialByClient(
      official([
        request("1", "JetBrainsPyCharm", 0, today),
        request("2", "VSCode", 5, today),
        request("3", "WorkBuddy", 7, today),
        request("4", "WorkBuddy", 11, today),
      ]),
      "workbuddy",
      null,
    );

    expect(usage.records).toBe(2);
    expect(usage.official.summary.usageToday).toBe(18);
    expect(usage.official.summary.usage7Days).toBe(18);
    expect(usage.official.summary.usageThisMonth).toBe(18);
    expect(usage.official.requests).toHaveLength(2);
    expect(usage.truncated).toBe(false);
    // 官方聚合值（999）必须被明细重算覆盖，不能只换文案不换数字。
    expect(usage.official.summary.usageToday).not.toBe(999);
  });

  it("账号明细被截断时标记 truncated（数字只是下限）", () => {
    const base = official([request("1", "VSCode", 3, dateKey(new Date()))]);
    base.accounts[0].detailTruncated = true;
    const usage = filterOfficialByClient(base, "vscode", null);
    expect(usage.truncated).toBe(true);
  });

  it("按档位账号集合再收一层（与档位筛选叠加）", () => {
    const today = dateKey(new Date());
    const base = official([
      request("1", "VSCode", 3, today, "acc-1"),
      request("2", "VSCode", 4, today, "acc-2"),
    ]);
    const usage = filterOfficialByClient(base, "vscode", new Set(["acc-1"]));
    expect(usage.records).toBe(1);
    expect(usage.official.summary.usageToday).toBe(3);
  });
});
