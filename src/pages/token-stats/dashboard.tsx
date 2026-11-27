import { CircleAlert } from "lucide-react";
import {
  Accordion,
  AccordionContent,
  AccordionItem,
  AccordionTrigger,
} from "@/components/ui/accordion";
import { statsRangeLabel, type StatsRangeKey } from "@/lib/stats-range";
import type {
  TokenStatsSource,
} from "@/lib/types";

import {
  exact,
  formatTokenCompact,
  heatmapActiveDays,
  modelTotalsInRange,
  overviewTotals,
  percentage,
  tokenTotal,
  topGroupShare,
} from "./data";
import { Distribution } from "./distribution";
import { Heatmap } from "./heatmap";
import { Overview } from "./overview";
import { SessionRanking } from "./ranking";
import { TrendChart } from "./trend-chart";

/**
 * Token 统计的主体内容：**首屏三件套 + 折叠区**。
 *
 * 首屏顺序固定为「四指标 → 一行结论 → 趋势图」：用户先看到总量，再用一句话知道
 * 「钱花在哪个模型」，最后才是可交互的趋势。其余三块（活跃热力图 / 用量分布 /
 * 消耗最高的会话）收进手风琴，第一个（活跃热力图）默认展开，标题上都带数字结论，
 * 不展开也能读到信息。
 *
 * 范围由页面唯一控件传入（`range`），本组件不持有范围状态。
 */
export function Dashboard({
  source,
  scopeLabel = "所有账号",
  range,
}: {
  source: TokenStatsSource;
  /** 当前账号筛选的显示名（第 7 条）；「所有账号」时沿用全局聚合。 */
  scopeLabel?: string;
  /** 全局统计范围（今天 / 近 7 天 / 近 30 天 / 本月 / 近 90 天 / 全部）。 */
  range: StatsRangeKey;
}) {
  const denominator = tokenTotal(source.summary);

  if (source.summary.records === 0) {
    return (
      <div className="rounded-xl border border-dashed px-4 py-16 text-center text-sm text-muted-foreground">
        <div>
          {source.summary.records === 0 && scopeLabel !== "所有账号"
            ? `「${scopeLabel}」在该来源没有用量记录。`
            : source.filesScanned > 0
              ? `已扫描 ${exact.format(source.filesScanned)} 个会话文件，但没有可用的 usage。`
              : "尚未发现该来源的本地会话日志。"}
        </div>
        {source.source === "workbuddy-ai" && (
          <div className="mt-2 text-xs leading-5">
            国际版数据源为空：本机可能未安装 WorkBuddy 国际版客户端，或尚未产生本地会话日志；
            国际版数据与国内版分开统计，不参与国内版用量。
          </div>
        )}
        {source.parseErrors > 0 && (
          <div className="mt-2 text-xs text-warning">
            已跳过 {exact.format(source.parseErrors)} 条无法解析的本地记录。
          </div>
        )}
      </div>
    );
  }

  const rangeText = statsRangeLabel(range);
  const totals = overviewTotals(source, range);
  const rangeTotal = tokenTotal(totals);
  // 模型维度可按范围切片（逐模型日序列），是「头部占比」结论的唯一同源数据。
  const modelRows = modelTotalsInRange(source, range);
  const topModel = topGroupShare(modelRows);
  const activeDays = heatmapActiveDays(source.daily);
  const topSession = topGroupShare(source.sessions);

  const modelShareText = topModel ? percentage(topModel.amount, topModel.total) : "—";
  const distributionTitle =
    topModel && modelRows.length > 0
      ? `用量分布 · ${modelRows.length} 个模型 · 头部 ${topModel.key} 占 ${modelShareText}`
      : "用量分布 · 当前范围没有模型消耗";
  const heatmapTitle = `活跃热力图 · 最近一年 ${activeDays} 个活跃日`;
  const sessionTitle = topSession
    ? `消耗最高的会话 · 头部 1 个占 ${percentage(topSession.amount, topSession.total)}`
    : "消耗最高的会话 · 暂无会话用量";

  return (
    <div className="min-w-0 space-y-6">
      <Overview source={source} range={range} />

      {/* 结论先行：一句话说清「这段时间的 Token 花在哪个模型上」 */}
      <p className="min-w-0 px-1 text-sm leading-6 text-muted-foreground">
        {rangeTotal > 0 ? (
          <>
            {rangeText}共{" "}
            <span className="font-medium tabular-nums text-foreground">
              {formatTokenCompact(rangeTotal)} Token
            </span>
            （{exact.format(totals.records)} 次调用）
            {topModel ? (
              <>
                ，其中{" "}
                <span className="font-medium text-foreground">{topModel.key}</span> 占{" "}
                <span className="font-medium tabular-nums text-foreground">{modelShareText}</span>
                （{topModel.count} 个模型）
              </>
            ) : null}
          </>
        ) : (
          <>{rangeText}内没有观察到 Token 用量，试试把统计范围切到「全部」。</>
        )}
      </p>

      <TrendChart source={source} range={range} />

      {/* 次要从属折叠：第一个（活跃热力图）默认展开，其余收起 */}
      <Accordion type="multiple" defaultValue={["heatmap"]} className="min-w-0">
        <AccordionItem value="heatmap" className="min-w-0 border-t border-border/60">
          <AccordionTrigger className="px-1 py-2.5 text-[13px] font-medium leading-5 text-foreground">
            <span className="min-w-0 truncate text-left" title={heatmapTitle}>
              {heatmapTitle}
            </span>
          </AccordionTrigger>
          <AccordionContent className="pb-1 pt-1">
            <Heatmap groups={source.daily} heading={false} />
          </AccordionContent>
        </AccordionItem>

        <AccordionItem value="distribution" className="min-w-0 border-t border-border/60">
          <AccordionTrigger className="px-1 py-2.5 text-[13px] font-medium leading-5 text-foreground">
            <span className="min-w-0 truncate text-left" title={distributionTitle}>
              {distributionTitle}
            </span>
          </AccordionTrigger>
          <AccordionContent className="pb-1 pt-1">
            <Distribution source={source} range={range} heading={false} />
          </AccordionContent>
        </AccordionItem>

        <AccordionItem value="sessions" className="min-w-0 border-t border-border/60">
          <AccordionTrigger className="px-1 py-2.5 text-[13px] font-medium leading-5 text-foreground">
            <span className="min-w-0 truncate text-left" title={sessionTitle}>
              {sessionTitle}
            </span>
          </AccordionTrigger>
          <AccordionContent className="pb-1 pt-1">
            <SessionRanking groups={source.sessions} denominator={denominator} heading={false} />
          </AccordionContent>
        </AccordionItem>
      </Accordion>

      <div className="space-y-1 px-1">
        {source.parseErrors > 0 && (
          <p className="flex items-center gap-1.5 text-xs text-warning">
            <CircleAlert className="size-3.5" aria-hidden="true" />
            已跳过 {exact.format(source.parseErrors)} 条无法解析的本地记录。
          </p>
        )}
        {/* 口径说明（第 8 条 / 第四轮 06）：用户必须能回答「覆盖了哪些客户端、哪些没有」。 */}
        <p className="text-[11px] leading-5 text-muted-foreground">
          数据只来自本地日志（非官方账单）：日志轮转或删除会低估用量。逐请求明细仅 CodeBuddy CLI
          提供。三个插件来源各自只扫自己的数据目录（VS Code / JetBrains / CodeBuddy IDE），
          客户端内置扩展写的 WorkBuddy 目录不计入，避免与国内版重复计数。
        </p>
      </div>
    </div>
  );
}
