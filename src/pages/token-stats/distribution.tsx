import { useState } from "react";
import { Button } from "@/components/ui/button";
import type {
  TokenStatsSource,
} from "@/lib/types";

import { DistributionKey, tokenTotal } from "./data";
import { SectionTitle } from "./metric";
import { Ranking } from "./ranking";

export function Distribution({ source }: { source: TokenStatsSource }) {
  const [distribution, setDistribution] = useState<DistributionKey>("projects");
  const groups = source[distribution];

  return (
    <section className="min-w-0 space-y-2.5" aria-labelledby="token-distribution-title">
      <SectionTitle id="token-distribution-title">用量分布</SectionTitle>
      <Ranking
        groups={groups}
        denominator={tokenTotal(source.summary)}
        description={
          distribution === "projects"
            ? "按项目汇总本地 Token 用量。"
            : "按模型汇总本地 Token 用量。"
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
