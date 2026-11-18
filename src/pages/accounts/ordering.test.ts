import { describe, expect, it } from "vitest";

import type { AccountMeta, CreditExpiry } from "@/lib/types";
import { creditPriorityRank, orderAccountsByCredit } from "./ordering";

function account(id: string): AccountMeta {
  return { id, uid: id, email: `${id}@example.com`, expiresAt: null } as AccountMeta;
}

function credit(overrides: Partial<CreditExpiry>): CreditExpiry {
  return { ok: true, expired: false, resources: [], ...overrides } as CreditExpiry;
}

const now = 1_800_000_000_000;

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
