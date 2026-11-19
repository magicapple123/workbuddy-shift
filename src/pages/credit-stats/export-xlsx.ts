import type {
  CreditLedgerSnapshot,
  CreditOfficialUsage,
  CreditStatistics,
} from "@/lib/types";
import { buildXlsx, type XlsxSheet } from "@/lib/xlsx";

import { creditStatsFileName } from "./export";

/** 导出上下文：范围 / 账本 / 客户端筛选的说明，写进每个 sheet 的末行。 */
export interface CreditExportContext {
  range?: "30d" | "90d" | "all";
  /** 账本快照：范围=全部时用于「按客户端」sheet 与说明。 */
  ledger?: CreditLedgerSnapshot;
  /** 当前客户端筛选的显示名（全部时省略）。 */
  clientLabel?: string;
  /** 当前口径说明（四卡顶部的同一句话）。 */
  caliberNote?: string;
}

function rangeLabelOf(range: CreditExportContext["range"]): string {
  if (range === "all") return "全部（本地账本累计）";
  if (range === "90d") return "近 90 天（官方窗口）";
  return "近 30 天（官方窗口）";
}

function clientRows(
  official: CreditOfficialUsage | undefined,
  context: CreditExportContext,
): (string | number)[][] {
  if (context.ledger?.status === "ok" && context.range === "all") {
    return (context.ledger.clients ?? []).map((item) => [
      item.client,
      item.requestCount,
      item.credit,
    ]);
  }
  const grouped = new Map<string, { count: number; credit: number }>();
  for (const request of official?.requests ?? []) {
    const key = request.client?.trim() || "—";
    const entry = grouped.get(key) ?? { count: 0, credit: 0 };
    entry.count += 1;
    entry.credit += Number.isFinite(request.credit) ? request.credit : 0;
    grouped.set(key, entry);
  }
  return [...grouped.entries()]
    .sort((left, right) => right[1].credit - left[1].credit)
    .map(([client, value]) => [client, value.count, value.credit]);
}

/**
 * 积分统计 Excel（第四轮 05）。
 *
 * 每个 sheet 的**最后一行**是说明行（口径、范围、截断状态）：Excel 用户经常只看
 * 单个 sheet，说明必须跟着数据走，而不是只写在总览页。
 */
export function creditStatsSheets(
  stats: CreditStatistics,
  official: CreditOfficialUsage | undefined,
  context: CreditExportContext = {},
): XlsxSheet[] {
  const note =
    context.caliberNote ??
    `范围：${rangeLabelOf(context.range)}${
      context.clientLabel ? ` · 客户端：${context.clientLabel}` : ""
    }`;
  const detailLimit = official?.detailLimitPerAccount ?? 1000;
  const requests = official?.requests ?? [];
  const truncated = (official?.accounts ?? []).some((account) => account.detailTruncated);
  const summary = official?.summary ?? {
    usageToday: stats.summary.usageToday,
    usage7Days: stats.summary.usage7Days,
    usageThisMonth: stats.summary.usageThisMonth,
  };

  const sheets: XlsxSheet[] = [
    {
      name: "总览",
      header: ["指标", "数值"],
      rows: [
        ["当前剩余（账号级）", stats.summary.currentRemaining],
        ["当前容量（账号级）", stats.summary.currentCapacity],
        ["今日消耗", summary.usageToday],
        ["近 7 天消耗", summary.usage7Days],
        ["本月消耗", summary.usageThisMonth],
        ["今日已签到账号", stats.summary.todayCheckedInAccounts],
        ["今日签到成功", stats.summary.todaySuccess],
        [`说明：${note}`, ""],
      ],
    },
    {
      name: "按账号",
      header: ["账号", "当前剩余", "容量", "今日消耗", "近 7 天", "本月", "档位"],
      rows: [
        ...stats.accounts.map((account) => {
          const usage = official?.accounts.find((item) => item.accountId === account.accountId);
          return [
            account.accountName || account.accountId,
            account.currentRemaining ?? "",
            account.totalCapacity ?? "",
            usage?.usageToday ?? account.usageToday,
            usage?.usage7Days ?? account.usage7Days,
            usage?.usageThisMonth ?? account.usageThisMonth,
            account.variant ?? "",
          ];
        }),
        [`说明：${note}；官方不可用的账号沿用本地观察值`, "", "", "", "", "", ""],
      ],
    },
    {
      name: "按日",
      header: ["日期", "消耗"],
      rows: [
        ...(official?.daily ?? stats.daily ?? []).map((row) => [row.date, row.usage]),
        [`说明：${note}`, ""],
      ],
    },
    {
      name: "按模型",
      header: ["模型", "请求数", "消耗"],
      rows: [
        ...(official?.models ?? []).map((row) => [row.model, row.requestCount, row.credit]),
        [`说明：${note}${context.range === "all" ? "；模型分组来自账本" : ""}`, "", ""],
      ],
    },
    {
      name: "按客户端",
      header: ["客户端", "请求数", "消耗"],
      rows: [
        ...clientRows(official, context),
        [
          `说明：${
            context.range === "all" ? "客户端分组来自本地账本" : "客户端分组来自官方逐请求明细"
          }；${note}`,
          "",
          "",
        ],
      ],
    },
    {
      name: "官方请求明细",
      header: ["请求时间", "账号", "模型", "积分", "客户端", "请求 id"],
      rows: [
        ...requests.map((row) => [
          row.requestTime,
          row.accountName,
          row.model,
          row.credit,
          row.client,
          row.requestId,
        ]),
        [
          context.range === "all"
            ? "说明：账本口径不导出逐请求明细（逐请求明细请在官方窗口范围查看）"
            : `说明：每账号上限 ${detailLimit} 条${
                truncated ? "（部分账号已截断）" : "（未截断）"
              } · 共 ${requests.length} 条`,
          "",
          "",
          "",
          "",
          "",
        ],
      ],
    },
    {
      name: "本地事件",
      header: ["时间", "账号", "类型", "数值", "备注"],
      rows: [
        ...(stats.events ?? []).map((event) => [
          new Date(event.ts).toISOString(),
          event.accountName,
          event.kind,
          event.kind === "usage" ? event.amount : event.result,
          event.kind === "usage" ? event.date : (event.error ?? ""),
        ]),
        ["说明：本地事件来自本工具快照与签到日志，与官方账单可能存在差异", "", "", "", ""],
      ],
    },
  ];
  return sheets;
}

export function buildCreditStatsXlsx(
  stats: CreditStatistics,
  official: CreditOfficialUsage | undefined,
  context: CreditExportContext = {},
): Uint8Array {
  return buildXlsx(creditStatsSheets(stats, official, context));
}

export function creditStatsXlsxFileName(variant: string): string {
  return creditStatsFileName(variant).replace(/\.csv$/, ".xlsx");
}
