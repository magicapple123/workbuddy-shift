import { displayName } from "@/lib/account-display";
import type { AccountMeta, CreditExpiry, RateLimitEntry } from "@/lib/types";

/** 即将到期（expiringSoon）但仍可用的积分总量；接口失败或缺失按 0 处理。 */
export function expiringSoonAmount(credit?: CreditExpiry): number {
  return credit?.ok ? credit.expiringSoonRemaining ?? 0 : 0;
}

export function hasExpiringSoonCredits(credit?: CreditExpiry): boolean {
  return credit?.ok === true && expiringSoonAmount(credit) > 0;
}

/** 最近一个「仍有剩余且即将到期」的资源时刻；否则回落到账号的最近到期时刻。 */
export function soonestRelevantExpiry(credit?: CreditExpiry): number {
  const soonestExpiringCredit = (credit?.resources ?? [])
    .filter((resource) => resource.remaining > 0 && resource.expiringSoon && resource.expireAt != null)
    .map((resource) => resource.expireAt as number)
    .reduce((soonest, expireAt) => Math.min(soonest, expireAt), Number.POSITIVE_INFINITY);
  return Number.isFinite(soonestExpiringCredit)
    ? soonestExpiringCredit
    : credit?.soonestExpireAt ?? Number.POSITIVE_INFINITY;
}

/** 排序优先级：即将到期 > 已过期 > 正常；数据缺失排最后。 */
export function creditPriorityRank(credit?: CreditExpiry): number {
  if (!credit?.ok) return 3;
  if (hasExpiringSoonCredits(credit)) return 0;
  if (credit.expired) return 1;
  return 2;
}

/* -------------------------------------------------------------------------- */
/* 排序维度（六项单选 + 同一项再点反转）                                        */
/* -------------------------------------------------------------------------- */

/**
 * 账号卡片排序维度。
 *
 * - `default`：现有的积分优先级排序（即将到期 > 已过期 > 正常）。
 * - `rateLimitRecovery`：「额度恢复时间」——受限模型中最快恢复的时刻。
 * - `creditExpiry`：「积分到期时间」——仍有剩余的资源里最早到期时刻。
 * - `creditRemaining`：「剩余积分」——当前剩余积分总量。
 * - `creditUpdated`：「最近更新」——积分采集时刻（卡片右下「18:33 更新」）。
 * - `name`：显示名（中文按拼音，数字按数值）。
 *
 * 这些字符串会写进 localStorage 的 `accounts.sort`，改动等于让用户偏好失效，勿轻易重命名。
 */
export type SortKey =
  | "default"
  | "rateLimitRecovery"
  | "creditExpiry"
  | "creditRemaining"
  | "creditUpdated"
  | "name";

export type SortDirection = "asc" | "desc";

export interface SortState {
  key: SortKey;
  dir: SortDirection;
}

/** 六项排序维度的固定迭代顺序（菜单与解析共用）。 */
export const SORT_KEYS: readonly SortKey[] = [
  "default",
  "rateLimitRecovery",
  "creditExpiry",
  "creditRemaining",
  "creditUpdated",
  "name",
];

/**
 * 各维度的默认方向：时间/金额类按用户最关心的方向先给一次。
 *
 * `default` 没有方向语义，固定 `asc` 只为了状态形状统一（反转对它无效）。
 */
export const SORT_DEFAULT_DIRECTION: Record<SortKey, SortDirection> = {
  default: "asc",
  rateLimitRecovery: "asc",
  creditExpiry: "asc",
  creditRemaining: "desc",
  creditUpdated: "desc",
  name: "asc",
};

/** 缺省排序：现有行为（不发任何排序请求即等于它）。 */
export const DEFAULT_SORT: SortState = { key: "default", dir: SORT_DEFAULT_DIRECTION.default };

function isSortKey(value: unknown): value is SortKey {
  return typeof value === "string" && SORT_KEYS.some((key) => key === value);
}

function isSortDirection(value: unknown): value is SortDirection {
  return value === "asc" || value === "desc";
}

/**
 * 解析 localStorage 里的排序偏好，任何异常都回退默认排序（不抛错）。
 *
 * 兼容三种输入：`{"key":"name","dir":"desc"}`（标准）、纯维度名 `name`
 * （方向取该维度默认值）、以及 `""` / `null` / 损坏文本 / 未知维度。
 */
export function parseSort(raw: string | null | undefined): SortState {
  if (typeof raw !== "string") return { ...DEFAULT_SORT };
  const text = raw.trim();
  if (!text) return { ...DEFAULT_SORT };

  // 纯维度名（早期手写或调试时写入的简写）。
  if (isSortKey(text)) return { key: text, dir: SORT_DEFAULT_DIRECTION[text] };

  let parsed: unknown;
  try {
    parsed = JSON.parse(text);
  } catch {
    return { ...DEFAULT_SORT };
  }
  if (!parsed || typeof parsed !== "object") return { ...DEFAULT_SORT };

  const record = parsed as { key?: unknown; dir?: unknown };
  if (!isSortKey(record.key)) return { ...DEFAULT_SORT };
  const key = record.key;
  // 方向缺失/非法时用该维度默认方向；`default` 恒为 asc，避免出现「默认 ↓」这种无意义状态。
  const dir = key === "default"
    ? SORT_DEFAULT_DIRECTION.default
    : isSortDirection(record.dir)
      ? record.dir
      : SORT_DEFAULT_DIRECTION[key];
  return { key, dir };
}

/** 偏好值 → localStorage 文本（与 `parseSort` 对称）。 */
export function serializeSort(sort: SortState): string {
  return JSON.stringify({ key: sort.key, dir: sort.dir });
}

/**
 * 点击某个维度后的下一个状态：换维度 → 用该维度默认方向；同维度 → 只翻转方向。
 *
 * `default` 没有方向可翻转，重复点击保持 `{key:"default", dir:"asc"}`。
 */
export function nextSortState(current: SortState, key: SortKey): SortState {
  if (current.key !== key) return { key, dir: SORT_DEFAULT_DIRECTION[key] };
  if (key === "default") return { key, dir: SORT_DEFAULT_DIRECTION.default };
  return { key, dir: current.dir === "asc" ? "desc" : "asc" };
}

/* -------------------------------------------------------------------------- */
/* 各维度取值（缺值统一返回 null，由排序器放到末尾）                            */
/* -------------------------------------------------------------------------- */

/**
 * 「额度恢复时间」：受限模型中最早的、尚未恢复的 `resetAt`。
 *
 * 过滤条件与卡片上的限额 chip 完全一致（`resetAt > now`）：已恢复的旧记录
 * 卡片上不显示，排序也不该把它当成「受限账号」冲到最前面。
 */
export function soonestRateLimitResetAt(
  entries: readonly RateLimitEntry[] | undefined,
  now: number,
): number | null {
  const active = (entries ?? []).filter(
    (entry) => Number.isFinite(entry.resetAt) && entry.resetAt > now,
  );
  if (active.length === 0) return null;
  return active.reduce((soonest, entry) => Math.min(soonest, entry.resetAt), Number.POSITIVE_INFINITY);
}

/**
 * 「积分到期时间」：仍有剩余的资源中最早的 `expireAt`，回退账号级 `soonestExpireAt`。
 *
 * 与 `soonestRelevantExpiry` 的区别：这里不要求 `expiringSoon`，任何「还有剩余且
 * 有到期日」的资源都参与，用户才能看到「谁先到期」。
 */
export function earliestRemainingExpiry(credit?: CreditExpiry): number | null {
  if (!credit?.ok) return null;
  const earliest = (credit.resources ?? [])
    .filter((resource) => resource.remaining > 0 && resource.expireAt != null)
    .map((resource) => resource.expireAt as number)
    .reduce((soonest, expireAt) => Math.min(soonest, expireAt), Number.POSITIVE_INFINITY);
  if (Number.isFinite(earliest)) return earliest;
  return credit.soonestExpireAt ?? null;
}

/**
 * 「剩余积分」：`totalRemaining`；企业版不限量（null）与未加载一样按缺值处理。
 */
export function creditRemainingValue(credit?: CreditExpiry): number | null {
  if (!credit?.ok) return null;
  const remaining = credit.totalRemaining;
  return typeof remaining === "number" && Number.isFinite(remaining) ? remaining : null;
}

/** 中文显示名排序器：拼音序 + 数字按数值（「账号2」排在「账号10」前）。 */
const zhNameCollator = new Intl.Collator("zh-CN", { numeric: true, sensitivity: "base" });

/** 维度取值类型：数字按大小比，字符串按中文显示名规则比。 */
type SortMetric = number | string;

/* -------------------------------------------------------------------------- */
/* 默认排序（原 orderAccountsByCredit，行为与字节不变）                         */
/* -------------------------------------------------------------------------- */

export interface OrderedAccounts {
  ordered: AccountMeta[];
  /** 第一个「即将到期」的账号（仅当全部账号积分已就绪时给出）。 */
  priorityAccountId?: string;
  /** 全部账号的积分都已加载完成（否则保持后端返回顺序，避免加载中跳动）。 */
  ready: boolean;
}

/**
 * 账号卡片排序：有即将到期积分的账号优先，其次按最近到期时间、再按即将到期数量。
 *
 * 只有全部可见账号的积分都已加载完成才排序；加载中保持原顺序，避免每次轮询
 * 返回一部分结果就重排卡片（用户正在看的卡片会跳动）。排序稳定：同分保持原索引。
 */
export function orderAccountsByCredit(
  accounts: AccountMeta[],
  creditMap: Record<string, CreditExpiry>,
  creditLoadingMap: Record<string, boolean>,
): OrderedAccounts {
  const ready =
    accounts.length > 0 &&
    accounts.every((account) => Boolean(creditMap[account.id]) && !creditLoadingMap[account.id]);
  if (!ready) return { ordered: accounts, ready: false };

  const ordered = accounts
    .map((account, index) => ({ account, index }))
    .sort((left, right) => {
      const leftCredit = creditMap[left.account.id];
      const rightCredit = creditMap[right.account.id];
      const rankDifference = creditPriorityRank(leftCredit) - creditPriorityRank(rightCredit);
      if (rankDifference !== 0) return rankDifference;

      const leftExpiry = soonestRelevantExpiry(leftCredit);
      const rightExpiry = soonestRelevantExpiry(rightCredit);
      if (leftExpiry !== rightExpiry) return leftExpiry - rightExpiry;

      const amountDifference = expiringSoonAmount(rightCredit) - expiringSoonAmount(leftCredit);
      if (amountDifference !== 0) return amountDifference;
      return left.index - right.index;
    })
    .map(({ account }) => account);

  return {
    ordered,
    priorityAccountId: ordered.find((account) => hasExpiringSoonCredits(creditMap[account.id]))?.id,
    ready: true,
  };
}

/* -------------------------------------------------------------------------- */
/* 六项排序入口                                                                 */
/* -------------------------------------------------------------------------- */

/** 排序所需的全部信号快照（都是账号页已有的映射，避免排序层再连 store）。 */
export interface SortInputs {
  /** 账号 id → 积分到期信息（`useAccountsStore`）。 */
  creditMap: Record<string, CreditExpiry>;
  /** 账号 id → 积分是否加载中（`useAccountsStore`）。 */
  creditLoadingMap: Record<string, boolean>;
  /** 账号 id → 积分采集时刻（卡片右下「18:33 更新」用的同一时间戳）。 */
  creditUpdatedAtMap: Record<string, number>;
  /** 账号 id → 当前受限模型（`useRateLimitSignals`，只含受限账号）。 */
  rateLimitMap: Record<string, RateLimitEntry[]>;
  /** 「现在」的时刻（供测试注入；缺省取 `Date.now()`）。 */
  now?: number;
}

export interface SortedAccounts extends OrderedAccounts {
  /** 本次生效的排序（数据未就绪而保持原序时仍返回它，便于按钮回显）。 */
  sort: SortState;
}

/** 积分类排序在有值判定上依赖积分是否全部加载完成，其余维度不依赖。 */
function needsCreditReady(key: SortKey): boolean {
  return key === "creditExpiry" || key === "creditRemaining";
}

/**
 * 按当前排序偏好排列账号卡片。
 *
 * - `default` 直接复用 `orderAccountsByCredit`（行为完全一致）。
 * - 积分类维度（剩余积分 / 积分到期）在积分未全部就绪时保持后端顺序，避免卡片乱跳；
 *   名称 / 最近更新 / 恢复时间不依赖积分就绪。
 * - **缺值恒排最后，与方向无关**：先按「有值 / 无值」分组再在组内按方向比较，
 *   否则升序时「没有限额」的账号会冲到最前面。同分保持传入顺序（稳定）。
 * - `priorityAccountId` 始终按默认排序的语义给出（「建议优先使用」徽标不随排序变化）。
 */
export function sortAccounts(
  accounts: AccountMeta[],
  sort: SortState,
  inputs: SortInputs,
): SortedAccounts {
  const base = orderAccountsByCredit(accounts, inputs.creditMap, inputs.creditLoadingMap);
  if (sort.key === "default") return { ...base, sort };
  // 积分类维度：积分没加载完就保持后端顺序（此时 base.ordered 就是原数组）。
  if (needsCreditReady(sort.key) && !base.ready) return { ...base, sort };

  const now = inputs.now ?? Date.now();
  const metricOf = (account: AccountMeta): SortMetric | null => {
    switch (sort.key) {
      case "rateLimitRecovery":
        return soonestRateLimitResetAt(inputs.rateLimitMap[account.id], now);
      case "creditExpiry":
        return earliestRemainingExpiry(inputs.creditMap[account.id]);
      case "creditRemaining":
        return creditRemainingValue(inputs.creditMap[account.id]);
      case "creditUpdated":
        return inputs.creditUpdatedAtMap[account.id] ?? null;
      case "name": {
        const name = displayName(account).trim();
        return name.length > 0 ? name : null;
      }
      default:
        return null;
    }
  };

  const direction = sort.dir === "asc" ? 1 : -1;
  const ordered = accounts
    .map((account, index) => ({ account, index, metric: metricOf(account) }))
    .sort((left, right) => {
      const leftMissing = left.metric === null;
      const rightMissing = right.metric === null;
      // 缺值恒排最后：方向只影响有值组内部的相对顺序。
      if (leftMissing !== rightMissing) return leftMissing ? 1 : -1;
      if (left.metric !== null && right.metric !== null) {
        const difference =
          typeof left.metric === "number" && typeof right.metric === "number"
            ? left.metric - right.metric
            : zhNameCollator.compare(String(left.metric), String(right.metric));
        if (difference !== 0) return difference * direction;
      }
      return left.index - right.index;
    })
    .map(({ account }) => account);

  return { ordered, priorityAccountId: base.priorityAccountId, ready: base.ready, sort };
}
