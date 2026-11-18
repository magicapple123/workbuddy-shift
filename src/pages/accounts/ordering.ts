import type { AccountMeta, CreditExpiry } from "@/lib/types";

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
