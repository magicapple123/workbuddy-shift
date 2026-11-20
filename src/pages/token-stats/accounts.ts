/**
 * Token 统计的账号维度（第 7 条）：账号下拉选项、客户端过滤与偏好持久化。
 *
 * 关键前提：后端一次扫描就产出 per-account 聚合（`source.accounts`），切账号**不再**
 * 重新扫盘、不发起新请求。这里全部是纯函数 + localStorage 偏好，便于单测。
 */

import { displayName } from "@/lib/account-display";
import { readPreference, writePreference } from "@/lib/persisted-preference";
import type {
  AccountMeta,
  TokenStatsAccount,
  TokenStatsSource,
  TokenStatsTotals,
} from "@/lib/types";

import type { SourceKey } from "./data";

/** 「未归属」在存储里的标记（uid 本身是空串，不能直接当键用）。 */
export const UNASSIGNED_MARKER = "unassigned";
/** 「所有账号」（默认）。 */
export const ALL_MARKER = "all";

export type AccountSelection =
  | { kind: "all" }
  | { kind: "unassigned" }
  | { kind: "uid"; uid: string };

export interface AccountOption {
  /** 存储值：`all` / `unassigned` / `uid:<uid>`。 */
  value: string;
  label: string;
  /** 该账号在当前来源里是否真的有用量（没用量也列出，显示 0）。 */
  hasUsage: boolean;
}

/** 零值合计：账号没有数据时用它，页面显示 0 而不是回骨架屏。 */
export function emptyTotals(): TokenStatsTotals {
  return {
    total: 0,
    input: 0,
    output: 0,
    cacheRead: 0,
    cacheWrite: 0,
    uncachedInput: 0,
    records: 0,
    cacheHitRate: null,
  };
}

/** 每个来源独立记忆：键里带来源。 */
export function accountSelectionStorageKey(source: SourceKey): string {
  return `workbuddy-shift:token-stats:account:${source}`;
}

export function serializeSelection(selection: AccountSelection): string {
  switch (selection.kind) {
    case "all":
      return ALL_MARKER;
    case "unassigned":
      return UNASSIGNED_MARKER;
    case "uid":
      return `uid:${selection.uid}`;
  }
}

export function parseSelection(raw: string): AccountSelection | null {
  if (raw === ALL_MARKER) return { kind: "all" };
  if (raw === UNASSIGNED_MARKER) return { kind: "unassigned" };
  if (raw.startsWith("uid:") && raw.length > 4) return { kind: "uid", uid: raw.slice(4) };
  return null;
}

/** 读取某个来源上次选择的账号；损坏/缺省回退「所有账号」。 */
export function readAccountSelection(source: SourceKey): AccountSelection {
  return readPreference<AccountSelection>({
    key: accountSelectionStorageKey(source),
    fallback: { kind: "all" },
    parse: parseSelection,
    serialize: serializeSelection,
    eventName: `workbuddy-shift:token-stats-account-changed:${source}`,
  });
}

/** 写入某个来源的账号选择；存储不可用时静默降级（会话内 state 仍生效）。 */
export function persistAccountSelection(source: SourceKey, selection: AccountSelection): void {
  writePreference<AccountSelection>(
    {
      key: accountSelectionStorageKey(source),
      fallback: { kind: "all" },
      parse: parseSelection,
      serialize: serializeSelection,
      eventName: `workbuddy-shift:token-stats-account-changed:${source}`,
    },
    selection,
  );
}

/**
 * 账号显示名：**直接复用全局口径** `displayName()`（R5-07：备注优先）。
 *
 * 历史实现是「昵称 → 邮箱」的第四套规则，导致同一账号在卡片叫昵称、在下拉叫邮箱、
 * 有备注时两处都不显示备注。现在下拉、标题、导出文件名与卡片 / 积分页同名。
 *
 * 账号库里找不到该 uid（历史账号 / 其它机器）时保留 `…{uid 后 6 位}` 兜底；
 * 空 uid 仍是「未归属」。
 */
export function accountLabel(uid: string, accounts: AccountMeta[]): string {
  if (!uid) return "未归属";
  const meta = accounts.find((account) => account.uid === uid);
  if (meta) return displayName(meta);
  return uid.length > 6 ? `…${uid.slice(-6)}` : uid;
}

/** 该来源应列出的账号库账号：WorkBuddy 两个来源按档位过滤，IDE/CLI 列全部。 */
function accountsForSource(source: SourceKey, accounts: AccountMeta[]): AccountMeta[] {
  const withUid = accounts.filter(
    (account): account is AccountMeta & { uid: string } =>
      typeof account.uid === "string" && account.uid.trim().length > 0,
  );
  if (source === "workbuddy") {
    return withUid.filter((account) => (account.variant ?? "cn") === "cn");
  }
  if (source === "workbuddy-ai") {
    return withUid.filter((account) => account.variant === "ai");
  }
  // IDE：路径 uid 与账号库 uid 可能对不上，仍然列出全部账号（对不上就显示 0）。
  // CLI：日志不记录账号（见 `cliAccountsUnsupported`），只留 所有/未归属。
  if (source === "codebuddy-cli") return [];
  return withUid;
}

/** CLI 的 JSONL 不写账号字段：下拉只有「所有账号 / 未归属」，页面必须说明原因。 */
export function cliAccountsUnsupported(source: SourceKey): boolean {
  return source === "codebuddy-cli";
}

/**
 * 账号下拉选项：`所有账号` → 账号库账号（按来源/档位过滤） → `未归属`。
 *
 * 调用方只在 `source.accounts` 存在（新后端）时渲染下拉；旧后端没有该字段时保持原样。
 */
export function accountOptions(
  source: SourceKey,
  accounts: AccountMeta[],
  usedAccounts: TokenStatsAccount[] | undefined,
): AccountOption[] {
  const usedUids = new Set((usedAccounts ?? []).map((account) => account.uid));
  const options: AccountOption[] = [
    { value: ALL_MARKER, label: "所有账号", hasUsage: true },
  ];
  const seen = new Set<string>();
  for (const account of accountsForSource(source, accounts)) {
    const uid = account.uid ?? "";
    if (!uid || seen.has(uid)) continue;
    seen.add(uid);
    options.push({ value: `uid:${uid}`, label: accountLabel(uid, accounts), hasUsage: usedUids.has(uid) });
  }
  // 后端返回了「有用量但账号库里没有」的 uid（历史账号 / 其它机器）：补进下拉，
  // 否则这些用量只能通过「所有账号」看到，用户会以为丢了数据。
  for (const used of usedAccounts ?? []) {
    if (!used.uid || seen.has(used.uid)) continue;
    seen.add(used.uid);
    options.push({
      value: `uid:${used.uid}`,
      label: accountLabel(used.uid, accounts),
      hasUsage: true,
    });
  }
  options.push({ value: UNASSIGNED_MARKER, label: "未归属", hasUsage: usedUids.has("") });
  return options;
}

/**
 * 按账号过滤一个来源的聚合：纯客户端，**不**触发任何请求。
 *
 * - `all`：原样返回（沿用后端全局聚合，数字与改造前逐项一致）；
 * - 具体 uid / 未归属：从 `source.accounts` 取四张表，`sessions` / `requests` 按 `uid`
 *   过滤；账号没有用量时返回 0 与空集，页面显示空态而不是骨架屏。
 */
export function filterSourceByAccount(
  source: TokenStatsSource,
  selection: AccountSelection,
): TokenStatsSource {
  if (selection.kind === "all") return source;
  const uid = selection.kind === "unassigned" ? "" : selection.uid;
  const account = (source.accounts ?? []).find((item) => item.uid === uid);
  const matches = (rowUid?: string) => (rowUid ?? "") === uid;
  return {
    ...source,
    summary: account?.summary ?? emptyTotals(),
    models: account?.models ?? [],
    projects: account?.projects ?? [],
    daily: account?.daily ?? [],
    dailyByModel: account?.dailyByModel,
    hours: account?.hours ?? [],
    sessions: source.sessions.filter((row) => matches(row.uid)),
    requests: source.requests?.filter((row) => matches(row.uid)),
  };
}

/** 当前选择对应的账号标签（用于标题与导出文件名）。 */
export function selectionLabel(selection: AccountSelection, accounts: AccountMeta[]): string {
  switch (selection.kind) {
    case "all":
      return "所有账号";
    case "unassigned":
      return "未归属";
    case "uid":
      return accountLabel(selection.uid, accounts);
  }
}

/** 当前来源是否已经提供账号维度（旧后端没有 `accounts` 字段）。 */
export function hasAccountDimension(source: TokenStatsSource | undefined): boolean {
  return Array.isArray(source?.accounts);
}
