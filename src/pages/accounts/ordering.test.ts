import { describe, expect, it } from "vitest";

import type { AccountMeta, CreditExpiry, RateLimitEntry } from "@/lib/types";
import {
  creditPriorityRank,
  DEFAULT_SORT,
  earliestRemainingExpiry,
  nextSortState,
  orderAccountsByCredit,
  parseSort,
  serializeSort,
  sortAccounts,
  type SortInputs,
  type SortState,
} from "./ordering";

function account(id: string, overrides: Partial<AccountMeta> = {}): AccountMeta {
  return { id, uid: id, email: `${id}@example.com`, expiresAt: null, ...overrides } as AccountMeta;
}

function credit(overrides: Partial<CreditExpiry>): CreditExpiry {
  return { ok: true, expired: false, resources: [], ...overrides } as CreditExpiry;
}

function limit(resetAt: number, model: string | null = "claude"): RateLimitEntry {
  return { model, resetAt, firstSeenAt: resetAt - 1_000, hitCount: 1 };
}

const now = 1_800_000_000_000;

/** 排序输入的最小快照：默认所有账号积分已就绪，避免每个用例重复拼。 */
function inputs(overrides: Partial<SortInputs> = {}): SortInputs {
  return {
    creditMap: {},
    creditLoadingMap: {},
    creditUpdatedAtMap: {},
    rateLimitMap: {},
    now,
    ...overrides,
  };
}

/** 让一组账号「积分全部就绪」，其余维度按需覆盖。 */
function readyCredits(
  accounts: AccountMeta[],
  perAccount: Record<string, CreditExpiry>,
): Record<string, CreditExpiry> {
  const map: Record<string, CreditExpiry> = {};
  for (const item of accounts) map[item.id] = perAccount[item.id] ?? credit({});
  return map;
}

function ids(list: AccountMeta[]): string[] {
  return list.map((item) => item.id);
}

describe("orderAccountsByCredit", () => {
  it("keeps the original order until every credit is loaded", () => {
    const accounts = [account("a"), account("b")];
    const result = orderAccountsByCredit(accounts, { a: credit({}) }, { b: true });
    expect(result.ready).toBe(false);
    expect(result.ordered.map((item) => item.id)).toEqual(["a", "b"]);
    expect(result.priorityAccountId).toBeUndefined();
  });

  it("puts expiring-soon credits first, then expired, then normal", () => {
    const accounts = [account("normal"), account("expired"), account("soon")];
    const creditMap: Record<string, CreditExpiry> = {
      normal: credit({ soonestExpireAt: now + 30 * 86_400_000 }),
      expired: credit({ expired: true }),
      soon: credit({ expiringSoonRemaining: 500, soonestExpireAt: now + 86_400_000 }),
    };
    const result = orderAccountsByCredit(accounts, creditMap, {});
    expect(result.ready).toBe(true);
    expect(result.ordered.map((item) => item.id)).toEqual(["soon", "expired", "normal"]);
    expect(result.priorityAccountId).toBe("soon");
  });

  it("orders by soonest expiry and then by expiring amount", () => {
    const accounts = [account("late"), account("small"), account("large")];
    const creditMap: Record<string, CreditExpiry> = {
      late: credit({ expiringSoonRemaining: 100, soonestExpireAt: now + 2 * 86_400_000 }),
      small: credit({ expiringSoonRemaining: 100, soonestExpireAt: now + 86_400_000 }),
      large: credit({ expiringSoonRemaining: 900, soonestExpireAt: now + 86_400_000 }),
    };
    const result = orderAccountsByCredit(accounts, creditMap, {});
    expect(result.ordered.map((item) => item.id)).toEqual(["large", "small", "late"]);
  });

  it("treats failed credit reads as lowest priority and keeps ties stable", () => {
    const accounts = [account("a"), account("b"), account("c")];
    const creditMap: Record<string, CreditExpiry> = {
      a: credit({ ok: false } as Partial<CreditExpiry>),
      b: credit({}),
      c: credit({}),
    };
    const result = orderAccountsByCredit(accounts, creditMap, {});
    expect(result.ordered.map((item) => item.id)).toEqual(["b", "c", "a"]);
    expect(creditPriorityRank(creditMap.a)).toBe(3);
  });
});

describe("parseSort / nextSortState", () => {
  it("parses the persisted shape and falls back for损坏数据", () => {
    expect(parseSort('{"key":"name","dir":"desc"}')).toEqual({ key: "name", dir: "desc" });
    expect(parseSort('{"key":"creditRemaining","dir":"asc"}')).toEqual({
      key: "creditRemaining",
      dir: "asc",
    });
    expect(parseSort(null)).toEqual(DEFAULT_SORT);
    expect(parseSort(undefined)).toEqual(DEFAULT_SORT);
    expect(parseSort("")).toEqual(DEFAULT_SORT);
    expect(parseSort("   ")).toEqual(DEFAULT_SORT);
    expect(parseSort("not-json")).toEqual(DEFAULT_SORT);
    expect(parseSort('{"key":"bogus","dir":"desc"}')).toEqual(DEFAULT_SORT);
    expect(parseSort('{"dir":"desc"}')).toEqual(DEFAULT_SORT);
    expect(parseSort("[1,2,3]")).toEqual(DEFAULT_SORT);
  });

  it("accepts a bare dimension name and defaults the direction", () => {
    expect(parseSort("name")).toEqual({ key: "name", dir: "asc" });
    expect(parseSort("creditRemaining")).toEqual({ key: "creditRemaining", dir: "desc" });
  });

  it("repairs a missing or invalid direction instead of dropping the dimension", () => {
    expect(parseSort('{"key":"creditUpdated"}')).toEqual({ key: "creditUpdated", dir: "desc" });
    expect(parseSort('{"key":"name","dir":"sideways"}')).toEqual({ key: "name", dir: "asc" });
  });

  it("normalizes the directionless default dimension to asc", () => {
    expect(parseSort('{"key":"default","dir":"desc"}')).toEqual({ key: "default", dir: "asc" });
  });

  it("round-trips through serializeSort", () => {
    const sorts: SortState[] = [
      DEFAULT_SORT,
      { key: "name", dir: "desc" },
      { key: "rateLimitRecovery", dir: "asc" },
      { key: "creditRemaining", dir: "desc" },
    ];
    for (const sort of sorts) expect(parseSort(serializeSort(sort))).toEqual(sort);
  });

  it("flips the direction when the same dimension is clicked again", () => {
    const byName: SortState = { key: "name", dir: "asc" };
    expect(nextSortState(byName, "name")).toEqual({ key: "name", dir: "desc" });
    expect(nextSortState({ key: "name", dir: "desc" }, "name")).toEqual({ key: "name", dir: "asc" });
    // 换维度用该维度默认方向，不继承上一维度的方向。
    expect(nextSortState(byName, "creditRemaining")).toEqual({ key: "creditRemaining", dir: "desc" });
    expect(nextSortState(byName, "rateLimitRecovery")).toEqual({
      key: "rateLimitRecovery",
      dir: "asc",
    });
    // 默认维度没有方向可翻转。
    expect(nextSortState(DEFAULT_SORT, "default")).toEqual(DEFAULT_SORT);
    expect(nextSortState({ key: "name", dir: "desc" }, "default")).toEqual(DEFAULT_SORT);
  });
});

describe("sortAccounts default", () => {
  it("is byte-identical to orderAccountsByCredit", () => {
    const accounts = [account("normal"), account("expired"), account("soon")];
    const creditMap: Record<string, CreditExpiry> = {
      normal: credit({ soonestExpireAt: now + 30 * 86_400_000 }),
      expired: credit({ expired: true }),
      soon: credit({ expiringSoonRemaining: 500, soonestExpireAt: now + 86_400_000 }),
    };
    const expected = orderAccountsByCredit(accounts, creditMap, {});
    const result = sortAccounts(accounts, DEFAULT_SORT, inputs({ creditMap }));
    expect(ids(result.ordered)).toEqual(ids(expected.ordered));
    expect(result.priorityAccountId).toBe(expected.priorityAccountId);
    expect(result.ready).toBe(true);
    expect(result.sort).toEqual(DEFAULT_SORT);
  });

  it("ignores the default direction even if a stale direction was persisted", () => {
    const accounts = [account("a"), account("b")];
    const creditMap = readyCredits(accounts, {});
    const result = sortAccounts(accounts, { key: "default", dir: "desc" }, inputs({ creditMap }));
    expect(ids(result.ordered)).toEqual(["a", "b"]);
  });
});

describe("sortAccounts 额度恢复时间", () => {
  const accounts = [account("slow"), account("fast"), account("unlimited"), account("recovered")];
  const rateLimitMap = {
    slow: [limit(now + 5_000)],
    fast: [limit(now + 30_000, "gpt"), limit(now + 1_000)],
    // 只有已恢复的旧记录：卡片上不显示限额 chip，排序也不应把它算作受限。
    recovered: [limit(now - 1_000)],
  };

  it("puts the soonest recovery first in ascending order", () => {
    const result = sortAccounts(accounts, { key: "rateLimitRecovery", dir: "asc" }, inputs({ rateLimitMap }));
    expect(ids(result.ordered)).toEqual(["fast", "slow", "unlimited", "recovered"]);
  });

  it("keeps missing values last even when the direction is flipped", () => {
    const result = sortAccounts(accounts, { key: "rateLimitRecovery", dir: "desc" }, inputs({ rateLimitMap }));
    expect(ids(result.ordered)).toEqual(["slow", "fast", "unlimited", "recovered"]);
  });

  it("does not require credits to be ready", () => {
    // 积分完全没加载（ready=false）也要能按恢复时间排序。
    const result = sortAccounts(accounts, { key: "rateLimitRecovery", dir: "asc" }, inputs({ rateLimitMap }));
    expect(result.ready).toBe(false);
    expect(ids(result.ordered)[0]).toBe("fast");
  });
});

describe("sortAccounts 积分到期时间", () => {
  const accounts = [account("late"), account("used"), account("soon"), account("noExpiry")];
  const creditMap = readyCredits(accounts, {
    late: credit({ resources: [{ remaining: 10, expireAt: now + 9_000 }] } as Partial<CreditExpiry>),
    // 剩余为 0 的资源不参与「还有剩余中最早到期」的判定，回退到 soonestExpireAt。
    used: credit({
      resources: [{ remaining: 0, expireAt: now + 1_000 }],
      soonestExpireAt: now + 9_000,
    } as Partial<CreditExpiry>),
    soon: credit({ resources: [{ remaining: 5, expireAt: now + 2_000 }] } as Partial<CreditExpiry>),
    noExpiry: credit({ resources: [], soonestExpireAt: null }),
  });

  it("orders by the earliest expiry among resources that still have credits", () => {
    const result = sortAccounts(accounts, { key: "creditExpiry", dir: "asc" }, inputs({ creditMap }));
    // soon(t+2s) → late/used(t+9s，同分保持原索引) → noExpiry（缺值恒末尾）
    expect(ids(result.ordered)).toEqual(["soon", "late", "used", "noExpiry"]);
  });

  it("keeps missing values last after flipping the direction", () => {
    const result = sortAccounts(accounts, { key: "creditExpiry", dir: "desc" }, inputs({ creditMap }));
    expect(ids(result.ordered)).toEqual(["late", "used", "soon", "noExpiry"]);
  });

  it("keeps the backend order while credits are still loading", () => {
    const loading = sortAccounts(
      accounts,
      { key: "creditExpiry", dir: "asc" },
      inputs({ creditMap, creditLoadingMap: { late: true } }),
    );
    expect(loading.ready).toBe(false);
    expect(ids(loading.ordered)).toEqual(["late", "used", "soon", "noExpiry"]);
  });

  it("treats a failed credit read as missing", () => {
    const withFailure = { ...creditMap, late: credit({ ok: false } as Partial<CreditExpiry>) };
    const result = sortAccounts(accounts, { key: "creditExpiry", dir: "asc" }, inputs({ creditMap: withFailure }));
    // 两个缺值账号（late 索引 0、noExpiry 索引 3）在末尾仍按原索引排。
    expect(ids(result.ordered)).toEqual(["soon", "used", "late", "noExpiry"]);
    expect(earliestRemainingExpiry(withFailure.late)).toBeNull();
  });
});

describe("sortAccounts 剩余积分", () => {
  const accounts = [account("mid"), account("many"), account("unlimited")];
  const creditMap = readyCredits(accounts, {
    mid: credit({ totalRemaining: 100 }),
    many: credit({ totalRemaining: 5_000 }),
    unlimited: credit({ unlimited: true, totalRemaining: null }),
  });

  it("sorts rich accounts first in the default descending direction", () => {
    const result = sortAccounts(accounts, { key: "creditRemaining", dir: "desc" }, inputs({ creditMap }));
    expect(ids(result.ordered)).toEqual(["many", "mid", "unlimited"]);
  });

  it("keeps unlimited / unknown (null) values last in ascending order too", () => {
    const result = sortAccounts(accounts, { key: "creditRemaining", dir: "asc" }, inputs({ creditMap }));
    expect(ids(result.ordered)).toEqual(["mid", "many", "unlimited"]);
  });

  it("keeps ties stable", () => {
    const tied = [account("a"), account("b"), account("c")];
    const tiedCredits = readyCredits(tied, {
      a: credit({ totalRemaining: 300 }),
      b: credit({ totalRemaining: 300 }),
      c: credit({ totalRemaining: 300 }),
    });
    const result = sortAccounts(tied, { key: "creditRemaining", dir: "desc" }, inputs({ creditMap: tiedCredits }));
    expect(ids(result.ordered)).toEqual(["a", "b", "c"]);
  });

  it("keeps the backend order while credits are still loading", () => {
    const result = sortAccounts(
      accounts,
      { key: "creditRemaining", dir: "desc" },
      inputs({ creditMap: { mid: credit({ totalRemaining: 100 }) } }),
    );
    expect(result.ready).toBe(false);
    expect(ids(result.ordered)).toEqual(["mid", "many", "unlimited"]);
  });
});

describe("sortAccounts 最近更新", () => {
  const accounts = [account("old"), account("new"), account("never")];
  const creditUpdatedAtMap = { old: now - 60_000, new: now - 1_000 };

  it("puts the freshest collection time first by default", () => {
    const result = sortAccounts(
      accounts,
      { key: "creditUpdated", dir: "desc" },
      inputs({ creditUpdatedAtMap }),
    );
    expect(ids(result.ordered)).toEqual(["new", "old", "never"]);
  });

  it("keeps accounts without a collection time last in either direction", () => {
    const asc = sortAccounts(accounts, { key: "creditUpdated", dir: "asc" }, inputs({ creditUpdatedAtMap }));
    expect(ids(asc.ordered)).toEqual(["old", "new", "never"]);
  });

  it("does not require credits to be ready", () => {
    const result = sortAccounts(
      accounts,
      { key: "creditUpdated", dir: "desc" },
      inputs({ creditUpdatedAtMap }),
    );
    expect(result.ready).toBe(false);
    expect(ids(result.ordered)).toEqual(["new", "old", "never"]);
  });
});

describe("sortAccounts 名称", () => {
  it("uses numeric collation so 账号2 comes before 账号10", () => {
    const accounts = [account("ten", { nickname: "账号10" }), account("two", { nickname: "账号2" })];
    const result = sortAccounts(accounts, { key: "name", dir: "asc" }, inputs({ rateLimitMap: {} }));
    expect(ids(result.ordered)).toEqual(["two", "ten"]);
  });

  it("sorts Chinese names by pinyin (not by code point)", () => {
    // 阿(a)应在白(bai)之前；按 UTF-16 码点比较白(0x767D)反而在前。
    const accounts = [account("bai", { nickname: "白白" }), account("a", { nickname: "阿明" })];
    const result = sortAccounts(accounts, { key: "name", dir: "asc" }, inputs());
    expect(ids(result.ordered)).toEqual(["a", "bai"]);
  });

  it("sorts by the displayed name (honours each account's displayField)", () => {
    // zzz 账号显示备注 Zzz，mmm 账号显示昵称 Mmm：按显示名排 Mmm 在前。
    const accounts = [
      account("zzz", { nickname: "Aaa", note: "Zzz", displayField: "note" }),
      account("mmm", { nickname: "Mmm" }),
    ];
    const result = sortAccounts(accounts, { key: "name", dir: "asc" }, inputs());
    expect(ids(result.ordered)).toEqual(["mmm", "zzz"]);
  });

  it("reverses with the descending direction", () => {
    const accounts = [account("a", { nickname: "阿明" }), account("b", { nickname: "白白" })];
    const result = sortAccounts(accounts, { key: "name", dir: "desc" }, inputs());
    expect(ids(result.ordered)).toEqual(["b", "a"]);
  });
});

describe("sortAccounts 通用行为", () => {
  it("keeps the「建议优先使用」 badge independent of the chosen dimension", () => {
    const accounts = [account("b", { nickname: "Bbb" }), account("a", { nickname: "Aaa" })];
    const creditMap = readyCredits(accounts, {
      b: credit({ expiringSoonRemaining: 500, soonestExpireAt: now + 86_400_000 }),
      a: credit({ soonestExpireAt: now + 30 * 86_400_000 }),
    });
    const result = sortAccounts(accounts, { key: "name", dir: "asc" }, inputs({ creditMap }));
    expect(ids(result.ordered)).toEqual(["a", "b"]);
    expect(result.priorityAccountId).toBe("b");
  });

  it("handles an empty list", () => {
    const result = sortAccounts([], { key: "name", dir: "asc" }, inputs());
    expect(result.ordered).toEqual([]);
    expect(result.ready).toBe(false);
  });
});

/**
 * R5-04 第七项排序：「标记」按 `mark.createdAt` 比较。
 *
 * 关键在于**缺值恒排最后，与方向无关**——否则升序时「没有标记」的账号会顶到最前面。
 */
describe("sortAccounts 标记", () => {
  /** 构造一条标记（`expiresAt` 默认不自动过期）。 */
  function mark(createdAt: number) {
    return { text: "即将限额", color: "warning" as const, createdAt, expiresAt: null };
  }

  const accounts = [
    account("older", { mark: mark(now - 2 * 86_400_000) }),
    account("none"),
    account("newer", { mark: mark(now - 3_600_000) }),
  ];

  it("默认降序：最近标记的在前，无标记恒末尾", () => {
    const result = sortAccounts(accounts, { key: "mark", dir: "desc" }, inputs());
    expect(ids(result.ordered)).toEqual(["newer", "older", "none"]);
  });

  it("升序同样把无标记的放最后", () => {
    const result = sortAccounts(accounts, { key: "mark", dir: "asc" }, inputs());
    expect(ids(result.ordered)).toEqual(["older", "newer", "none"]);
  });

  it("默认方向是 desc", () => {
    expect(nextSortState({ key: "name", dir: "asc" }, "mark")).toEqual({ key: "mark", dir: "desc" });
  });

  it("老版本读到 \"mark\" 不会崩：解析成合法状态，未知维度回退默认", () => {
    expect(parseSort('{"key":"mark","dir":"asc"}')).toEqual({ key: "mark", dir: "asc" });
    expect(parseSort("mark")).toEqual({ key: "mark", dir: "desc" });
    expect(parseSort('{"key":"nope","dir":"asc"}')).toEqual(DEFAULT_SORT);
  });

  it("已过期的标记仍然参与排序（只影响卡片是否显示图标）", () => {
    const expired = [
      account("expired", {
        mark: { text: "即将限额", color: "warning", createdAt: now - 1_000, expiresAt: now - 500 },
      }),
      account("live", {
        mark: { text: "即将限额", color: "warning", createdAt: now - 5_000, expiresAt: now + 60_000 },
      }),
    ];
    const result = sortAccounts(expired, { key: "mark", dir: "desc" }, inputs());
    expect(ids(result.ordered)).toEqual(["expired", "live"]);
  });
});
