import { useMemo, useState } from "react";
import { Button } from "@/components/ui/button";
import { statsRangeLabel, type StatsRangeKey } from "@/lib/stats-range";
import type {
  TokenStatsSource,
} from "@/lib/types";

import { DistributionKey, modelTotalsInRange, tokenTotal } from "./data";
import { SectionTitle } from "./metric";
import { Ranking } from "./ranking";

/**
 * 用量分布（按项目 / 按模型）。
 *
 * 范围口径：模型维度有逐日序列（`dailyByModel`），能按全局范围切片；项目维度后端
 * 只给全量聚合，切片会被算成「该范围 0 Token」这种明显的假数字，因此如实标注
 * 「全量」而不是假装支持范围。
 */
export function Distribution({
  source,
  range,
  heading = true,
}: {
  source: TokenStatsSource;
  range: StatsRangeKey;
  /** 折叠区里由 `AccordionTrigger` 承担标题，卡内不再重复渲染区块标题。 */
  heading?: boolean;
}) {
  const [distribution, setDistribution] = useState<DistributionKey>("projects");
  const inRange = distribution === "models";
  const modelRows = useMemo(() => modelTotalsInRange(source, range), [source, range]);
  const groups = inRange ? modelRows : source.projects;
  // 分母与「分子来自哪份数据」保持一致：模型行=范围合计，项目行=全量合计。
  const denominator = inRange
    ? groups.reduce((sum, group) => sum + tokenTotal(group), 0)
    : tokenTotal(source.summary);
  const modelsSliceable = Boolean(source.dailyByModel);

  return (
    <section
      className="min-w-0 space-y-2.5"
      aria-labelledby={heading ? "token-distribution-title" : undefined}
    >
      {heading ? <SectionTitle id="token-distribution-title">用量分布</SectionTitle> : null}
      <Ranking
        groups={groups}
        denominator={denominator}
        description={
          distribution === "projects"
            ? "按项目汇总本地 Token 用量（项目维度只有全量聚合，不受统计范围影响）。"
            : modelsSliceable
              ? `按模型汇总 ${statsRangeLabel(range)} 的 Token 用量。`
              : "按模型汇总本地 Token 用量（当前后端未提供逐模型日序列，按全量统计）。"
        }
        controls={
          <div className="flex rounded-lg bg-muted p-1" role="group" aria-label="用量分布维度">
            <Button
              type="button"
              variant="ghost"
              size="sm"
              className={`h-7 px-2.5 text-xs ${
                distribution === "projects"
                  ? "bg-background font-medium text-foreground shadow-sm hover:bg-background"
                  : "text-muted-foreground"
              }`}
              aria-pressed={distribution === "projects"}
              onClick={() => setDistribution("projects")}
            >
              按项目
            </Button>
            <Button
              type="button"
              variant="ghost"
              size="sm"
              className={`h-7 px-2.5 text-xs ${
                distribution === "models"
                  ? "bg-background font-medium text-foreground shadow-sm hover:bg-background"
                  : "text-muted-foreground"
              }`}
              aria-pressed={distribution === "models"}
              onClick={() => setDistribution("models")}
            >
              按模型
            </Button>
          </div>
        }
      />
    </section>
  );
}
