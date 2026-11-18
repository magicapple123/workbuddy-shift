import { beforeEach, describe, expect, it, vi } from "vitest";

// 只替换统计相关的两个入口：本模块不碰其它 api，避免把整个 barrel 都 mock 掉。
vi.mock("@/lib/api", () => ({
  getTokenStatistics: vi.fn(),
  getCreditStatistics: vi.fn(),
}));

import * as api from "@/lib/api";
import type { CreditStatistics, TokenStatistics } from "@/lib/types";

import {
  __resetStatsCacheForTests,
  invalidateCreditStatisticsCache,
  isTokenStatisticsCacheFresh,
  loadCreditStatistics,
  loadTokenStatistics,
  readCreditStatisticsCache,
  readTokenStatisticsCache,
  TOKEN_CACHE_TTL_MS,
} from "./stats-cache";

const getTokenStatistics = vi.mocked(api.getTokenStatistics);
const getCreditStatistics = vi.mocked(api.getCreditStatistics);

/** 手写 deferred：用来制造「在途」状态（fake timers 不参与并发断言）。 */
function createDeferred<T>(): {
  promise: Promise<T>;
  resolve: (value: T) => void;
  reject: (cause: unknown) => void;
} {
  let resolve: (value: T) => void = () => {};
  let reject: (cause: unknown) => void = () => {};
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

function makeTokenStatistics(generatedAt: number): TokenStatistics {
  return { generatedAt, sources: [] };
}

function makeCreditStatistics(generatedAt: number): CreditStatistics {
  return {
    generatedAt,
    retentionDays: 30,
    coverageStartAt: null,
    summary: {
      currentRemaining: 0,
      currentCapacity: 0,
      usageToday: 0,
      usage7Days: 0,
      usageThisMonth: 0,
      todayCheckedInAccounts: 0,
      todaySuccess: 0,
      todayAlready: 0,
      todayFailed: 0,
    },
    daily: [],
    accounts: [],
    events: [],
  };
}

beforeEach(() => {
  __resetStatsCacheForTests();
  getTokenStatistics.mockReset();
  getCreditStatistics.mockReset();
});

describe("Token 统计缓存", () => {
  it("成功结果写入缓存，第二次调用不再发请求", async () => {
    const value = makeTokenStatistics(1_000);
    getTokenStatistics.mockResolvedValue(value);

    expect(readTokenStatisticsCache()).toBeNull();

    const first = await loadTokenStatistics();
    expect(first).toBe(value);
    expect(readTokenStatisticsCache()).toBe(value);

    const second = await loadTokenStatistics();
    expect(second).toBe(value);
    expect(getTokenStatistics).toHaveBeenCalledTimes(1);
  });

  it("在途请求被后来者 join：并发两次只调用一次接口（force 也一样）", async () => {
    const deferred = createDeferred<TokenStatistics>();
    getTokenStatistics.mockReturnValue(deferred.promise);

    const plain = loadTokenStatistics();
    const forced = loadTokenStatistics({ force: true });
    expect(getTokenStatistics).toHaveBeenCalledTimes(1);

    const value = makeTokenStatistics(2_000);
    deferred.resolve(value);

    await expect(plain).resolves.toBe(value);
    await expect(forced).resolves.toBe(value);
    expect(getTokenStatistics).toHaveBeenCalledTimes(1);
    expect(readTokenStatisticsCache()).toBe(value);
  });

  it("失败不写缓存、清空在途，下一次调用可以重试", async () => {
    getTokenStatistics.mockRejectedValueOnce(new Error("scan failed"));

    await expect(loadTokenStatistics()).rejects.toThrow("scan failed");
    expect(readTokenStatisticsCache()).toBeNull();

    const value = makeTokenStatistics(3_000);
    getTokenStatistics.mockResolvedValueOnce(value);

    await expect(loadTokenStatistics()).resolves.toBe(value);
    expect(getTokenStatistics).toHaveBeenCalledTimes(2);
    expect(readTokenStatisticsCache()).toBe(value);
  });

  it("失败后重新发起的是新请求（在途已被清空，不会 join 旧的 rejection）", async () => {
    const deferred = createDeferred<TokenStatistics>();
    getTokenStatistics.mockReturnValueOnce(deferred.promise);

    const first = loadTokenStatistics();
    deferred.reject(new Error("boom"));
    await expect(first).rejects.toThrow("boom");

    getTokenStatistics.mockResolvedValueOnce(makeTokenStatistics(4_000));
    const second = loadTokenStatistics();
    expect(getTokenStatistics).toHaveBeenCalledTimes(2);
    await expect(second).resolves.toMatchObject({ generatedAt: 4_000 });
  });

  it("force 绕过已缓存值重新请求，之后默认调用复用最新缓存", async () => {
    const first = makeTokenStatistics(5_000);
    getTokenStatistics.mockResolvedValueOnce(first);
    await loadTokenStatistics();

    const second = makeTokenStatistics(6_000);
    getTokenStatistics.mockResolvedValueOnce(second);

    await expect(loadTokenStatistics({ force: true })).resolves.toBe(second);
    expect(getTokenStatistics).toHaveBeenCalledTimes(2);
    expect(readTokenStatisticsCache()).toBe(second);

    await expect(loadTokenStatistics()).resolves.toBe(second);
    expect(getTokenStatistics).toHaveBeenCalledTimes(2);
  });
  it("新鲜窗口按 generatedAt 判定：10 分钟内新鲜、之后过期", async () => {
    expect(isTokenStatisticsCacheFresh(1_000)).toBe(false);

    const generatedAt = 1_000_000;
    getTokenStatistics.mockResolvedValueOnce(makeTokenStatistics(generatedAt));
    await loadTokenStatistics();

    expect(isTokenStatisticsCacheFresh(generatedAt + TOKEN_CACHE_TTL_MS - 1)).toBe(true);
    expect(isTokenStatisticsCacheFresh(generatedAt + TOKEN_CACHE_TTL_MS)).toBe(false);
  });
});

describe("积分统计缓存", () => {
  it("缓存命中时不发请求，refresh=true 才重新采集", async () => {
    const cached = makeCreditStatistics(10_000);
    getCreditStatistics.mockResolvedValueOnce(cached);

    await expect(loadCreditStatistics(false)).resolves.toBe(cached);
    await expect(loadCreditStatistics(false)).resolves.toBe(cached);
    expect(getCreditStatistics).toHaveBeenCalledTimes(1);
    expect(readCreditStatisticsCache()).toBe(cached);
    expect(getCreditStatistics).toHaveBeenLastCalledWith(false);

    const refreshed = makeCreditStatistics(20_000);
    getCreditStatistics.mockResolvedValueOnce(refreshed);
    await expect(loadCreditStatistics(true)).resolves.toBe(refreshed);
    expect(getCreditStatistics).toHaveBeenCalledTimes(2);
    expect(getCreditStatistics).toHaveBeenLastCalledWith(true);
    expect(readCreditStatisticsCache()).toBe(refreshed);
  });

  it("refresh=true 与 refresh=false 的在途请求互不 join", async () => {
    const refreshing = createDeferred<CreditStatistics>();
    const plain = createDeferred<CreditStatistics>();
    getCreditStatistics.mockReturnValueOnce(refreshing.promise).mockReturnValueOnce(plain.promise);

    const forced = loadCreditStatistics(true);
    const shared = loadCreditStatistics(false);
    expect(getCreditStatistics).toHaveBeenCalledTimes(2);

    // 同一 key 的第二次调用仍然 join。
    const forcedAgain = loadCreditStatistics(true);
    const sharedAgain = loadCreditStatistics(false);
    expect(getCreditStatistics).toHaveBeenCalledTimes(2);

    const plainValue = makeCreditStatistics(11_000);
    plain.resolve(plainValue);
    await expect(shared).resolves.toBe(plainValue);
    await expect(sharedAgain).resolves.toBe(plainValue);

    const forcedValue = makeCreditStatistics(22_000);
    refreshing.resolve(forcedValue);
    await expect(forced).resolves.toBe(forcedValue);
    await expect(forcedAgain).resolves.toBe(forcedValue);
  });

  it("失败不写缓存且可重试", async () => {
    getCreditStatistics.mockRejectedValueOnce(new Error("stats failed"));
    await expect(loadCreditStatistics(false)).rejects.toThrow("stats failed");
    expect(readCreditStatisticsCache()).toBeNull();

    const value = makeCreditStatistics(30_000);
    getCreditStatistics.mockResolvedValueOnce(value);
    await expect(loadCreditStatistics(false)).resolves.toBe(value);
    expect(getCreditStatistics).toHaveBeenCalledTimes(2);
  });

  it("invalidate 后重新回读（预热回填官方用量用）", async () => {
    const first = makeCreditStatistics(40_000);
    getCreditStatistics.mockResolvedValueOnce(first);
    await loadCreditStatistics(false);

    invalidateCreditStatisticsCache();
    expect(readCreditStatisticsCache()).toBeNull();

    const second = makeCreditStatistics(50_000);
    getCreditStatistics.mockResolvedValueOnce(second);
    await expect(loadCreditStatistics(false)).resolves.toBe(second);
    expect(getCreditStatistics).toHaveBeenCalledTimes(2);
    expect(readCreditStatisticsCache()).toBe(second);
  });
});
