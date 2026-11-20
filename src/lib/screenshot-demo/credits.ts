import { displayName } from "../account-display";
import type { CreditExpiry, CreditOfficialUsageModel, CreditStatistics } from "../types";
import { accountVariant, variantSupportsCheckin } from "../variant";
import type { DemoContext } from "./accounts";
import { atLocalTime, accounts, currentAccountIds, futureAt, hydratedAccounts, localDate } from "./accounts";

/**
 * 积分演示数据：积分包 fixture、到期明细与统计页聚合。
 *
 * 与账号列表严格同索引（usageSeeds / creditPackages / accounts），新增账号需同步追加。
 */

export const CREDIT_DEMO_COMMANDS = ["get_credit_expiry", "get_credit_statistics"] as const;

/** 演示数据里的模型名（只作为类型来源；新增 fixture 时同步这里）。 */
type ModelName = "deepseek-v4-flash" | "kimi-k3-1" | "deepseek-v4-pro" | "glm-5.2" | "hy3";

interface ModelSeed {
  model: ModelName;
  requestCount: number;
  credit: number;
}

interface AccountUsageSeed {
  requestCount: number;
  models: ModelSeed[];
}

// Counts and relative model roles follow anonymous aggregates from the sanitized local cache.
// No upstream request row or identifier is copied into this fixture.
// 与 `accounts` 严格同索引：`buildStatistics()` 按账号索引取这里的 seed。
const usageSeeds: AccountUsageSeed[] = [
  {
    requestCount: 2243,
    models: [
      { model: "deepseek-v4-flash", requestCount: 2133, credit: 1794.39 },
      { model: "kimi-k3-1", requestCount: 24, credit: 2497.16 },
      { model: "deepseek-v4-pro", requestCount: 23, credit: 3.63 },
      { model: "glm-5.2", requestCount: 1, credit: 33.63 },
      { model: "hy3", requestCount: 62, credit: 0 },
    ],
  },
  {
    requestCount: 679,
    models: [
      { model: "deepseek-v4-flash", requestCount: 659, credit: 1270.62 },
      { model: "hy3", requestCount: 20, credit: 0 },
    ],
  },
  {
    requestCount: 318,
    models: [
      { model: "deepseek-v4-flash", requestCount: 309, credit: 595.08 },
      { model: "hy3", requestCount: 9, credit: 0 },
    ],
  },
  // 国际版 A：用量集中在主力模型，国际版演示视图因此有可读的曲线与模型占比。
  {
    requestCount: 1420,
    models: [
      { model: "deepseek-v4-flash", requestCount: 1338, credit: 1120.5 },
      { model: "kimi-k3-1", requestCount: 68, credit: 642.7 },
      { model: "glm-5.2", requestCount: 14, credit: 33.63 },
    ],
  },
  // 国际版 B：轻量账号，用于演示同一档位内的用量差异。
  {
    requestCount: 268,
    models: [
      { model: "deepseek-v4-flash", requestCount: 240, credit: 482.9 },
      { model: "hy3", requestCount: 28, credit: 0 },
    ],
  },
  // 企业版 D：轻量用量，让统计页的企业版账号也有可读数据。
  {
    requestCount: 156,
    models: [
      { model: "deepseek-v4-flash", requestCount: 148, credit: 268.4 },
      { model: "hy3", requestCount: 8, credit: 0 },
    ],
  },
];

/** 演示积分包种子：[名称, 总量, 剩余, N 天后到期]；与 `accounts` 严格同索引。 */
type CreditPackageSeed = readonly [packageName: string, total: number, remaining: number, expireDays: number];

/**
 * 演示积分包按账号索引与 `accounts` 严格对应，覆盖卡片内容区的多种形态：
 * A（1 条）/ B（2 条）为个人版，覆盖 1 行、2 行两种高度；C 为企业版单条
 * 「企业配额」（企业接口只返回一条配额资源）；D 为企业版不限量，无积分包。
 * 说明：原先由 C 覆盖的「>2 条 → 查看全部积分包」样例随企业版改造移除，
 * 如需该样例，应由后续新增的个人版账号承载，而不是改回 C 的个人版数据。
 */
const creditPackages: readonly (readonly CreditPackageSeed[])[] = [
  // 账号 A：1 条 → 1 行，无「查看全部积分包」
  [
    ["CodeBuddy 新用户体验包", 800, 386.4, 5],
  ],
  // 账号 B：2 条 → 2 行，无「查看全部积分包」（链接在 resources.length > 2 时才出现）
  [
    ["CodeBuddy 个人版积分包", 1800, 905.5, 42],
    ["CodeBuddy 签到赠送积分", 240, 174.35, 15],
  ],
  // 账号 C（企业版有限配额）：单条「企业配额」，总量 2000 / 剩余 1500 / 30 天重置
  [
    ["企业配额", 2000, 1500, 30],
  ],
  // 国际版账号：合成码不会命中官方商品码映射，因此展示的就是这里自造的名字；
  // 沿用官方命名口径，覆盖「试用 / 赠送 / 购买」三类国际版语义。
  [
    ["试用版基础用量", 2000, 1340.8, 30],
    ["版本赠送用量", 600, 402.35, 75],
  ],
  [
    ["购买积分", 3000, 2180.6, 12],
    ["试用版基础用量", 500, 318.75, 60],
    ["平台奖励积分", 800, 512.4, 90],
  ],
  // 账号 D（企业版不限量）：无积分包，creditExpiry 返回 unlimited 形态
  [],
];

function creditExpiry(accountId: string): CreditExpiry {
  const index = Math.max(0, accounts.findIndex((account) => account.id === accountId));
  const account = accounts[index] ?? accounts[0];
  const packages = creditPackages[index] ?? [];
  const resources = packages.map(([packageName, total, remaining, expireDays], packageIndex) => ({
    packageCode: `demo-package-${index + 1}-${packageIndex + 1}`,
    packageName,
    total,
    remaining,
    used: Number((total - remaining).toFixed(2)),
    status: 1,
    expireAt: futureAt(expireDays),
    expired: false,
    expiringSoon: expireDays <= 7,
  }));
  const totalCapacity = resources.reduce((sum, resource) => sum + resource.total, 0);
  const totalRemaining = resources.reduce((sum, resource) => sum + resource.remaining, 0);
  const expiringSoonRemaining = resources.filter((resource) => resource.expiringSoon).reduce((sum, resource) => sum + resource.remaining, 0);
  const updatedAt = Date.now() - (index + 1) * 4 * 60 * 1000;

  // 企业版（issue #167）：企业配额不在个人版接口里，卡片按企业形态渲染。
  // 无积分包 = 不限量（limitNum=-1）；有积分包 = 单条「企业配额」有限配额。
  if (account.isEnterprise && resources.length === 0) {
    return {
      ok: true,
      accountId: account.id,
      accountName: displayName(account),
      updatedAt,
      unlimited: true,
      totalCapacity: null,
      totalRemaining: null,
      expiringSoonRemaining: 0,
      expiredRemaining: 0,
      soonestExpireAt: null,
      expiringSoon: false,
      expired: false,
      resources: [],
    };
  }

  return {
    ok: true,
    accountId: account.id,
    accountName: displayName(account),
    updatedAt,
    // 企业版有限配额显式标记 unlimited=false；个人版不新增字段（保持原有 mock 形态）。
    ...(account.isEnterprise ? { unlimited: false } : {}),
    totalCapacity,
    totalRemaining: Number(totalRemaining.toFixed(2)),
    expiringSoonRemaining: Number(expiringSoonRemaining.toFixed(2)),
    expiredRemaining: 0,
    soonestExpireAt: Math.min(...resources.map((resource) => resource.expireAt)),
    expiringSoon: expiringSoonRemaining > 0,
    expired: false,
    resources,
  };
}

function dailyWeight(accountIndex: number, dayIndex: number): number {
  const weekdayWave = [0.72, 1.08, 0.93, 1.22, 0.84, 1.16, 1.01][dayIndex % 7];
  const quiet = (dayIndex + accountIndex * 4) % 13 === 0 ? 0.16 : 1;
  return weekdayWave * quiet * (1 + accountIndex * 0.035);
}

function distributeModels(seed: AccountUsageSeed, accountIndex: number) {
  const weights = Array.from({ length: 30 }, (_, dayIndex) => dailyWeight(accountIndex, dayIndex));
  const weightTotal = weights.reduce((sum, weight) => sum + weight, 0);
  const countSeries = seed.models.map((model) => {
    const raw = weights.map((weight) => (model.requestCount * weight) / weightTotal);
    const values = raw.map(Math.floor);
    const remaining = model.requestCount - values.reduce((sum, value) => sum + value, 0);
    const byFraction = raw.map((value, index) => ({ index, fraction: value - Math.floor(value) })).sort((left, right) => right.fraction - left.fraction);
    for (let index = 0; index < remaining; index += 1) values[byFraction[index].index] += 1;
    return values;
  });
  const creditSeries = seed.models.map((model) => {
    const values = weights.map((weight) => Number(((model.credit * weight) / weightTotal).toFixed(2)));
    const drift = Number((model.credit - values.reduce((sum, value) => sum + value, 0)).toFixed(2));
    values[values.length - 1] = Number((values[values.length - 1] + drift).toFixed(2));
    return values;
  });
  return Array.from({ length: 30 }, (_, dayIndex) => {
    const models = seed.models.map((model, modelIndex) => ({
      model: model.model,
      requestCount: countSeries[modelIndex][dayIndex],
      credit: creditSeries[modelIndex][dayIndex],
    }));
    return {
      date: localDate(29 - dayIndex),
      usage: Number(models.reduce((sum, model) => sum + model.credit, 0).toFixed(2)),
      models,
    };
  });
}

function sumModels(rows: { models: CreditOfficialUsageModel[] }[]): CreditOfficialUsageModel[] {
  const totals = new Map<string, CreditOfficialUsageModel>();
  for (const row of rows) {
    for (const model of row.models) {
      const current = totals.get(model.model) ?? { model: model.model, requestCount: 0, credit: 0 };
      current.requestCount += model.requestCount;
      current.credit = Number((current.credit + model.credit).toFixed(2));
      totals.set(model.model, current);
    }
  }
  return [...totals.values()].sort((left, right) => right.credit - left.credit);
}

function visibleRequests(accountIndex: number) {
  const account = accounts[accountIndex];
  const seed = usageSeeds[accountIndex];
  const hours = [16, 15, 17, 14, 1, 0, 3];
  const flashCredits = [0.13, 0.04, 0.2, 1, 0.08, 3.99, 0.45, 8.5, 24.56];
  const kimiCredits = [86.4, 103.2, 112.8, 128.4, 74.6];
  const proCredits = [0.04, 0.13, 0.2, 0.45];
  const weightedModels = seed.models.flatMap((model) =>
    Array.from({ length: Math.max(1, Math.round((model.requestCount / seed.requestCount) * 100)) }, () => model.model),
  );

  return Array.from({ length: 100 }, (_, rowIndex) => {
    const daysAgo = Math.floor(rowIndex / 8);
    const hour = hours[(rowIndex + accountIndex * 2) % hours.length];
    const minute = (rowIndex * 7 + accountIndex * 11) % 60;
    const ts = new Date(atLocalTime(daysAgo, hour, minute));
    const model = rowIndex < seed.models.length
      ? seed.models[rowIndex].model
      : weightedModels[rowIndex % weightedModels.length];
    const credit = model === "hy3"
      ? 0
      : model === "kimi-k3-1"
        ? kimiCredits[(rowIndex + accountIndex) % kimiCredits.length]
        : model === "glm-5.2"
          ? 33.63
          : model === "deepseek-v4-pro"
            ? proCredits[(rowIndex + accountIndex) % proCredits.length]
            : flashCredits[(rowIndex + accountIndex * 3) % flashCredits.length];
    return {
      accountId: account.id,
      accountName: displayName(account),
      requestId: `demo-request-${String(accountIndex + 1).padStart(2, "0")}-${String(rowIndex + 1).padStart(4, "0")}`,
      credit,
      model,
      client: rowIndex % 50 === 0 ? "CodeBuddyIDE" : "CLI",
      requestTime: `${localDate(daysAgo)} ${String(ts.getHours()).padStart(2, "0")}:${String(ts.getMinutes()).padStart(2, "0")}:00`,
    };
  });
}

function buildStatistics(): CreditStatistics {
  const demoAccounts = hydratedAccounts();
  const accountDaily = usageSeeds.map((seed, index) => distributeModels(seed, index));
  const daily = accountDaily[0].map((_, dayIndex) => {
    const models = new Map<string, CreditOfficialUsageModel>();
    for (const rows of accountDaily) {
      for (const model of rows[dayIndex].models) {
        const current = models.get(model.model) ?? { model: model.model, requestCount: 0, credit: 0 };
        current.requestCount += model.requestCount;
        current.credit = Number((current.credit + model.credit).toFixed(2));
        models.set(model.model, current);
      }
    }
    const modelRows = [...models.values()];
    return { date: accountDaily[0][dayIndex].date, usage: Number(modelRows.reduce((sum, model) => sum + model.credit, 0).toFixed(2)), models: modelRows };
  });
  const sumRecent = (rows: { usage: number }[], count: number) => Number(rows.slice(-count).reduce((sum, row) => sum + row.usage, 0).toFixed(2));
  const monthPrefix = localDate(0).slice(0, 7);
  const sumMonth = (rows: { date: string; usage: number }[]) => Number(rows.filter((row) => row.date.startsWith(monthPrefix)).reduce((sum, row) => sum + row.usage, 0).toFixed(2));
  const generatedAt = Date.now() - 3 * 60 * 1000;
  const creditRows = accounts.map((account) => creditExpiry(account.id));
  const officialAccounts = demoAccounts.map((account, index) => ({
    accountId: account.id,
    accountName: displayName(account),
    ok: true,
    requestCount: usageSeeds[index].requestCount,
    detailTruncated: true,
    usageToday: accountDaily[index][accountDaily[index].length - 1]?.usage ?? 0,
    usage7Days: sumRecent(accountDaily[index], 7),
    usageThisMonth: sumMonth(accountDaily[index]),
    reportedTotal: usageSeeds[index].requestCount,
    fetchedCount: usageSeeds[index].requestCount,
    models: sumModels(accountDaily[index]),
    daily: accountDaily[index],
  }));
  const usageToday = daily[daily.length - 1]?.usage ?? 0;
  const usage7Days = sumRecent(daily, 7);
  const usageThisMonth = sumMonth(daily);
  const totalRemaining = creditRows.reduce((sum, credit) => sum + (credit.totalRemaining ?? 0), 0);
  const totalCapacity = creditRows.reduce((sum, credit) => sum + (credit.totalCapacity ?? 0), 0);

  return {
    generatedAt,
    retentionDays: 90,
    coverageStartAt: atLocalTime(29, 0, 0),
    // 签到计数与 `checkinLogs()` / `events` 的构造规则保持一致：
    // 支持签到的账号共 4 个（国内版 A/B/C/D），其中 B 为 already、其余为 success。
    summary: { currentRemaining: Number(totalRemaining.toFixed(2)), currentCapacity: totalCapacity, usageToday, usage7Days, usageThisMonth, todayCheckedInAccounts: 4, todaySuccess: 3, todayAlready: 1, todayFailed: 0 },
    daily,
    accounts: demoAccounts.map((account, index) => {
      // 国际版没有签到接口：签到态一律留空，与 `variantSupportsCheckin` 一致。
      const checkinSupported = variantSupportsCheckin(accountVariant(account));
      const checkinResult = index === 1 ? "already" : "success";
      return {
        accountId: account.id,
        accountName: displayName(account),
        isCurrent: currentAccountIds.has(account.id),
        currentRemaining: creditRows[index].totalRemaining ?? null,
        totalCapacity: creditRows[index].totalCapacity ?? null,
        lastSnapshotAt: generatedAt - index * 120_000,
        usageToday: officialAccounts[index].usageToday ?? 0,
        usage7Days: officialAccounts[index].usage7Days ?? 0,
        usageThisMonth: officialAccounts[index].usageThisMonth ?? 0,
        checkedInToday: checkinSupported ? true : null,
        checkinStatusToday: checkinSupported ? checkinResult : null,
        lastCheckinAt: checkinSupported ? atLocalTime(0, 8, 6 + index * 9) : null,
        lastCheckinResult: checkinSupported ? checkinResult : null,
        daily: accountDaily[index],
      };
    }),
    // 签到事件按档位能力生成：国际版没有签到接口，不构造事件（国内版索引与取值不变）。
    events: demoAccounts.flatMap((account, index) => variantSupportsCheckin(accountVariant(account))
      ? [{
          kind: "checkin" as const,
          ts: atLocalTime(0, 8, 6 + index * 9),
          date: localDate(0),
          accountId: account.id,
          accountName: displayName(account),
          result: index === 1 ? "already" : "success",
        }]
      : []),
    officialUsage: {
      status: "complete",
      rangeStart: localDate(29),
      rangeEnd: localDate(0),
      collectedAt: generatedAt,
      summary: { usageToday, usage7Days, usageThisMonth },
      daily,
      accounts: officialAccounts,
      requests: accounts.flatMap((_, index) => visibleRequests(index)),
      models: sumModels(daily),
      detailLimitPerAccount: 100,
      errors: [],
    },
  };
}

export function creditsDemoResponse(
  command: string,
  _ctx: DemoContext,
  args?: Record<string, unknown>,
): unknown | undefined {
  switch (command) {
    case "get_credit_expiry": return creditExpiry(String(args?.accountId ?? ""));
    case "get_credit_statistics": return buildStatistics();
    default: return undefined;
  }
}