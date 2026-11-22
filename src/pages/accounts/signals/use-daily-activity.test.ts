import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { DailyActivityRunResult } from "@/lib/types";

/**
 * `useDailyActivity` 的装配层用例（issue #214）。
 *
 * 纯状态机（`runDailyActivityAttempt`）已由 `../daily-activity.test.ts` 覆盖；这里要证明的是
 * **接线本身**：工具栏按钮调的 `request()` 只开确认框、一个请求都不发，只有确认框的
 * 「开始」按钮调的 `confirm()` 才下发命令。此前这条只能靠读代码判断。
 *
 * 做法：把 `@/lib/api` 换成假实现，用 `renderToStaticMarkup` 渲染一个只捕获 hook 返回值的
 * 探针组件（仓库没有 testing-library，node 环境也没有 DOM），拿到 controller 后在渲染之外
 * 调用 `request()` / `confirm()`，直接看 `runDailyActivity` 被调了几次。
 */

vi.mock("@/lib/api", () => ({
  // 只有 confirm() 应该碰到它。
  runDailyActivity: vi.fn(),
  // daily-activity.ts 的错误格式化依赖，测试里保持与生产同形状（String(error)）。
  asError: (error: unknown) => (error instanceof Error ? error.message : String(error)),
}));

const { runDailyActivity } = await import("@/lib/api");
const { useDailyActivity } = await import("./use-daily-activity");

/** 一轮"空账号库"的结果：不发请求、也不需要真实账号。 */
const EMPTY_RUN: DailyActivityRunResult = { date: "2026-11-22", accounts: [] };

/** 渲染一次 hook，返回捕获到的 controller（以及渲染期就能看到的初始快照）。 */
function mountController() {
  const captured: { current: ReturnType<typeof useDailyActivity> | null } = { current: null };
  function Probe() {
    captured.current = useDailyActivity();
    return null;
  }
  renderToStaticMarkup(createElement(Probe));
  const controller = captured.current;
  if (!controller) throw new Error("探针组件没有捕获到 controller");
  return controller;
}

describe("useDailyActivity 接线：只有确认后才发请求", () => {
  beforeEach(() => {
    vi.mocked(runDailyActivity).mockReset();
    vi.mocked(runDailyActivity).mockResolvedValue(EMPTY_RUN);
  });

  it("工具栏入口（request）只开确认框，不发请求", () => {
    const controller = mountController();

    controller.request();

    expect(runDailyActivity).not.toHaveBeenCalled();
  });

  it("取消（cancel）不发请求", () => {
    const controller = mountController();

    controller.cancel();

    expect(runDailyActivity).not.toHaveBeenCalled();
  });

  it("确认框「开始」（confirm）是唯一会下发命令的动作", async () => {
    const controller = mountController();

    controller.confirm();

    // confirm 是 fire-and-forget（`void runDailyActivityAttempt(...)`），命令在
    // 同步段里就已下发，这里再等一个微任务把 catch/finally 走完。
    await Promise.resolve();
    expect(runDailyActivity).toHaveBeenCalledTimes(1);
  });
});
