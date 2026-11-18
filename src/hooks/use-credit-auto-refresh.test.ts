import { describe, expect, it } from "vitest";

import {
  CREDIT_REFRESH_INTERVAL_MS,
  resolveCreditRefreshAction,
} from "./use-credit-auto-refresh";
import { WORKBUDDY_STATUS_REFRESH_INTERVAL_MS } from "./use-workbuddy-status-refresh";

describe("可见性轮询的间隔与刷新计划", () => {
  it("积分刷新间隔保持 30 分钟", () => {
    expect(CREDIT_REFRESH_INTERVAL_MS).toBe(30 * 60 * 1000);
  });

  it("WorkBuddy 状态刷新间隔保持 60 秒", () => {
    expect(WORKBUDDY_STATUS_REFRESH_INTERVAL_MS).toBe(60 * 1000);
  });

  it("从未刷新过时进入可见只补拉缺失积分", () => {
    expect(resolveCreditRefreshAction(0, 1_700_000_000_000)).toBe("ensure");
  });

  it("距上次刷新不足一个间隔时切回页面不重复请求", () => {
    const now = 1_700_000_000_000;
    expect(resolveCreditRefreshAction(now - CREDIT_REFRESH_INTERVAL_MS + 1, now)).toBe("none");
  });

  it("距上次刷新满一个间隔时静默刷新", () => {
    const now = 1_700_000_000_000;
    expect(resolveCreditRefreshAction(now - CREDIT_REFRESH_INTERVAL_MS, now)).toBe("refresh");
    expect(resolveCreditRefreshAction(now - CREDIT_REFRESH_INTERVAL_MS * 2, now)).toBe("refresh");
  });
});
