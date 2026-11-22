import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";

import type { AccountMeta, DailyActivityAccountResult } from "@/lib/types";

import { DailyActivityPanel } from "./daily-activity-panel";

/**
 * 结果面板的最小渲染断言（node 环境 + `renderToStaticMarkup`，与账号页冒烟测试同法）。
 *
 * 状态映射本身由 `../daily-activity.test.ts` 覆盖；这里只锁「四种状态真的出现在界面上」
 * 与「整体失败时已渲染的结果行不会被一起抹掉」这两件渲染层面的行为。
 */

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

function account(id: string): AccountMeta {
  return { id, uid: id, email: `${id}@example.com`, expiresAt: null } as AccountMeta;
}

function render(
  accounts: AccountMeta[],
  state: {
    running?: boolean;
    result?: { date: string; accounts: DailyActivityAccountResult[] } | null;
    error?: string | null;
  },
) {
  const controller = {
    state: {
      running: state.running ?? false,
      result: state.result ?? null,
      error: state.error ?? null,
    },
    confirmOpen: false,
    request: vi.fn(),
    cancel: vi.fn(),
    confirm: vi.fn(),
    dismiss: vi.fn(),
  };
  return renderToStaticMarkup(createElement(DailyActivityPanel, { accounts, controller }));
}

describe("每日活跃结果面板", () => {
  it("逐账号显示成功（含模型）/ 已跳过 / 不支持 / 失败四类", () => {
    const html = render([account("acc-1"), account("acc-2"), account("acc-3"), account("acc-4")], {
      result: {
        date: "2026-11-22",
        accounts: [
          row({ accountId: "acc-1", ok: true, phase: "done", model: "hy3", modelApplied: true }),
          row({ accountId: "acc-2", ok: true, skipped: true, phase: "skipped" }),
          row({
            accountId: "acc-3",
            phase: "unsupported",
            error: "每日活跃会话仅国际版可用；国内版走签到",
          }),
          row({ accountId: "acc-4", phase: "turn", modelsTried: ["hy3"], error: "ACP 失败" }),
        ],
      },
    });

    expect(html).toContain("每日活跃");
    expect(html).toContain("2026-11-22");
    expect(html).toContain("acc-1@example.com");
    expect(html).toContain("模型 hy3");
    expect(html).toContain("今日已跳过");
    expect(html).toContain("每日活跃会话仅国际版可用；国内版走签到");
    expect(html).toContain("会话执行失败");
    expect(html).toContain("共 4 个账号：1 个成功、1 个今日已跳过、1 个不支持、1 个失败");
  });

  it("整体失败只加一条说明，上一轮已渲染的结果行仍在", () => {
    const html = render([account("acc-1")], {
      result: {
        date: "2026-11-22",
        accounts: [row({ accountId: "acc-1", ok: true, phase: "done", model: "hy3" })],
      },
      error: "无法连接 workbuddy-shift 服务",
    });

    expect(html).toContain("本轮失败：无法连接 workbuddy-shift 服务");
    expect(html).toContain("模型 hy3");
    expect(html).toContain("成功");
  });

  it("进行中显示进度态，且关闭按钮被禁用", () => {
    const html = render([account("acc-1")], { running: true });
    expect(html).toContain("正在为账号逐个跑会话");
    expect(html).toContain("aria-busy=\"true\"");
    expect(html).toContain("disabled");
  });

  it("没有任何结果时不渲染面板（工具栏按钮仍有自己的确认框）", () => {
    const html = render([account("acc-1")], {});
    expect(html).not.toContain("每日活跃");
  });
});
