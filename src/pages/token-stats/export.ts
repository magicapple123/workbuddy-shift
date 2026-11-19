import type { TokenStatsGroup, TokenStatsSource } from "@/lib/types";
import { buildCsv, csvFileName, type CsvSection } from "@/lib/csv";
import { buildXlsx, type XlsxCell, type XlsxSheet } from "@/lib/xlsx";

import { formatRequestTime } from "./data";

/**
 * Token 统计导出（清单第 17 条）。
 *
 * 五类数据**不再共用一套表头**：旧实现的 `键 | 名称 | …` 让模型/项目/日/小时的「名称」
 * 永远为空，WPS 里看起来像整体右移。现在每个区块只带自己的列：
 *
 * | 区块 | 表头 |
 * | --- | --- |
 * | 汇总 | 指标 · 数值 |
 * | 按日 | 日期 · 总量… |
 * | 按模型 | 模型 · 总量… |
 * | 按项目 | 项目 · 总量… |
 * | 按会话 | 项目 · 会话 · 标题 · 账号 · 总量… |
 * | 按小时 | 小时 · 总量… |
 * | 请求明细 | 时间 · 模型 · 项目 · 会话 · 标题 · 账号 · 输入 … |
 *
 * CSV 与 xlsx 共用同一份中间结构（`ExportSection`），保证两种格式逐段数字一致。
 */

/** 数值列（CSV 里保持纯数字，可求和；xlsx 里是数值类型）。 */
const TOTALS_HEADERS = ["总 Token", "输入", "输出", "缓存读", "缓存写", "请求数"];

interface ExportSection {
  /** 区块名 = xlsx 的 sheet 名；CSV 用它做段标题（加 `# ` 前缀）。 */
  name: string;
  header: string[];
  rows: XlsxCell[][];
}

function totalsCells(group: TokenStatsGroup): XlsxCell[] {
  return [
    group.total,
    group.input,
    group.output,
    group.cacheRead,
    group.cacheWrite,
    group.records,
  ];
}

/** 逐日/逐模型/逐项目/逐小时共用的「键 + 数值」区块。 */
function keyedSection(name: string, keyLabel: string, groups: TokenStatsGroup[]): ExportSection {
  return {
    name,
    header: [keyLabel, ...TOTALS_HEADERS],
    rows: groups.map((group) => [group.key, ...totalsCells(group)]),
  };
}

/** 账号列：空 uid 是「未归属」，其余显示 uid 后 6 位（与页面行标签一致）。 */
function accountCell(uid?: string): string {
  if (uid === undefined) return "";
  if (!uid) return "未归属";
  return `账号 …${uid.slice(-6)}`;
}

function summarySection(source: TokenStatsSource): ExportSection {
  const summary = source.summary;
  const rows: XlsxCell[][] = [
    ["总 Token", summary.total],
    ["输入", summary.input],
    ["输出", summary.output],
    ["缓存读", summary.cacheRead],
    ["缓存写", summary.cacheWrite],
    ["未命中缓存输入", summary.uncachedInput],
    ["请求数", summary.records],
    [
      "缓存命中率",
      summary.cacheHitRate === null ? "" : `${(summary.cacheHitRate * 100).toFixed(1)}%`,
    ],
    ["会话文件数", source.filesScanned],
    ["解析错误数", source.parseErrors],
  ];
  if (source.coverageStartAt) {
    rows.push(["覆盖起算", formatRequestTime(source.coverageStartAt)]);
  }
  if (source.coverageEndAt) {
    rows.push(["覆盖终点", formatRequestTime(source.coverageEndAt)]);
  }
  return { name: "汇总", header: ["指标", "数值"], rows };
}

function sessionSection(source: TokenStatsSource): ExportSection {
  return {
    name: "按会话",
    header: ["项目 · 会话", "标题", "账号", ...TOTALS_HEADERS],
    rows: source.sessions.map((group) => [
      group.key,
      group.title ?? "",
      accountCell(group.uid),
      ...totalsCells(group),
    ]),
  };
}

function requestSection(source: TokenStatsSource): ExportSection {
  const rows = source.requests ?? [];
  if (source.source !== "codebuddy-cli" || rows.length === 0) {
    // 只有 CLI 提供逐请求明细；其它来源明说，避免用户以为数据丢了。
    return {
      name: "请求明细",
      header: ["说明"],
      rows: [["当前数据源不提供逐条请求明细（仅 CodeBuddy CLI 支持）"]],
    };
  }
  return {
    name: "请求明细",
    header: [
      "时间",
      "模型",
      "项目",
      "会话",
      "标题",
      "账号",
      "输入",
      "输出",
      "缓存读",
      "缓存写",
      "思考",
      "总 Token",
    ],
    rows: rows.map((row) => [
      formatRequestTime(row.timestamp),
      row.model,
      row.project,
      row.sessionId,
      row.title ?? "",
      accountCell(row.uid),
      row.input,
      row.output,
      row.cacheRead,
      row.cacheWrite,
      row.thinking,
      row.total,
    ]),
  };
}

/** 一次导出用到的全部区块（CSV 与 xlsx 共用）。 */
export function tokenStatsSections(source: TokenStatsSource): ExportSection[] {
  return [
    summarySection(source),
    keyedSection("按日", "日期", source.daily),
    keyedSection("按模型", "模型", source.models),
    keyedSection("按项目", "项目", source.projects),
    sessionSection(source),
    keyedSection("按小时", "小时", source.hours),
    requestSection(source),
  ];
}

/** CSV：每段自己的表头；数值保持纯数字（不做强制文本）；段标题加 `# ` 前缀。 */
export function buildTokenStatsCsv(source: TokenStatsSource): string {
  const sections: CsvSection[] = tokenStatsSections(source).map((section) => ({
    title: `# ${section.name}`,
    headers: section.header,
    rows: section.rows.map((row) => row.map((cell) => (cell === null ? "" : cell))),
  }));
  return buildCsv(sections);
}

/** xlsx：每个区块一个工作表，首行冻结 + 表头样式 + 数值类型。 */
export function tokenStatsSheets(source: TokenStatsSource): XlsxSheet[] {
  return tokenStatsSections(source).map((section) => ({
    name: section.name,
    header: section.header,
    rows: section.rows,
  }));
}

/** 生成 xlsx 字节（前端生成，走 Blob 下载）。 */
export function buildTokenStatsXlsx(source: TokenStatsSource): Uint8Array {
  return buildXlsx(tokenStatsSheets(source));
}

/** 导出作用域：数据源 + 账号筛选（账号名会被清洗成安全文件名片段）。 */
function exportScope(source: TokenStatsSource["source"], accountLabel?: string): string {
  return accountLabel ? `${source}-${accountLabel}` : source;
}

export function tokenStatsFileName(
  source: TokenStatsSource["source"],
  accountLabel?: string,
): string {
  return csvFileName("token-stats", exportScope(source, accountLabel));
}

export function tokenStatsXlsxFileName(
  source: TokenStatsSource["source"],
  accountLabel?: string,
): string {
  return tokenStatsFileName(source, accountLabel).replace(/\.csv$/, ".xlsx");
}
