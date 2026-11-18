import type { SessionGroupClient, SessionGroupPruneReport } from "@/lib/types";

/**
 * 「清理失效关联组」的纯逻辑：入口可见性与确认框文案。
 *
 * 与后端语义逐字对齐：只删除关联关系，不删除任何账号内的会话内容；清理前会先备份
 * 关联记录（`session_links.json.bak-<时间戳>`）。抽成纯函数是因为这些判断要单测覆盖，
 * 而页面本身依赖 DOM 与 store。
 */

/**
 * 批量清理在三个客户端都提供（WorkBuddy / VS Code 插件 / CodeBuddy IDE）。
 *
 * 三者的「会话行」判定依据不同（`sessions` 表 vs 工作区索引），但都在后端按客户端分派：
 * 索引可读时正常统计与删除，读不到时后端会**拒绝整次清理**并把「哪个账号、为什么判不了」
 * 放进错误文案（页面原样透传，见 `pruneFailureDescription`）。
 */
export function supportsPrune(client: SessionGroupClient): boolean {
  return client === "workbuddy" || client === "vscodeExt" || client === "codebuddyIde";
}

/**
 * 入口是否**展示**：客户端支持、并且确实有可清理的对象。
 *
 * 与「能不能点」（[`canPruneGroups`]）刻意分开：被阻塞或正在忙时入口仍然要在，只是禁用并
 * 说明原因——把按钮整片隐藏会让用户以为「没有失效组」，而实际上有、只是现在清不了。
 */
export function showPruneEntry(args: {
  client: SessionGroupClient;
  groupCount: number;
  deadGroupCount: number;
}): boolean {
  return supportsPrune(args.client) && (args.groupCount > 0 || args.deadGroupCount > 0);
}

/**
 * 入口是否可用：客户端支持、并且确实有可清理的对象（列表里的组或已被隐藏的失效组）、
 * 且当前没有动作在跑、且清理没有被后端阻塞。
 *
 * `deadGroupCount > 0` 必须算进来：失效组已被列表过滤掉，全部失效时列表为空，
 * 若只看 `groupCount` 就再也点不到清理入口——正是最需要清理的时候。
 *
 * `blockedReason` 是后端给的「点了必然整次失败」的原因（某个账号的索引读不全等）：
 * 非空时**必须禁用**入口并展示原因，不能让用户对着一个必然失败的按钮点一次。
 */
export function canPruneGroups(args: {
  client: SessionGroupClient;
  groupCount: number;
  deadGroupCount: number;
  busy: boolean;
  blockedReason?: string | null;
}): boolean {
  return (
    showPruneEntry(args) &&
    !args.busy &&
    blockedPruneReason(args.blockedReason) === null
  );
}

/**
 * 被阻塞的原因（`null` = 可清理）。
 *
 * 空串 / 纯空白按「没有原因」处理：不能因为后端给了个空字符串就禁用入口。
 */
export function blockedPruneReason(reason: string | null | undefined): string | null {
  const text = (reason ?? "").trim();
  return text.length > 0 ? text : null;
}

/**
 * 二次确认文案：写明「可清理 N 组」与「只删关联、不删会话内容」。
 *
 * 用 dry-run 的统计结果生成，用户看到的是后端真实判定过的数量，而不是猜测。
 */
export function pruneConfirmDescription(report: SessionGroupPruneReport): string {
  const groups = Math.max(report.removedGroups, 0);
  const members = Math.max(report.removedMembers, 0);
  return `可清理 ${groups} 组失效关联（涉及 ${members} 个账号副本）。只删除关联关系，不删除任何账号内的会话内容；清理前会先备份关联记录。`;
}

/**
 * 清理失败时的说明文案：后端拒绝原因必须**原样透传**。
 *
 * 拒绝清理是有信息量的结论（「无法判定 VS Code 插件账号 X 的会话，已取消清理」），把它替换成
 * 泛化的「清理失败」会让用户以为可以重试，而真实原因（数据目录/索引不可读）永远看不到。
 */
export function pruneFailureDescription(reason: string): string {
  const text = reason.trim();
  return text.length > 0
    ? text
    : "后端未给出具体原因，请确认客户端数据目录可读后重试。";
}

/** 发一次清理请求：`dryRun = true` 只统计，`false` 真删（后端写盘前先备份）。 */
export type PruneRequest = (dryRun: boolean) => Promise<SessionGroupPruneReport>;

/**
 * 第一步：`dryRun` 统计。返回 `null` 表示没有可清理的组（不弹确认框）。
 *
 * 拆成纯函数是为了能直接单测确认流程：数量为 0 时绝不能打开确认框，也不能顺手发
 * 第二次请求。
 */
export async function probePrune(
  request: PruneRequest,
): Promise<SessionGroupPruneReport | null> {
  const report = await request(true);
  return report.removedGroups > 0 ? report : null;
}

/**
 * 第一次点击「清理失效关联组」的完整判定（纯函数，可直接单测）。
 *
 * `blocked` 时**一次请求都不发**：后端会用同一条原因整次拒绝，先请求只会让用户看到
 * 一个「检查失败」的错误提示，而真正的原因（数据目录/索引读不全）本该在入口旁就能看到。
 */
export type PruneAttempt =
  | { status: "blocked"; reason: string }
  | { status: "empty" }
  | { status: "ready"; report: SessionGroupPruneReport };

export async function attemptPrune(args: {
  blockedReason: string | null | undefined;
  request: PruneRequest;
}): Promise<PruneAttempt> {
  const reason = blockedPruneReason(args.blockedReason);
  if (reason) return { status: "blocked", reason };
  const report = await probePrune(args.request);
  return report ? { status: "ready", report } : { status: "empty" };
}

/** 一次真删的结果：清理本身的结果与「刷新列表」的结果分开回报。 */
export interface PruneOutcome {
  report: SessionGroupPruneReport;
  /** 刷新列表失败的错误；`null` 表示刷新也成功。 */
  reloadError: unknown | null;
}

/**
 * 第二步：真删（`dryRun: false`）后刷新列表。
 *
 * 刷新**不放在**真删的 try 里，也不向外抛：清理已经落盘，若把刷新失败当成清理失败，
 * 用户会在「已清理 N 组」之后立刻看到一条「清理失败」，既误导又可能让他重复清理。
 * 刷新失败只通过 `reloadError` 回报，由调用方补一条独立提示。
 */
export async function runPrune(
  request: PruneRequest,
  reload: () => Promise<void>,
): Promise<PruneOutcome> {
  const report = await request(false);
  try {
    await reload();
    return { report, reloadError: null };
  } catch (reloadError) {
    return { report, reloadError };
  }
}
