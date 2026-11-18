import type { TokenStatsSource } from "@/lib/types";
import { buildCsv, csvFileName, type CsvSection } from "@/lib/csv";

/** Token 统计导出：区块与页面一一对应，范围是当前数据源、全量日期。 */
export function buildTokenStatsCsv(source: TokenStatsSource): string {
  const summary = source.summary;
  const sections: CsvSection[] = [
    {
      title: `Token 统计 · ${source.source}`,
      note: `文件 ${source.filesScanned} 个 · 解析错误 ${source.parseErrors}`,
      headers: ["指标", "数值"],
      rows: [
        ["总 Token", summary.total],
        ["输入", summary.input],
        ["输出", summary.output],
        ["缓存读", summary.cacheRead],
        ["缓存写", summary.cacheWrite],
        ["未命中缓存输入", summary.uncachedInput],
        ["请求数", summary.records],
        ["缓存命中率", summary.cacheHitRate === null ? "" : `${(summary.cacheHitRate * 100).toFixed(1)}%`],
      ],
    },
    groupSection("按日", source.daily),
    groupSection("按模型", source.models),
    groupSection("按项目", source.projects),
    groupSection("按会话", source.sessions),
    groupSection("按小时", source.hours),
  ];

  if (source.source === "codebuddy-cli" && (source.requests?.length ?? 0) > 0) {
    sections.push({
      title: "请求明细",
      note: `共 ${source.requests?.length ?? 0} 条`,
      headers: ["时间", "模型", "项目", "会话", "标题", "输入", "输出", "缓存读", "缓存写", "思考", "总 Token"],
      rows: (source.requests ?? []).map((row) => [
        new Date(row.timestamp).toISOString(),
        row.model,
        row.project,
        row.sessionId,
        row.title ?? "",
        row.input,
        row.output,
        row.cacheRead,
        row.cacheWrite,
        row.thinking,
        row.total,
      ]),
    });
  } else {
    sections.push({
      title: "请求明细",
      note: "当前数据源不提供逐条请求明细（仅 codebuddy-cli 支持），区块为空。",
      headers: ["说明"],
      rows: [],
    });
  }
  return buildCsv(sections);
}

function groupSection(title: string, groups: TokenStatsSource["daily"]): CsvSection {
  return {
    title,
    headers: ["键", "名称", "总 Token", "输入", "输出", "缓存读", "缓存写", "请求数"],
    rows: groups.map((group) => [
      group.key,
      group.title ?? group.project ?? "",
      group.total,
      group.input,
      group.output,
      group.cacheRead,
      group.cacheWrite,
      group.records,
    ]),
  };
}

export function tokenStatsFileName(source: TokenStatsSource["source"]): string {
  return csvFileName("token-stats", source);
}

