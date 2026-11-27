

import { dateDaysAgo, dateKey } from "@/lib/format";
import { statsDateInRange, type StatsRangeKey } from "@/lib/stats-range";
import type {
  CreditLedgerClientGroup,
  CreditLedgerSnapshot,
  CreditOfficialUsage,
  CreditOfficialUsageAccount,
  CreditOfficialUsageModel,
  CreditOfficialUsageRequest,
  CreditStatsAccount,
  CreditStatsDailyPoint,
  CreditStatsEvent,
  CreditStatistics,
  WbVariant,
} from "@/lib/types";
import { DEFAULT_VARIANT, normalizeVariant, variantLabel } from "@/lib/variant";

/**
 * 逐日切片与汇总口径统一用 `StatsRangeKey`（见 `@/lib/stats-range`）：
 * 范围选项只在那一处定义，本文件只负责「按范围过滤 / 求和」。
 */
export type RangeKey = StatsRangeKey;

export type StatsVariantView = "all" | "cn" | "ai";

/**
 * 积分明细的客户端维度（第四轮 06）。
 *
 * 官方只提供「按账号 / 按日 / 按模型」聚合，没有按客户端聚合，所以客户端筛选只能
 * 在**明细**上做。实测 `client` 取值：`WorkBuddy`、`VSCode`、`JetBrainsPyCharm`、
 * `CodeBuddyIDE`、`—`（未知）。
 */
export type CreditClientKey = "all" | "workbuddy" | "vscode" | "jetbrains" | "ide" | "cli" | "unknown";

export const CLIENT_VIEW_LABELS: Record<CreditClientKey, string> = {
  all: "全部客户端",
  workbuddy: "WorkBuddy",
  vscode: "VS Code",
  jetbrains: "JetBrains",
  ide: "CodeBuddy IDE",
  cli: "CLI",
  unknown: "未知",
};

/** 固定顺序：只列数据里出现过的值，但组序不随消耗高低跳动。 */
const CLIENT_ORDER: CreditClientKey[] = [
  "workbuddy",
  "vscode",
  "jetbrains",
  "ide",
  "cli",
  "unknown",
];

/**
 * 把官方明细里的 `client` 原文归一成一个筛选键。
 *
 * JetBrains 会带上产品名（实测 `JetBrainsPyCharm`，换 IDEA 是 `JetBrainsIntelliJ`），
 * 所以用前缀匹配；`—` 与空串归「未知」且不丢弃。
 */
export function clientKeyOf(client: string | null | undefined): CreditClientKey {
  const text = (client ?? "").trim();
  if (!text || text === "—") return "unknown";
  if (/^jetbrains/i.test(text)) return "jetbrains";
  if (/^vs\s*code/i.test(text) || /^vscode/i.test(text)) return "vscode";
  if (/^workbuddy/i.test(text)) return "workbuddy";
  if (/^codebuddy\s*ide/i.test(text) || /^codebuddyide$/i.test(text)) return "ide";
  if (/cli/i.test(text)) return "cli";
  return "unknown";
}

export interface ClientViewOption {
  key: CreditClientKey;
  label: string;
  count: number;
}

/** 客户端筛选选项：只列数据里出现过的值（含「未知」），按固定顺序排序。 */
export function clientViewOptions(
  requests: CreditOfficialUsageRequest[] | undefined,
): ClientViewOption[] {
  const counts = new Map<CreditClientKey, number>();
  for (const request of requests ?? []) {
    const key = clientKeyOf(request.client);
    counts.set(key, (counts.get(key) ?? 0) + 1);
  }
  return CLIENT_ORDER.filter((key) => (counts.get(key) ?? 0) > 0).map((key) => ({
    key,
    label: CLIENT_VIEW_LABELS[key],
    count: counts.get(key) ?? 0,
  }));
}

function requestDate(request: CreditOfficialUsageRequest): string {
  // requestTime 是「YYYY-MM-DD HH:mm:ss」文本，前 10 位即日期。
  return (request.requestTime ?? "").slice(0, 10);
}

export interface ClientDetailUsage {
  /** 只含该客户端明细的官方口径响应：四卡、趋势、模型、明细共用。 */
  official: CreditOfficialUsage;
  /** 参与汇总的明细条数。 */
  records: number;
  /** 任一账号明细被截断：数字只是下限。 */
  truncated: boolean;
}

/**
 * 用**明细**重算某个客户端的官方口径。
 *
 * 官方没有按客户端聚合，只有明细里带 `client`，所以「据明细汇总」是这个维度唯一
 * 诚实的算法。明细每账号上限 1000 条：一旦命中上限就把 `truncated` 置真，UI 必须
 * 写成「部分截断 / 下限」，不能当成完整总量。
 */
export function filterOfficialByClient(
  official: CreditOfficialUsage,
  client: CreditClientKey,
  accountIds: Set<string> | null,
): ClientDetailUsage {
  const requests = official.requests.filter(
    (request) =>
      clientKeyOf(request.client) === client &&
      (accountIds === null || accountIds.has(request.accountId)),
  );
  const today = dateKey(new Date());
  const weekStart = dateDaysAgo(6);
  const monthStart = today.slice(0, 7);
  let usageToday = 0;
  let usage7Days = 0;
  let usageThisMonth = 0;
  const usageByDate = new Map<string, number>();
  const modelsByDate = new Map<string, Map<string, CreditOfficialUsageModel>>();
  const models = new Map<string, CreditOfficialUsageModel>();
  const byAccount = new Map<string, CreditOfficialUsageRequest[]>();
  for (const request of requests) {
    const date = requestDate(request);
    const credit = Number.isFinite(request.credit) ? request.credit : 0;
    if (date === today) usageToday += credit;
    if (date >= weekStart && date <= today) usage7Days += credit;
    if (date.startsWith(monthStart)) usageThisMonth += credit;
    usageByDate.set(date, (usageByDate.get(date) ?? 0) + credit);

    const modelName = request.model?.trim() || "未知模型";
    const modelEntry = models.get(modelName) ?? { model: modelName, requestCount: 0, credit: 0 };
    modelEntry.requestCount += 1;
    modelEntry.credit += credit;
    models.set(modelName, modelEntry);
    const dailyModels = modelsByDate.get(date) ?? new Map<string, CreditOfficialUsageModel>();
    const dailyEntry = dailyModels.get(modelName) ?? { model: modelName, requestCount: 0, credit: 0 };
    dailyEntry.requestCount += 1;
    dailyEntry.credit += credit;
    dailyModels.set(modelName, dailyEntry);
    modelsByDate.set(date, dailyModels);

    const list = byAccount.get(request.accountId) ?? [];
    list.push(request);
    byAccount.set(request.accountId, list);
  }

  const truncated = official.accounts.some(
    (account) => account.detailTruncated && (accountIds === null || accountIds.has(account.accountId)),
  );

  const accounts: CreditOfficialUsageAccount[] = official.accounts
    .filter((account) => byAccount.has(account.accountId))
    .map((account) => {
      const rows = byAccount.get(account.accountId) ?? [];
      let accountToday = 0;
      let account7Days = 0;
      let accountMonth = 0;
      const accountDaily = new Map<string, number>();
      const accountModels = new Map<string, CreditOfficialUsageModel>();
      for (const request of rows) {
        const date = requestDate(request);
        const credit = Number.isFinite(request.credit) ? request.credit : 0;
        if (date === today) accountToday += credit;
        if (date >= weekStart && date <= today) account7Days += credit;
        if (date.startsWith(monthStart)) accountMonth += credit;
        accountDaily.set(date, (accountDaily.get(date) ?? 0) + credit);
        const modelName = request.model?.trim() || "未知模型";
        const entry = accountModels.get(modelName) ?? {
          model: modelName,
          requestCount: 0,
          credit: 0,
        };
        entry.requestCount += 1;
        entry.credit += credit;
        accountModels.set(modelName, entry);
      }
      return {
        ...account,
        requestCount: rows.length,
        usageToday: accountToday,
        usage7Days: account7Days,
        usageThisMonth: accountMonth,
        models: [...accountModels.values()].sort((left, right) => right.credit - left.credit),
        daily: [...accountDaily.entries()]
          .sort(([left], [right]) => left.localeCompare(right))
          .map(([date, usage]) => ({ date, usage })),
      };
    });

  const mergedModels = [...models.values()].sort(
    (left, right) =>
      right.credit - left.credit ||
      right.requestCount - left.requestCount ||
      left.model.localeCompare(right.model),
  );

  return {
    official: {
      ...official,
      status: official.status,
      summary: { usageToday, usage7Days, usageThisMonth },
      // 日期轴沿用官方序列，空日期保留 0，趋势图不会因为筛选而缩短。
      daily: official.daily.map((point) => ({
        date: point.date,
        usage: usageByDate.get(point.date) ?? 0,
        models: [...(modelsByDate.get(point.date)?.values() ?? [])],
      })),
      models: mergedModels,
      accounts,
      requests,
    },
    records: requests.length,
    truncated,
  };
}

export function statsVariantViewLabel(view: StatsVariantView): string {
  return view === "all" ? "全部" : variantLabel(view);
}

export const VARIANT_VIEW_OPTIONS: { key: StatsVariantView; label: string }[] = [
  { key: "all", label: statsVariantViewLabel("all") },
  { key: "cn", label: statsVariantViewLabel("cn") },
  { key: "ai", label: statsVariantViewLabel("ai") },
];

/** 账号消耗构成的分组顺序，与顶部档位切换一致；固定顺序避免组序随消耗高低跳动。 */
export const VARIANT_GROUP_ORDER: WbVariant[] = ["cn", "ai"];


/**
 * 读取指定范围的消耗值；官方账号不可用（字段为 null）时返回 null，调用方不计入占比分母。
 * 「近 30 天」没有对应的汇总字段，由 rangeUsage 从逐日数据求和。
 */
export function usageShareValue(
  account: { usageToday?: number | null; usage7Days?: number | null; usageThisMonth?: number | null },
  daily: CreditStatsDailyPoint[] | undefined,
  range: RangeKey,
): number | null {
  const { usageToday, usage7Days, usageThisMonth } = account;
  if (usageToday == null || usage7Days == null || usageThisMonth == null) return null;
  return rangeUsage({ usageToday, usage7Days, usageThisMonth }, daily ?? [], range);
}

export function formatChartDate(date: string): string {
  return date.slice(5).replace("-", "/");
}

export function accountLabel(account: { accountName?: string | null; accountId: string }): string {
  return account.accountName || account.accountId;
}

/** 读取可选的后端档位标记；缺省（当前后端不下发）时返回 undefined，由调用方回退到映射表。 */
function taggedVariant(value: { variant?: unknown }): WbVariant | undefined {
  return value.variant === undefined ? undefined : normalizeVariant(value.variant);
}

export function resolveAccountVariant(
  account: CreditStatsAccount,
  variantByAccountId: Map<string, WbVariant>,
): WbVariant {
  return taggedVariant(account) ?? variantByAccountId.get(account.accountId) ?? DEFAULT_VARIANT;
}

function resolveEventVariant(
  event: CreditStatsEvent,
  variantByAccountId: Map<string, WbVariant>,
): WbVariant {
  const tagged = taggedVariant(event);
  if (tagged) return tagged;
  if (event.accountId) return variantByAccountId.get(event.accountId) ?? DEFAULT_VARIANT;
  return DEFAULT_VARIANT;
}

function mergeOfficialModels(models: CreditOfficialUsageModel[]): CreditOfficialUsageModel[] {
  const usage = new Map<string, { requestCount: number; credit: number }>();
  for (const item of models) {
    const raw = item.model?.trim() ?? "";
    const name = !raw || raw === "—" ? "未知模型" : raw;
    const entry = usage.get(name) ?? { requestCount: 0, credit: 0 };
    entry.requestCount += item.requestCount;
    entry.credit += item.credit;
    usage.set(name, entry);
  }
  return [...usage.entries()]
    .map(([model, value]) => ({ model, requestCount: value.requestCount, credit: value.credit }))
    .sort(
      (left, right) =>
        right.credit - left.credit ||
        right.requestCount - left.requestCount ||
        left.model.localeCompare(right.model),
    );
}

function officialStatusFromAccounts(
  accounts: CreditOfficialUsageAccount[],
): CreditOfficialUsage["status"] {
  if (accounts.length === 0) return "unavailable";
  const okCount = accounts.filter((account) => account.ok).length;
  if (okCount === accounts.length) return "complete";
  if (okCount > 0) return "partial";
  return "unavailable";
}

/** 仅 `cn` / `ai` 视图调用；`all` 必须直接使用后端聚合值（D1）。 */
export function recomputeLocalStats(
  stats: CreditStatistics,
  accountIds: Set<string>,
  viewVariant: WbVariant,
  variantByAccountId: Map<string, WbVariant>,
): CreditStatistics {
  const accounts = stats.accounts.filter((account) => accountIds.has(account.accountId));
  const events = stats.events.filter(
    (event) => resolveEventVariant(event, variantByAccountId) === viewVariant,
  );

  let currentRemaining = 0;
  let currentCapacity = 0;
  let usageToday = 0;
  let usage7Days = 0;
  let usageThisMonth = 0;
  const usageByDate = new Map<string, number>();
  for (const account of accounts) {
    usageToday += account.usageToday;
    usage7Days += account.usage7Days;
    usageThisMonth += account.usageThisMonth;
    // 与后端口径一致：currentRemaining / currentCapacity 只统计 isCurrent 账号
    if (account.isCurrent) {
      currentRemaining += account.currentRemaining ?? 0;
      currentCapacity += account.totalCapacity ?? 0;
    }
    for (const point of account.daily ?? []) {
      usageByDate.set(point.date, (usageByDate.get(point.date) ?? 0) + point.usage);
    }
  }

  const today = dateKey(new Date());
  let todaySuccess = 0;
  let todayAlready = 0;
  let todayFailed = 0;
  const checkedIn = new Set<string>();
  for (const event of events) {
    if (event.kind !== "checkin" || event.date !== today) continue;
    if (event.result === "success") todaySuccess += 1;
    else if (event.result === "already") todayAlready += 1;
    else todayFailed += 1;
    if ((event.result === "success" || event.result === "already") && event.accountId) {
      checkedIn.add(event.accountId);
    }
  }

  return {
    ...stats,
    summary: {
      currentRemaining,
      currentCapacity,
      usageToday,
      usage7Days,
      usageThisMonth,
      todayCheckedInAccounts: checkedIn.size,
      todaySuccess,
      todayAlready,
      todayFailed,
    },
    // 日期轴以后端 stats.daily 为基准，避免过滤后曲线变短
    daily: stats.daily.map((point) => ({
      date: point.date,
      usage: usageByDate.get(point.date) ?? 0,
    })),
    accounts,
    events,
  };
}

/** 仅 `cn` / `ai` 视图调用；官方账号无 variant，用 accountId 关联 stats 档位集合（D2）。 */
export function recomputeOfficialUsage(
  official: CreditOfficialUsage,
  accountIds: Set<string>,
): CreditOfficialUsage {
  const accounts = official.accounts.filter((account) => accountIds.has(account.accountId));
  const usageByDate = new Map<string, number>();
  const modelsByDate = new Map<string, CreditOfficialUsageModel[]>();
  const mergedModels: CreditOfficialUsageModel[] = [];
  let usageToday = 0;
  let usage7Days = 0;
  let usageThisMonth = 0;
  for (const account of accounts) {
    usageToday += account.usageToday ?? 0;
    usage7Days += account.usage7Days ?? 0;
    usageThisMonth += account.usageThisMonth ?? 0;
    if (account.models) mergedModels.push(...account.models);
    for (const point of account.daily ?? []) {
      usageByDate.set(point.date, (usageByDate.get(point.date) ?? 0) + point.usage);
      if (point.models && point.models.length > 0) {
        const list = modelsByDate.get(point.date) ?? [];
        list.push(...point.models);
        modelsByDate.set(point.date, list);
      }
    }
  }
  const visibleIds = new Set(accounts.map((account) => account.accountId));

  return {
    ...official,
    status: officialStatusFromAccounts(accounts),
    summary: { usageToday, usage7Days, usageThisMonth },
    // 日期轴以官方 daily 既有序列为基准，保留范围内空日期
    daily: official.daily.map((point) => ({
      date: point.date,
      usage: usageByDate.get(point.date) ?? 0,
      models: mergeOfficialModels(modelsByDate.get(point.date) ?? []),
    })),
    models: mergeOfficialModels(mergedModels),
    accounts,
    requests: official.requests.filter((request) => visibleIds.has(request.accountId)),
    errors: official.errors.filter((error) => visibleIds.has(error.accountId)),
  };
}

export function isOfficialUsageAvailable(officialUsage?: CreditOfficialUsage): boolean {
  return officialUsage?.status === "complete" || officialUsage?.status === "partial";
}

export function officialAccountFor(
  officialUsage: CreditOfficialUsage | undefined,
  accountId: string,
): CreditOfficialUsageAccount | undefined {
  return officialUsage?.accounts.find((account) => account.accountId === accountId);
}

/** 按范围切出逐日点（与「按日」明细、趋势图、四卡共用同一套日期判定）。 */
export function chartPoints(daily: CreditStatsDailyPoint[], range: RangeKey) {
  return daily.filter((point) => statsDateInRange(point.date, range));
}

/**
 * 指定范围的消耗合计。
 *
 * 今天 / 近 7 天 / 本月直接取后端（或账本）给的窗口汇总；30 天 / 90 天 / 全部由逐日
 * 数据求和——官方也不提供这三个窗口的汇总量，求和是唯一同源的口径（趋势图底部的
 * 「当前口径合计」用的就是这个函数，两处数字必然一致）。
 */
export function rangeUsage(
  summary: CreditStatistics["summary"] | CreditOfficialUsage["summary"],
  daily: CreditStatsDailyPoint[],
  range: RangeKey,
): number {
  switch (range) {
    case "today":
      return summary.usageToday;
    case "7d":
      return summary.usage7Days;
    case "month":
      return summary.usageThisMonth;
    case "30d":
    case "90d":
    case "all":
      return chartPoints(daily, range).reduce((sum, point) => sum + point.usage, 0);
  }
}

/** 范围内的单日峰值（无数据返回 null，避免把「没有数据」显示成 0）。 */
export function rangePeakUsage(daily: CreditStatsDailyPoint[], range: RangeKey): number | null {
  const points = chartPoints(daily, range);
  if (points.length === 0) return null;
  return points.reduce((max, point) => Math.max(max, point.usage), 0);
}

/** 占比文案：`<0.1%` 保留一位小数，0 与非法值返回 `—`。 */
export function formatPercent(ratio: number | null | undefined): string {
  if (typeof ratio !== "number" || !Number.isFinite(ratio)) return "—";
  const percent = ratio * 100;
  if (percent <= 0) return "0.0%";
  return percent < 0.05 ? "<0.1%" : `${percent.toFixed(1)}%`;
}

/** 账号消耗构成的一行：官方不可用的账号 `usage` 为 null（不计入分母）。 */
export interface AccountUsageRow {
  accountId: string;
  accountName: string;
  usage: number | null;
  variant: WbVariant;
}

/**
 * 账号消耗构成的唯一数据源。
 *
 * 折叠区（第一屏结论与「账号消耗构成」）都从这里取行，保证「头部账号占 X%」与下方
 * 条形图的百分比是同一份数字；改造前这两处各算一遍，切范围后容易互相矛盾。
 */
export function accountUsageRows(
  stats: CreditStatistics,
  officialUsage: CreditOfficialUsage | undefined,
  range: RangeKey,
  variantByAccountId: Map<string, WbVariant>,
  /** 「全部」时的账号级累计（来自账本 `accounts[].credit`；账本的按账号逐日为空）。 */
  allTimeByAccountId?: Map<string, number>,
): AccountUsageRow[] {
  const official = isOfficialUsageAvailable(officialUsage) ? officialUsage : undefined;
  // 构造期先不带 `variant`：账号档位只存在于账号库（`variantByAccountId`），官方响应
  // 与本地统计都没有这个字段。统一在下面的 `.map` 里补，取不到时回落到 `DEFAULT_VARIANT`
  // （与「档位筛选 = 全部」时一致），避免两条分支各写一次 fallback。
  const rows: Array<Omit<AccountUsageRow, "variant">> = official
    ? official.accounts.map((account) => ({
        accountId: account.accountId,
        accountName: account.accountName,
        usage: account.ok ? usageShareValue(account, account.daily, range) : null,
      }))
    : stats.accounts.map((account) => ({
        accountId: account.accountId,
        accountName: account.accountName,
        usage:
          range === "all" && allTimeByAccountId?.has(account.accountId)
            ? (allTimeByAccountId.get(account.accountId) ?? 0)
            : usageShareValue(account, account.daily, range),
      }));

  return rows
    .map((row) => ({ ...row, variant: variantByAccountId.get(row.accountId) ?? DEFAULT_VARIANT }))
    .sort(
      (a, b) => (b.usage ?? -1) - (a.usage ?? -1) || accountLabel(a).localeCompare(accountLabel(b)),
    );
}

/** 头部账号占比结论（第一屏那一行文字与折叠区标题共用）。 */
export interface TopAccountShare {
  label: string;
  usage: number;
  share: number;
  total: number;
  /** 有可测消耗值的账号数（官方失败账号不计入）。 */
  counted: number;
}

export function topAccountShare(rows: AccountUsageRow[]): TopAccountShare | null {
  const measurable = rows.filter(
    (row): row is AccountUsageRow & { usage: number } => row.usage !== null,
  );
  const total = measurable.reduce((sum, row) => sum + row.usage, 0);
  if (measurable.length === 0 || total <= 0) return null;
  const top = measurable.reduce((best, row) => (row.usage > best.usage ? row : best));
  return {
    label: accountLabel(top),
    usage: top.usage,
    share: top.usage / total,
    total,
    counted: measurable.length,
  };
}

/**
 * 指定范围 + 指定账号（null = 汇总）的模型消耗。
 *
 * 从官方逐日模型聚合求和：全量、不受明细条数上限影响。折叠区标题与「按模型分类」
 * 共用它，避免标题里的百分比与卡片内的条形各算一遍。
 */
export function rangeModels(
  officialUsage: CreditOfficialUsage,
  range: RangeKey,
  accountId: string | null,
): CreditOfficialUsageModel[] {
  const account = accountId
    ? officialUsage.accounts.find((item) => item.accountId === accountId)
    : undefined;
  const basePoints = chartPoints(account ? (account.daily ?? []) : officialUsage.daily, range);
  const totals = new Map<string, { requestCount: number; credit: number }>();
  for (const point of basePoints) {
    for (const item of point.models ?? []) {
      const entry = totals.get(item.model) ?? { requestCount: 0, credit: 0 };
      entry.requestCount += item.requestCount;
      entry.credit += item.credit;
      totals.set(item.model, entry);
    }
  }
  return [...totals.entries()]
    .map(([model, value]) => ({ model, requestCount: value.requestCount, credit: value.credit }))
    .sort(
      (left, right) =>
        right.credit - left.credit ||
        right.requestCount - left.requestCount ||
        left.model.localeCompare(right.model),
    );
}

/** 请求用量明细的排序维度：请求时间（默认倒序）与消耗 */
export type RequestSortKey = "time" | "credit";

export type SortDirection = "desc" | "asc";

/** 明细行的时间戳：requestTime 是「YYYY-MM-DD HH:mm:ss」文本，按字典序即时间序 */
function requestSortValue(request: CreditOfficialUsageRequest, key: RequestSortKey): number | string {
  if (key === "credit") return request.credit ?? 0;
  return request.requestTime ?? "";
}

export function compareRequests(
  left: CreditOfficialUsageRequest,
  right: CreditOfficialUsageRequest,
  key: RequestSortKey,
  direction: SortDirection,
): number {
  const leftValue = requestSortValue(left, key);
  const rightValue = requestSortValue(right, key);
  let order: number;
  if (typeof leftValue === "number" && typeof rightValue === "number") {
    order = leftValue - rightValue;
  } else {
    order = String(leftValue).localeCompare(String(rightValue));
  }
  if (order === 0) {
    // 同值时用请求 ID 兜底，保证排序稳定（数组排序在 V8 中稳定，但仍显式保证）
    order = (left.requestId ?? "").localeCompare(right.requestId ?? "");
  }
  return direction === "desc" ? -order : order;
}

/**
 * 账本 → 官方口径形状（第四轮 07）。
 *
 * 选「全部」时四卡、趋势、模型、账号分组全部改读账本；把它们映射成 `CreditOfficialUsage`
 * 让上层组件不需要知道数据来自官方接口还是本地 SQLite。整页只有这一处做形状转换。
 *
 * `status` 保持 `degraded` 时不返回对象：页面必须退回官方 30 天口径并提示账本不可用，
 * 绝不能拿空账本冒充「总计 0」。
 */
export function ledgerToOfficialUsage(
  ledger: CreditLedgerSnapshot | undefined,
  client: CreditClientKey,
): CreditOfficialUsage | null {
  if (!ledger || ledger.status !== "ok") return null;
  const group: CreditLedgerClientGroup | undefined =
    client === "all"
      ? undefined
      : (ledger.byClient ?? []).find((item) => clientKeyOf(item.client) === client);
  if (client !== "all" && !group) return null;
  const summary = group ? group.summary : ledger.summary;
  const daily = group ? group.daily : ledger.daily;
  const models = group ? group.models : ledger.models;
  const accounts: CreditOfficialUsageAccount[] = ledger.accounts.map((account) => ({
    accountId: account.accountId,
    accountName: account.accountName || account.accountId,
    ok: true,
    requestCount: account.requestCount,
    detailTruncated: false,
    // 账本按账号补了三个窗口值（见 credit_ledger）：缺字段的旧后端才退化成 null。
    usageToday: (account as { usageToday?: number | null }).usageToday ?? null,
    usage7Days: (account as { usage7Days?: number | null }).usage7Days ?? null,
    usageThisMonth: (account as { usageThisMonth?: number | null }).usageThisMonth ?? null,
    models: [],
    daily: [],
  }));
  return {
    status: "complete",
    rangeStart: ledger.startAt ?? "",
    rangeEnd: ledger.endAt ?? "",
    summary,
    daily: daily.map((point) => ({ date: point.date, usage: point.usage, models: [] })),
    accounts,
    requests: [],
    models,
    detailLimitPerAccount: 0,
    errors: [],
  };
}

/** 账本口径下的「剩余积分」仍是账号级：返回 true 让页面补一句说明。 */
export function ledgerRemainingIsAccountLevel(client: CreditClientKey): boolean {
  return client !== "all";
}

/** 账本里的客户端选项（「全部」范围下用它，而不是官方明细）。 */
export function ledgerClientViewOptions(
  ledger: CreditLedgerSnapshot | undefined,
): ClientViewOption[] {
  if (!ledger) return [];
  const counts = new Map<CreditClientKey, number>();
  for (const item of ledger.clients ?? []) {
    const key = clientKeyOf(item.client);
    counts.set(key, (counts.get(key) ?? 0) + item.requestCount);
  }
  return CLIENT_ORDER.filter((key) => (counts.get(key) ?? 0) > 0).map((key) => ({
    key,
    label: CLIENT_VIEW_LABELS[key],
    count: counts.get(key) ?? 0,
  }));
}
