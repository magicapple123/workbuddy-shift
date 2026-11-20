import { describe, expect, it } from "vitest";

import type { AccountMeta, TokenStatsSource, TokenStatsTotals } from "@/lib/types";

import {
  accountLabel,
  accountOptions,
  cliAccountsUnsupported,
  emptyTotals,
  filterSourceByAccount,
  parseSelection,
  selectionLabel,
  serializeSelection,
} from "./accounts";

function totals(total: number): TokenStatsTotals {
  return {
    total,
    input: total,
    output: 0,
    cacheRead: 0,
    cacheWrite: 0,
    uncachedInput: total,
    records: 1,
    cacheHitRate: null,
  };
}

function group(key: string, uid?: string) {
  return { key, uid, ...totals(10) };
}

function account(uid: string, total: number) {
  return {
    uid,
    summary: totals(total),
    models: [group(`model-${uid}`)],
    projects: [group(`project-${uid}`)],
    daily: [group("2026-11-19")],
    hours: [group("0-9")],
  };
}

function source(overrides: Partial<TokenStatsSource> = {}): TokenStatsSource {
  return {
    source: "workbuddy",
    summary: totals(300),
    models: [group("model-a")],
    projects: [group("project-a")],
    sessions: [group("s1", "uid-a"), group("s2", "uid-b"), group("s3", "")],
    daily: [group("2026-11-19")],
    hours: [group("0-9")],
    accounts: [account("uid-a", 200), account("uid-b", 90), account("", 10)],
    filesScanned: 3,
    parseErrors: 0,
    ...overrides,
  };
}

const ACCOUNTS: AccountMeta[] = [
  { id: "1", uid: "uid-a", email: "a@example.com", nickname: "账号 A", enterpriseName: null, expiresAt: null, refreshExpiresAt: null, refreshedAt: null, createdAt: null, needsRelogin: false, needsReloginReason: null, variant: "cn" },
  { id: "2", uid: "uid-b", email: "b@example.com", nickname: null, enterpriseName: null, expiresAt: null, refreshExpiresAt: null, refreshedAt: null, createdAt: null, needsRelogin: false, needsReloginReason: null, variant: "ai" },
  { id: "3", uid: "uid-c", email: null, nickname: null, enterpriseName: null, expiresAt: null, refreshExpiresAt: null, refreshedAt: null, createdAt: null, needsRelogin: false, needsReloginReason: null, variant: "cn" },
];

describe("Token 账号维度", () => {
  it("每个来源独立过滤账号库：cn / ai 各自只列自己的账号", () => {
    const cn = accountOptions("workbuddy", ACCOUNTS, source().accounts);
    // 国内版来源 → 先列 cn 账号库（uid-a、uid-c），后端返回但账号库里查不到的 uid-b
    // 也补进来（否则这部分用量只能在「所有账号」里看到），最后是「未归属」。
    expect(cn.map((option) => option.value)).toEqual([
      "all",
      "uid:uid-a",
      "uid:uid-c",
      "uid:uid-b",
      "unassigned",
    ]);
    const ai = accountOptions("workbuddy-ai", ACCOUNTS, undefined);
    expect(ai.map((option) => option.value)).toEqual(["all", "uid:uid-b", "unassigned"]);
    // 互不影响：没有「未归属之外」的跨档位污染——国内版视图选到 uid-b 也只显示它自己的数。
    expect(cn.filter((option) => option.value === "uid:uid-b")).toHaveLength(1);

  });

  it("零用量账号仍列出（显示 0），CLI 只有 所有/未归属", () => {
    const partial = accountOptions("workbuddy", ACCOUNTS, [account("uid-a", 200)]);
    const zeroUsage = partial.find((option) => option.value === "uid:uid-c");
    expect(zeroUsage?.hasUsage).toBe(false);
    expect(cliAccountsUnsupported("codebuddy-cli")).toBe(true);
    const cli = accountOptions("codebuddy-cli", ACCOUNTS, [account("", 5)]);
    expect(cli.map((option) => option.value)).toEqual(["all", "unassigned"]);
  });

  it("账号库为空时不崩：只有 所有/未归属", () => {
    const options = accountOptions("codebuddy-ide", [], undefined);
    expect(options.map((option) => option.value)).toEqual(["all", "unassigned"]);
  });

  it("按 uid 过滤：summary/models/projects/daily/hours 只留该账号，sessions 按 uid 过滤", () => {
    const filtered = filterSourceByAccount(source(), { kind: "uid", uid: "uid-a" });
    expect(filtered.summary.total).toBe(200);
    expect(filtered.models[0]?.key).toBe("model-uid-a");
    expect(filtered.sessions.map((row) => row.key)).toEqual(["s1"]);
    // 全局聚合不被改写
    expect(source().summary.total).toBe(300);
  });

  it("未归属桶：空 uid 的账号与会话", () => {
    const filtered = filterSourceByAccount(source(), { kind: "unassigned" });
    expect(filtered.summary.total).toBe(10);
    expect(filtered.sessions.map((row) => row.key)).toEqual(["s3"]);
  });

  it("账号在该来源没有用量：返回 0 与空集，不需要请求", () => {
    const filtered = filterSourceByAccount(source(), { kind: "uid", uid: "uid-zzz" });
    expect(filtered.summary).toEqual(emptyTotals());
    expect(filtered.sessions).toEqual([]);
    expect(filtered.models).toEqual([]);
  });

  it("所有账号：原样返回全局聚合（不重算）", () => {
    const global = source();
    expect(filterSourceByAccount(global, { kind: "all" })).toBe(global);
  });

  it("选择值可往返，显示名回退 uid 后 6 位", () => {
    expect(parseSelection(serializeSelection({ kind: "uid", uid: "uid-a" }))).toEqual({
      kind: "uid",
      uid: "uid-a",
    });
    expect(parseSelection("unassigned")).toEqual({ kind: "unassigned" });
    expect(parseSelection("坏了")).toBeNull();
    expect(accountLabel("uid-a", ACCOUNTS)).toBe("账号 A");
    expect(accountLabel("uid-b", ACCOUNTS)).toBe("b@example.com");
    expect(accountLabel("0123456789abcdef", ACCOUNTS)).toBe("…abcdef");
    expect(accountLabel("", ACCOUNTS)).toBe("未归属");
    expect(selectionLabel({ kind: "all" }, ACCOUNTS)).toBe("所有账号");
  });

  /**
   * R5-07：Token 统计下拉与账号卡片同名——有备注显示备注，显式手机号偏好仍然压过备注。
   *
   * 此前的本地实现是「昵称 → 邮箱」，用户自己写的备注完全不出现。
   */
  it("账号显示名复用全局口径：备注优先、手机号例外、无备注回退昵称/邮箱", () => {
    const noted = [{ ...ACCOUNTS[0], note: "设计组-主号" }];
    expect(accountLabel("uid-a", noted)).toBe("设计组-主号");
    expect(
      accountLabel("uid-a", [
        { ...ACCOUNTS[0], note: "设计组-主号", phoneNumber: "13800000000", displayField: "phone" },
      ]),
    ).toBe("13800000000");
    // 无备注：昵称 → 邮箱 → uid。
    expect(accountLabel("uid-a", ACCOUNTS)).toBe("账号 A");
    expect(accountLabel("uid-b", ACCOUNTS)).toBe("b@example.com");
    expect(accountLabel("uid-c", ACCOUNTS)).toBe("uid-c");
  });
});
