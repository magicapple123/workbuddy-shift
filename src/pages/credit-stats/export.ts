import type { CreditOfficialUsage, CreditStatistics } from "@/lib/types";
import { buildCsv, csvFileName, type CsvSection } from "@/lib/csv";

/** 积分统计导出：聚合 + 官方请求明细（标注每账号上限与是否截断）。 */
export function buildCreditStatsCsv(
  stats: CreditStatistics,
  official: CreditOfficialUsage | undefined,
): string {
  const summary = stats.summary;
  const sections: CsvSection[] = [
    {
      title: "积分统计 · 总览",
      note: `生成于 ${new Date(stats.generatedAt).toLocaleString("zh-CN")}`,
      headers: ["指标", "数值"],
      rows: [
        ["当前剩余", summary.currentRemaining],
        ["当前容量", summary.currentCapacity],
        ["今日消耗", summary.usageToday],
        ["近 7 天消耗", summary.usage7Days],
        ["本月消耗", summary.usageThisMonth],
        ["今日已签到账号", summary.todayCheckedInAccounts],
        ["今日签到成功", summary.todaySuccess],
      ],
    },
    {
      title: "按账号",
      headers: ["账号", "当前剩余", "容量", "今日消耗", "近 7 天", "本月", "档位"],
      rows: stats.accounts.map((account) => [
        account.accountName || account.accountId,
        account.currentRemaining ?? "",
        account.totalCapacity ?? "",
        account.usageToday,
        account.usage7Days,
        account.usageThisMonth,
        account.variant ?? "",
      ]),
    },
    {
      title: "按日",
      headers: ["日期", "消耗"],
      rows: (stats.daily ?? []).map((row) => [row.date, row.usage]),
    },
    {
      title: "按模型",
      headers: ["模型", "请求数", "消耗"],
      rows: (official?.models ?? []).map((row) => [row.model, row.requestCount, row.credit]),
    },
  ];

  const detailLimit = official?.detailLimitPerAccount ?? 1000;
  const requests = official?.requests ?? [];
  const truncated = (official?.accounts ?? []).some((account) => account.detailTruncated);
  sections.push({
    title: "官方请求明细",
    note: `每账号上限 ${detailLimit} 条${truncated ? "（部分账号已截断）" : "（未截断）"} · 共 ${requests.length} 条`,
    headers: ["请求时间", "账号", "模型", "积分", "客户端", "请求 id"],
    rows: requests.map((row) => [
      row.requestTime,
      row.accountName,
      row.model,
      row.credit,
      row.client,
      row.requestId,
    ]),
  });

  sections.push({
    title: "本地事件",
    headers: ["时间", "账号", "类型", "数值", "备注"],
    rows: (stats.events ?? []).map((event) => [
      new Date(event.ts).toISOString(),
      event.accountName,
      event.kind,
      event.kind === "usage" ? event.amount : event.result,
      event.kind === "usage" ? event.date : event.error ?? "",
    ]),
  });

  return buildCsv(sections);
}

export function creditStatsFileName(variant: string): string {
  return csvFileName("credit-stats", variant);
}
