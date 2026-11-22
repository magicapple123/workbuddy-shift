import { describe, expect, it } from "vitest";

import {
  RATE_LIMIT_FETCH_FLOOR_MS,
  RATE_LIMIT_REFRESH_INTERVAL_MS,
  shouldFetchRateLimits,
} from "./rate-limit-throttle";

describe("shouldFetchRateLimits", () => {
  it("首次拉取直接允许（进页面就要有台账）", () => {
    expect(shouldFetchRateLimits(1_000, 0)).toBe(true);
    expect(shouldFetchRateLimits(1_000, -1)).toBe(true);
  });

  it("防抖下限内不重复拉取", () => {
    const last = 10_000_000;
    expect(shouldFetchRateLimits(last + 0, last)).toBe(false);
    expect(shouldFetchRateLimits(last + 1_000, last)).toBe(false);
    expect(shouldFetchRateLimits(last + RATE_LIMIT_FETCH_FLOOR_MS - 1, last)).toBe(
      false,
    );
  });

  it("过了防抖下限但未满刷新间隔仍不拉取", () => {
    const last = 10_000_000;
    expect(shouldFetchRateLimits(last + RATE_LIMIT_FETCH_FLOOR_MS, last)).toBe(false);
    expect(
      shouldFetchRateLimits(last + RATE_LIMIT_REFRESH_INTERVAL_MS - 1, last),
    ).toBe(false);
  });

  it("满 1 分钟允许再次拉取", () => {
    const last = 10_000_000;
    expect(shouldFetchRateLimits(last + RATE_LIMIT_REFRESH_INTERVAL_MS, last)).toBe(
      true,
    );
    expect(
      shouldFetchRateLimits(last + RATE_LIMIT_REFRESH_INTERVAL_MS * 3, last),
    ).toBe(true);
  });

  it("force 忽略节流（事件推送路径）", () => {
    const last = 10_000_000;
    expect(shouldFetchRateLimits(last + 1, last, true)).toBe(true);
    expect(shouldFetchRateLimits(last, last, true)).toBe(true);
  });

  it("轮询间隔必须比刷新间隔更密，否则每次 tick 都刚好差一点", () => {
    expect(RATE_LIMIT_FETCH_FLOOR_MS).toBeLessThan(RATE_LIMIT_REFRESH_INTERVAL_MS);
  });
});
