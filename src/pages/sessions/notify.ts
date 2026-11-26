import { toast } from "sonner";

import type { SessionGroupClient, WbVariant } from "@/lib/types";
import { variantLabel } from "@/lib/variant";

/**
 * 会话同步结果 → 提示文案的纯函数。
 *
 * 从 `hooks/use-session-actions` 里抽出来的唯一原因是**可测**：原来这段判定内联在 hook 末尾、
 * 直接调用 sonner 的 `toast.*`，要断言「跨组成功说的是副本还是会话」「覆盖组数按实际条目去重」
 * 就只能挂载 React + mock 弹窗容器。现在判定与文案产出是纯的（`buildSyncToasts`），
 * 只有最后一步发送（`emitSyncToasts`）碰外部系统。
 *
 * 文案、判定顺序、tone 与抽取前**逐字一致**：这是用户可见的既有行为，本次重构不改。
 */

export type SyncToastTone = "success" | "warning" | "error" | "info";

/** 一条提示：`description` 缺省表示该提示只有标题（当前只有 info 空态如此）。 */
export interface SyncToast {
  tone: SyncToastTone;
  title: string;
  description?: string;
}

/**
 * 同步类报告的**公共子集**：单组同步、插件侧统一、批量安全快进与跨组批量同步共用一份口径。
 *
 * 字段与后端报告同构且全部可选，因此各个具体报告（`SessionGroupActionReport` /
 * `SessionGroupAllSafeReport` / 局部汇总对象）都能直接传进来，不需要先转换。
 */
export interface SyncToastReport {
  /** 会话条目：跨组汇总时每条都带 `groupId`，用于统计「实际覆盖了多少组」。 */
  synced?: { groupId?: string }[];
  skipped?: unknown[];
  errors?: { error: string }[];
  needsRecovery?: boolean;
  temporaryFiles?: { reason: string }[];
  restartedVariants?: WbVariant[];
  restartedEditor?: boolean;
  editorError?: string;
  /** 报告所属客户端：重开提示要指名真正的宿主（JetBrains 不是 VS Code）。 */
  client?: SessionGroupClient;
  /** 跨组批量同步（issue #231）：本次涉及的组。有它时成功提示改说「副本 / 组」。 */
  groups?: { groupId: string; title: string; targetCount: number }[];
  /** 跨组批量同步：整组跳过的原因（最多展示 2 条，其余只报数量）。 */
  skippedGroups?: { groupId: string; title: string; reasonCode: string; message: string }[];
}

/**
 * 把一次同步结果翻译成按展示顺序排列的提示列表。
 *
 * 顺序固定为：成功 → 单条跳过 → 整组跳过 → 失败 → 待恢复 → 宿主重开失败 → 空态。
 * 空态只在「什么都没发生」时出现（成功/跳过/失败/待恢复都没有），否则会盖住真实结论。
 */
export function buildSyncToasts(report: SyncToastReport, targetName?: string): SyncToast[] {
  const toasts: SyncToast[] = [];
  const synced = report.synced?.length ?? 0;
  const skipped = report.skipped?.length ?? 0;
  const errors = report.errors ?? [];
  const groups = report.groups ?? [];
  const skippedGroups = report.skippedGroups ?? [];
  const editorLabel = report.client === "jetbrains" ? "JetBrains IDE" : "VS Code";
  const restartHint = report.restartedVariants?.length
    ? `，已重新打开 ${report.restartedVariants.map(variantLabel).join("、")}`
    : report.restartedEditor ? `，已重新打开 ${editorLabel}` : "";
  if (synced > 0) {
    const isBatch = groups.length > 0;
    // 覆盖组数按**实际同步成功的条目**去重统计：写盘途中出错会 `break`，后面未执行的组
    // 不该被算成「已覆盖」（`groups` 是准备阶段的清单，不等于真正写过的组）。
    const coveredGroups = new Set(
      (report.synced ?? []).flatMap((item) => (item.groupId ? [item.groupId] : [])),
    ).size || groups.length;
    const headline = targetName
      ? `已同步到「${targetName}」`
      : isBatch ? `已同步 ${synced} 个副本` : `已同步 ${synced} 个会话`;
    const detail = isBatch ? `覆盖 ${coveredGroups} 个组${restartHint}` : `完成 ${synced} 项${restartHint}`;
    toasts.push({ tone: "success", title: headline, description: detail });
  }
  if (skipped > 0) {
    toasts.push({
      tone: "warning",
      title: `有 ${skipped} 项跳过`,
      description: "预览过期或复核后不再符合安全条件的项不会计为成功。",
    });
  }
  if (skippedGroups.length > 0) {
    // 整组跳过：前两条给具体原因，其余只报数量（toast 里塞不下一屏）。
    const reasons = skippedGroups.slice(0, 2).map((item) => `${item.title || item.groupId}：${item.message}`);
    const rest = skippedGroups.length > 2 ? `；另有 ${skippedGroups.length - 2} 个组跳过` : "";
    toasts.push({
      tone: "warning",
      title: `跳过 ${skippedGroups.length} 个组`,
      description: `${reasons.join("；")}${rest}`,
    });
  }
  if (errors.length > 0) {
    toasts.push({
      tone: "error",
      title: "部分会话同步失败",
      description: errors.map((item) => item.error).join("；"),
    });
  }
  if (report.needsRecovery) {
    toasts.push({
      tone: "error",
      title: "会话操作待恢复",
      description: report.temporaryFiles?.map((item) => item.reason).join("；") || "已保留恢复所需材料。",
    });
  }
  if (report.editorError) {
    toasts.push({
      tone: "warning",
      title: `${editorLabel} 未能自动重新打开`,
      description: report.editorError,
    });
  }
  if (synced === 0 && skipped === 0 && skippedGroups.length === 0 && errors.length === 0 && !report.needsRecovery) {
    toasts.push({ tone: "info", title: "当前没有需要同步的副本" });
  }
  return toasts;
}

/**
 * 发送提示：与抽取前的行为一致（逐条 `toast[tone]`，有 `description` 才带选项对象）。
 *
 * 这是本模块唯一触碰外部系统的地方，hook 只需要调它一次。
 */
export function emitSyncToasts(report: SyncToastReport, targetName?: string): void {
  for (const item of buildSyncToasts(report, targetName)) {
    toast[item.tone](item.title, item.description ? { description: item.description } : undefined);
  }
}
