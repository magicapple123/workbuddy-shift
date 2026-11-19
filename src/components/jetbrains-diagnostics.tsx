import { AlertTriangle, CheckCircle2, FolderSearch, HelpCircle, XCircle } from "lucide-react";

import { Badge } from "@/components/ui/badge";
import type { JetbrainsStatus } from "@/lib/types";

interface Props {
  /** JetBrains 状态（提供 `diagnostics`）；缺失时只显示「无法读取状态」结论，不渲染明细。 */
  status?: JetbrainsStatus | null;
  /** 折叠面板默认是否展开（没有可切换目标时建议展开）。 */
  defaultOpen?: boolean;
}

/** 状态结论的四种口径：切换能不能用、不能用是谁的问题。 */
export type JetbrainsStateKind = "ready" | "idle" | "blocked" | "missing" | "no-plugin";

export interface JetbrainsStateSummary {
  kind: JetbrainsStateKind;
  /** 一行结论（不含任何路径，可直接展示给普通用户）。 */
  title: string;
  /** 下一步该做什么；`ready` 时是切换行为的说明。 */
  hint: string;
}

/**
 * 把后端字段归约成四种用户能理解的状态（清单第 16 条验收口径）：
 *
 * 1. `missing`   —— 没装 IDE（配置根不存在 / 下面没有任何子目录）；
 * 2. `no-plugin` —— 装了 IDE，但没装 CodeBuddy 插件（或目录名不被识别）；
 * 3. `blocked`   —— IDE 在跑，但映射不到配置目录（版本形态不认识）→ 切换会被拒绝；
 * 4. `ready`     —— 可以切换；`idle` 是同一状态下的「IDE 没运行」子情形。
 *
 * 纯函数：不读 DOM、不发起请求，便于单测钉住判定顺序。
 */
export function classifyJetbrainsState(status?: JetbrainsStatus | null): JetbrainsStateSummary {
  if (!status) {
    return { kind: "missing", title: "无法读取 JetBrains 状态", hint: "请重试；仍失败可查看诊断信息。" };
  }
  const diagnostics = status.diagnostics ?? null;
  const scanned = diagnostics?.scannedDirs ?? [];
  const rejected = scanned.filter((dir) => !dir.accepted);
  const unmapped = status.unmappedRunning ?? [];

  // 运行中却映射不到配置目录：写入会被 IDE 内存态覆盖，后端会拒绝切换。
  if (unmapped.length > 0) {
    return {
      kind: "blocked",
      title: `${unmapped.length} 个运行中的 IDE 无法识别版本`,
      hint: "请完全退出这些 IDE 后重新打开本弹窗；识别不了版本时不写入，避免「切了没变」。",
    };
  }

  const rootExists = diagnostics ? diagnostics.configRootExists : status.installed;
  // 子目录清单是 `diagnostics` 才有的信息：没有它时只能按 `installed` 判定，
  // 否则「旧后端 / 演示数据」会被误报成「未安装」。
  const noDirs = diagnostics ? scanned.length === 0 : !status.installed;
  if (!rootExists || noDirs) {
    return {
      kind: "missing",
      title: "未检测到 JetBrains IDE",
      hint: "本机没有 IDEA / PyCharm 的用户配置目录：请先安装并至少启动一次。",
    };
  }

  if (scanned.length > 0 && scanned.every((dir) => !dir.accepted)) {
    return {
      kind: "no-plugin",
      title: `已安装 ${scanned.length} 个 JetBrains 目录，但都不是受支持的产品名`,
      hint: `请把下方诊断信息反馈给维护者（目录名：${rejected
        .map((dir) => dir.name)
        .join("、")}）。`,
    };
  }

  if (!status.pluginInstalled) {
    return {
      kind: "no-plugin",
      title: "已安装 JetBrains IDE，但未检测到 CodeBuddy 插件",
      hint: "请在 IDEA / PyCharm 的插件市场安装「Tencent Cloud CodeBuddy」后重试。",
    };
  }

  if (status.running) {
    return {
      kind: "ready",
      title: "可切换（IDE 正在运行）",
      hint: "切换时会先保存并退出 IDEA / PyCharm，写入后自动重新打开。",
    };
  }

  return {
    kind: "idle",
    title: "可切换（IDE 未运行）",
    hint: "IDE 未运行，写入后下次打开 IDE 生效。",
  };
}

const STATE_ICON: Record<JetbrainsStateKind, typeof CheckCircle2> = {
  ready: CheckCircle2,
  idle: CheckCircle2,
  blocked: AlertTriangle,
  missing: XCircle,
  "no-plugin": HelpCircle,
};

/**
 * JetBrains 诊断面板：把「识别不到」变成用户能自行判断的信息。
 *
 * 顶部一行是**始终可见**的状态结论（不含路径）；配置根、子目录名、插件目录名、
 * 进程 PID 与映射结果都在折叠区里，只在需要排查时展开。
 * 不展示任何 secret 值——`secret-storage.xml` 只报「文件是否存在」。
 */
export function JetbrainsDiagnosticsPanel({ status, defaultOpen = false }: Props) {
  const diagnostics = status?.diagnostics ?? null;
  const summary = classifyJetbrainsState(status);
  const Icon = STATE_ICON[summary.kind];
  const scanned = diagnostics?.scannedDirs ?? [];
  const running = diagnostics?.running ?? [];
  /** 「可切换」时目标列表已经说明了状态，只有异常态才额外顶一行结论（避免重复）。 */
  const showSummary = defaultOpen || summary.kind !== "ready";

  return (
    <div className="space-y-1.5">
      {showSummary && (
        <div
          className="flex items-start gap-1.5 text-xs leading-5"
          data-state={summary.kind}
          aria-live="polite"
        >
          <Icon className="mt-0.5 size-3.5 flex-none text-muted-foreground" aria-hidden />
          <div className="min-w-0">
            <p className="font-medium text-foreground">{summary.title}</p>
            <p className="text-muted-foreground">{summary.hint}</p>
          </div>
        </div>
      )}

      {diagnostics ? (
        <details
          className="rounded-lg border border-border/60 bg-muted/30 px-3 py-2 text-xs"
          open={defaultOpen}
        >
          <summary className="flex cursor-pointer items-center gap-1.5 font-medium text-muted-foreground">
            <FolderSearch className="size-3.5" aria-hidden />
            诊断信息（配置目录 / 进程映射）
          </summary>

          <div className="mt-2 space-y-2 leading-5">
            <div className="flex flex-wrap items-center gap-1.5">
              <span className="text-muted-foreground">配置根</span>
              <code className="break-all rounded bg-background/70 px-1.5 py-0.5">
                {diagnostics.configRoot ?? "（无法定位）"}
              </code>
              <Badge variant={diagnostics.configRootExists ? "secondary" : "warning"}>
                {diagnostics.configRootExists ? "存在" : "不存在"}
              </Badge>
            </div>

            <div className="space-y-1">
              <p className="text-muted-foreground">
                子目录 {scanned.length} 个（受支持 {diagnostics.acceptedDirs?.length ?? 0} 个，已装插件{" "}
                {diagnostics.pluginInstalledDirs?.length ?? 0} 个）
              </p>
              {scanned.length === 0 ? (
                <p className="text-muted-foreground">配置根下没有任何子目录。</p>
              ) : (
                <ul className="space-y-1">
                  {scanned.map((dir) => (
                    <li key={dir.name} className="flex flex-wrap items-center gap-1.5">
                      <code className="break-all rounded bg-background/70 px-1.5 py-0.5">
                        {dir.name}
                      </code>
                      <Badge variant={dir.accepted ? "success" : "outline"}>
                        {dir.accepted ? "已识别" : "目录名不支持"}
                      </Badge>
                      <Badge variant={dir.pluginInstalled ? "success" : "outline"}>
                        {dir.pluginInstalled ? "插件已安装" : "未装插件"}
                      </Badge>
                      {dir.pluginDirs.length > 0 && (
                        <span className="break-all text-muted-foreground">
                          plugins: {dir.pluginDirs.join("、")}
                        </span>
                      )}
                      {dir.hasSecretFile && (
                        <span className="text-muted-foreground">有登录文件</span>
                      )}
                    </li>
                  ))}
                </ul>
              )}
            </div>

            <div className="space-y-1">
              <p className="text-muted-foreground">运行中的 IDE 进程 {running.length} 个</p>
              {running.length === 0 ? (
                <p className="text-muted-foreground">没有检测到 IDEA / PyCharm 进程。</p>
              ) : (
                <ul className="space-y-1">
                  {running.map((proc) => (
                    <li key={`${proc.pid}-${proc.launchTarget ?? ""}`} className="space-y-0.5">
                      <div className="flex flex-wrap items-center gap-1.5">
                        <span className="font-medium">
                          {proc.product ?? "未知 IDE"}
                          {proc.version ? ` ${proc.version}` : ""}
                        </span>
                        <span className="text-muted-foreground">PID {proc.pid}</span>
                        <Badge variant={proc.mappingSource === "unmapped" ? "warning" : "secondary"}>
                          {mappingLabel(proc.mappingSource)}
                        </Badge>
                        {proc.configDir && (
                          <code className="rounded bg-background/70 px-1.5 py-0.5">
                            {proc.configDir}
                          </code>
                        )}
                      </div>
                      {proc.mappingNote && (
                        <p className="break-all text-muted-foreground">原因：{proc.mappingNote}</p>
                      )}
                      {proc.launchTarget && (
                        <p className="break-all text-muted-foreground">{proc.launchTarget}</p>
                      )}
                    </li>
                  ))}
                </ul>
              )}
            </div>
          </div>
        </details>
      ) : null}
    </div>
  );
}

function mappingLabel(source: "product-info" | "fallback" | "unmapped"): string {
  switch (source) {
    case "product-info":
      return "product-info 精确映射";
    case "fallback":
      return "按 exe / 目录名推断";
    default:
      return "无法映射";
  }
}
