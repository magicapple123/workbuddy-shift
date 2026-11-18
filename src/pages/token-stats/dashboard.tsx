import { CircleAlert } from "lucide-react";
import type {
  TokenStatsSource,
} from "@/lib/types";

import { exact, tokenTotal } from "./data";
import { Distribution } from "./distribution";
import { Heatmap } from "./heatmap";
import { Overview } from "./overview";
import { SessionRanking } from "./ranking";
import { TrendChart } from "./trend-chart";

export function Dashboard({ source }: { source: TokenStatsSource }) {
  const denominator = tokenTotal(source.summary);

  if (source.summary.records === 0) {
    return (
      <div className="rounded-xl border border-dashed px-4 py-16 text-center text-sm text-muted-foreground">
        <div>
          {source.filesScanned > 0
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

  return (
    <div className="min-w-0 space-y-12">
      <Overview source={source} />
      <TrendChart source={source} />
      <Heatmap groups={source.daily} />
      <Distribution source={source} />
      <SessionRanking groups={source.sessions} denominator={denominator} />
      {source.parseErrors > 0 && (
        <p className="flex items-center gap-1.5 px-1 text-xs text-warning">
          <CircleAlert className="size-3.5" aria-hidden="true" />
          已跳过 {exact.format(source.parseErrors)} 条无法解析的本地记录。
        </p>
      )}
    </div>
  );
}
