import { describe, expect, it, vi } from "vitest";

import type { AccountMeta, DailyActivityAccountResult, DailyActivityRunResult } from "@/lib/types";

import {
  classifyDailyActivity,
  dailyActivityAccountLabel,
  describeDailyActivityRow,
  EMPTY_DAILY_ACTIVITY_STATE,
  runDailyActivityAttempt,
  summarizeDailyActivity,
  type DailyActivityState,
} from "./daily-activity";

/** 结果行工厂：默认值 = core 的 `AccountRunResult::default()`（camelCase）。 */
function row(overrides: Partial<DailyActivityAccountResult> = {}): DailyActivityAccountResult {
  return {
    accountId: "acc-1",
    ok: false,
    skipped: false,
    phase: "create",
    elapsedMs: 0,
    ...overrides,
  };
}

function account(id: string, overrides: Partial<AccountMeta> = {}): AccountMeta {
  return { id, uid: id, email: `${id}@example.com`, expiresAt: null, ...overrides } as AccountMeta;
}

function result(rows: DailyActivityAccountResult[]): DailyActivityRunResult {
  return { date: "2026-11-22", accounts: rows };
}

describe("每日活跃结果状态映射", () => {
  it("跑完的账号是成功，并显示实际使用的模型", () => {
    const view = describeDailyActivityRow(
      row({ ok: true, phase: "done", model: "hy3", modelApplied: true, status: "completed" }),
    );
    expect(view.outcome).toBe("success");
    expect(view.label).toBe("成功");
    expect(view.badge).toBe("success");
    expect(view.detail).toContain("hy3");
  });

  it("今日已完成是「已跳过」而不是「成功」（ok 也是 true，判定顺序有语义）", () => {
    const skipped = row({ ok: true, skipped: true, phase: "skipped" });
    expect(classifyDailyActivity(skipped)).toBe("skipped");
    const view = describeDailyActivityRow(skipped);
    expect(view.label).toBe("今日已跳过");
    expect(view.detail).toContain("没有发任何请求");
  });

  it("国内版 / 缺凭据的账号标注「不支持」并带上后端原因，不静默忽略", () => {
    const view = describeDailyActivityRow(
      row({ phase: "unsupported", error: "每日活跃会话仅国际版可用；国内版走签到" }),
    );
    expect(view.outcome).toBe("unsupported");
    expect(view.label).toBe("不支持");
    expect(view.badge).toBe("warning");
    expect(view.detail).toBe("每日活跃会话仅国际版可用；国内版走签到");
  });

  it("失败行带上卡住的阶段、尝试过的模型与脱敏原因", () => {
    const view = describeDailyActivityRow(
      row({
        phase: "poll",
        modelsTried: ["hy3", "deepseek-v4.1-flash"],
        error: "轮询超时",
      }),
    );
    expect(view.outcome).toBe("failed");
    expect(view.label).toBe("失败");
    expect(view.badge).toBe("danger");
    expect(view.detail).toContain("等待会话完成失败");
    expect(view.detail).toContain("hy3 → deepseek-v4.1-flash");
    expect(view.detail).toContain("轮询超时");
  });

  it("成功但 set_model 未生效时说明按默认模型跑完", () => {
    const view = describeDailyActivityRow(
      row({ ok: true, phase: "done", model: "hy3", modelApplied: false }),
    );
    expect(view.outcome).toBe("success");
    expect(view.detail).toContain("set_model 未被接受");
  });

  it("计数摘要按四类分列", () => {
    const text = summarizeDailyActivity(
      result([
        row({ accountId: "a", ok: true, phase: "done", model: "hy3" }),
        row({ accountId: "b", ok: true, skipped: true, phase: "skipped" }),
        row({ accountId: "c", phase: "unsupported", error: "仅国际版可用" }),
        row({ accountId: "d", phase: "create", error: "建会话失败" }),
      ]),
    );
    expect(text).toBe("共 4 个账号：1 个成功、1 个今日已跳过、1 个不支持、1 个失败");
  });

  it("空账号库的摘要有明确说法，不显示「共 0 个账号：」这种半截文案", () => {
    expect(summarizeDailyActivity(result([]))).toBe("账号库里还没有账号");
  });
});

describe("每日活跃结果行的账号名", () => {
  it("accountId 命中 id 或 uid 时用显示名", () => {
    const accounts = [account("acc-1"), account("acc-2", { uid: "uid-2" })];
    expect(dailyActivityAccountLabel(accounts, "acc-2")).toBe("acc-2@example.com");
    expect(dailyActivityAccountLabel(accounts, "uid-2")).toBe("acc-2@example.com");
  });

  it("账号已删除 / 命中不了时退回 key 本身，不显示空行", () => {
    expect(dailyActivityAccountLabel([], " gone-id ")).toBe("gone-id");
    expect(dailyActivityAccountLabel([], "")).toBe("未知账号");
  });
});

describe("每日活跃：确认框未确认不发请求", () => {
  it("取消 / 未确认时不调用请求函数，也不改状态", async () => {
    const runner = vi.fn<() => Promise<DailyActivityRunResult>>();
    const publish = vi.fn<(next: DailyActivityState) => void>();

    await runDailyActivityAttempt({
      state: EMPTY_DAILY_ACTIVITY_STATE,
      confirmed: false,
      runner,
      publish,
    });

    expect(runner).not.toHaveBeenCalled();
    expect(publish).not.toHaveBeenCalled();
  });

  it("确认后先发布进行中状态，再用返回值替换结果", async () => {
    const finished = result([row({ ok: true, phase: "done", model: "hy3" })]);
    const runner = vi.fn(async () => finished);
    const published: DailyActivityState[] = [];

    await runDailyActivityAttempt({
      state: EMPTY_DAILY_ACTIVITY_STATE,
      confirmed: true,
      runner,
      publish: (next) => published.push(next),
    });

    expect(runner).toHaveBeenCalledTimes(1);
    expect(published).toHaveLength(2);
    expect(published[0]).toEqual({ running: true, result: null, error: null });
    expect(published[1]).toEqual({ running: false, result: finished, error: null });
  });

  it("进行中再点一次不会发第二个请求", async () => {
    const runner = vi.fn(async () => result([]));
    const publish = vi.fn<(next: DailyActivityState) => void>();

    await runDailyActivityAttempt({
      state: { running: true, result: null, error: null },
      confirmed: true,
      runner,
      publish,
    });

    expect(runner).not.toHaveBeenCalled();
    expect(publish).not.toHaveBeenCalled();
  });

  it("本轮失败只清 running 并记录原因，保留上一次结果", async () => {
    const previous = result([row({ accountId: "a", ok: true, phase: "done", model: "hy3" })]);
    const published: DailyActivityState[] = [];

    await runDailyActivityAttempt({
      state: { running: false, result: previous, error: null },
      confirmed: true,
      runner: () => Promise.reject(new Error("无法连接 workbuddy-shift 服务")),
      publish: (next) => published.push(next),
    });

    expect(published[published.length - 1]).toEqual({
      running: false,
      result: previous,
      error: "无法连接 workbuddy-shift 服务",
    });
  });
});
