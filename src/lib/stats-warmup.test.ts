import { beforeEach, describe, expect, it, vi } from "vitest";

// 纯编排层不发请求；这里 mock 掉 api 只为一件事：直接给「真实 Token 缓存」播种
// （验证 10 分钟新鲜窗口命中），以及避免意外触到真实 invoke。
vi.mock("@/lib/api", () => ({
  getTokenStatistics: vi.fn(),
  getCreditStatistics: vi.fn(),
  isDemoMode: () => false,
  isWebui: () => true,
  asError: (cause: unknown) => (cause instanceof Error ? cause.message : String(cause)),
}));

import { __resetWarmupScheduleForTests, scheduleStatsWarmup } from "@/hooks/use-stats-warmup";
import * as api from "@/lib/api";
import type { TokenStatistics } from "@/lib/types";

import { __resetStatsCacheForTests, loadTokenStatistics, TOKEN_CACHE_TTL_MS } from "./stats-cache";
import {
  OFFICIAL_USAGE_TTL_MS,
  runStatsWarmup,
  type WarmupDeps,
  type WarmupResult,
} from "./stats-warmup";

const getTokenStatistics = vi.mocked(api.getTokenStatistics);
const NOW = 1_700_000_000_000;

/** 手写 deferred：用于断言「谁先发、谁还在途」。 */
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

/** 让已排队的微任务与一次宏任务跑完（不使用 fake timers，避免与 promise 链互相干扰）。 */
function flush(): Promise<void> {
  return new Promise<void>((resolve) => {
    setTimeout(resolve, 0);
  });
}

function emptyResult(): WarmupResult {
  return { credit: "skipped", token: "skipped", official: "skipped" };
}

interface FakeOptions {
  isDemo?: () => boolean;
  isVisible?: () => boolean;
  /** 账号数量，缺省 1（有账号）。 */
  accounts?: number;
  /** 官方用量采集时刻，缺省 null（视为需要补采）。 */
  collectedAt?: number | null;
  loadCredit?: (refresh?: boolean) => Promise<unknown>;
  loadToken?: () => Promise<unknown>;
  refreshOfficialUsage?: () => Promise<unknown>;
}

/** 构造 fake deps，并用 `calls` 记录每一处外部调用的发生顺序。 */
function createFakeDeps(calls: string[], options: FakeOptions = {}): WarmupDeps {
  const accountCount = options.accounts ?? 1;
  const collectedAt = options.collectedAt === undefined ? null : options.collectedAt;
  return {
    isDemo: options.isDemo ?? (() => false),
    isVisible: options.isVisible ?? (() => true),
    fetchAccounts: () => {
      calls.push("accounts");
      return Promise.resolve();
    },
    hasAccountsInflight: () => false,
    getAccounts: () => ({ length: accountCount }),
    getOfficialCollectedAt: () => collectedAt,
    loadCredit:
      options.loadCredit ??
      ((refresh = false) => {
        calls.push(`credit:${refresh}`);
        return Promise.resolve(null);
      }),
    loadToken:
      options.loadToken ??
      (() => {
        calls.push("token");
        return Promise.resolve(null);
      }),
    refreshOfficialUsage:
      options.refreshOfficialUsage ??
      (() => {
        calls.push("official");
        return Promise.resolve(null);
      }),
    now: () => NOW,
    prefetchRoutes: () => {
      calls.push("prefetch");
    },
  };
}

beforeEach(() => {
  __resetStatsCacheForTests();
  __resetWarmupScheduleForTests();
  getTokenStatistics.mockReset();
});

describe("runStatsWarmup 顺序与并行", () => {
  it("本地积分先完成；随后 token 与官方采集并行发出", async () => {
    const calls: string[] = [];
    const credit = createDeferred<unknown>();
    const token = createDeferred<unknown>();
    const official = createDeferred<unknown>();
    const deps = createFakeDeps(calls, {
      loadCredit: (refresh = false) => {
        calls.push(`credit:${refresh}`);
        return refresh ? Promise.resolve(null) : credit.promise;
      },
      loadToken: () => {
        calls.push("token");
        return token.promise;
      },
      refreshOfficialUsage: () => {
        calls.push("official");
        return official.promise;
      },
    });

    const pending = runStatsWarmup(deps);
    await flush();
    // 积分还没回来之前，token 与官方都不得启动。
    expect(calls).toEqual(["accounts", "credit:false"]);

    credit.resolve(null);
    await flush();
    // 积分完成 → token 与官方同一轮内先后发出（token 未 await，两者并行）。
    expect(calls).toEqual(["accounts", "credit:false", "token", "official"]);

    official.resolve(null);
    await flush();
    // 官方补采完成后回读积分（回填），此时 token 仍在途。
    expect(calls).toEqual(["accounts", "credit:false", "token", "official", "credit:false"]);

    token.resolve(null);
    const result = await pending;
    expect(result).toEqual({ credit: "refreshed", token: "loaded", official: "refreshed" });
    expect(calls).toEqual(["accounts", "credit:false", "token", "official", "credit:false", "prefetch"]);
  });

  it("预热入口永不 reject：token 失败收敛为 failed", async () => {
    const calls: string[] = [];
    const deps = createFakeDeps(calls, {
      loadToken: () => Promise.reject(new Error("scan failed")),
    });

    const result = await runStatsWarmup(deps);
    expect(result.token).toBe("failed");
    expect(result.credit).toBe("refreshed");
    // 失败不置完成标记、不抛错；页面自己还有加载与重试路径。
    expect(calls).toContain("prefetch");
  });

  it("账号拉取失败不阻塞预热", async () => {
    const calls: string[] = [];
    const base = createFakeDeps(calls, { accounts: 0 });
    const deps: WarmupDeps = {
      ...base,
      fetchAccounts: () => Promise.reject(new Error("store failed")),
    };

    const result = await runStatsWarmup(deps);
    expect(result).toEqual({ credit: "refreshed", token: "loaded", official: "skipped" });
  });
});

describe("runStatsWarmup 官方用量 30 分钟门控", () => {
  it("采集时刻新鲜时不补采", async () => {
    const calls: string[] = [];
    const deps = createFakeDeps(calls, { collectedAt: NOW - 1_000 });

    const result = await runStatsWarmup(deps);
    expect(result.official).toBe("fresh");
    expect(calls).not.toContain("official");
    // 只有一次积分读取，没有回填。
    expect(calls.filter((call) => call === "credit:false")).toHaveLength(1);
  });

  it("采集时刻过期时恰好补采一次", async () => {
    const calls: string[] = [];
    const refreshOfficialUsage = vi.fn(() => Promise.resolve(null));
    const deps = createFakeDeps(calls, {
      collectedAt: NOW - OFFICIAL_USAGE_TTL_MS - 1,
      refreshOfficialUsage,
    });

    const result = await runStatsWarmup(deps);
    expect(result.official).toBe("refreshed");
    expect(refreshOfficialUsage).toHaveBeenCalledTimes(1);
    expect(calls.filter((call) => call === "credit:false")).toHaveLength(2);
  });

  it("尚无采集时刻（collectedAt 为 null）时补采", async () => {
    const refreshOfficialUsage = vi.fn(() => Promise.resolve(null));
    const deps = createFakeDeps([], { collectedAt: null, refreshOfficialUsage });

    await runStatsWarmup(deps);
    expect(refreshOfficialUsage).toHaveBeenCalledTimes(1);
  });

  it("补采失败时收敛为 skipped 且不抛错", async () => {
    const deps = createFakeDeps([], {
      refreshOfficialUsage: () => Promise.reject(new Error("official failed")),
    });

    const result = await runStatsWarmup(deps);
    expect(result.official).toBe("skipped");
    expect(result.token).toBe("loaded");
  });

  it("没有账号时跳过官方，本地积分与 token 照常", async () => {
    const calls: string[] = [];
    const refreshOfficialUsage = vi.fn(() => Promise.resolve(null));
    const deps = createFakeDeps(calls, { accounts: 0, refreshOfficialUsage });

    const result = await runStatsWarmup(deps);
    expect(result).toEqual({ credit: "refreshed", token: "loaded", official: "skipped" });
    expect(refreshOfficialUsage).not.toHaveBeenCalled();
    expect(calls).toContain("token");
  });
});

describe("runStatsWarmup 跳过条件", () => {
  it("演示模式：全部 skipped 且零调用", async () => {
    const calls: string[] = [];
    const deps = createFakeDeps(calls, { isDemo: () => true });

    const result = await runStatsWarmup(deps);
    expect(result).toEqual({ credit: "skipped", token: "skipped", official: "skipped" });
    expect(calls).toEqual([]);
  });

  it("主窗口不可见：全部 skipped、不发请求、不预取", async () => {
    const calls: string[] = [];
    const deps = createFakeDeps(calls, { isVisible: () => false });

    const result = await runStatsWarmup(deps);
    expect(result).toEqual({ credit: "skipped", token: "skipped", official: "skipped" });
    expect(calls).toEqual([]);
  });
});

describe("runStatsWarmup Token 新鲜窗口", () => {
  it("缓存 10 分钟内直接命中，不发请求", async () => {
    getTokenStatistics.mockResolvedValueOnce({
      generatedAt: NOW - TOKEN_CACHE_TTL_MS + 1_000,
      sources: [],
    } satisfies TokenStatistics);
    await loadTokenStatistics();

    const calls: string[] = [];
    const loadToken = vi.fn(() => Promise.resolve(null));
    const deps = createFakeDeps(calls, { accounts: 0, collectedAt: NOW, loadToken });

    const result = await runStatsWarmup(deps);
    expect(result.token).toBe("hit");
    expect(loadToken).not.toHaveBeenCalled();
    // 缓存命中时不计入「本次加载」，也不影响积分与官方的结果。
    expect(result.credit).toBe("refreshed");
  });

  it("缓存超过 10 分钟则重新加载", async () => {
    getTokenStatistics.mockResolvedValueOnce({
      generatedAt: NOW - TOKEN_CACHE_TTL_MS - 1,
      sources: [],
    } satisfies TokenStatistics);
    await loadTokenStatistics();

    const loadToken = vi.fn(() => Promise.resolve(null));
    const deps = createFakeDeps([], { accounts: 0, collectedAt: NOW, loadToken });

    const result = await runStatsWarmup(deps);
    expect(result.token).toBe("loaded");
    expect(loadToken).toHaveBeenCalledTimes(1);
  });
});

describe("scheduleStatsWarmup 调度", () => {
  it("重复调度只执行一次预热", () => {
    const run = vi.fn(async (): Promise<WarmupResult> => emptyResult());
    const deps = createFakeDeps([]);
    const start = (): (() => void) =>
      scheduleStatsWarmup({
        run,
        resolveDeps: () => deps,
        isDemo: () => false,
        isVisible: () => true,
        subscribeVisibility: () => () => {},
        scheduleIdle: (callback) => {
          callback();
          return () => {};
        },
      });

    const cleanupFirst = start();
    const cleanupSecond = start();
    expect(run).toHaveBeenCalledTimes(1);

    cleanupFirst();
    cleanupSecond();
    start(); // 已真正执行过：cleanup 不重置模块级 scheduled
    expect(run).toHaveBeenCalledTimes(1);
  });

  it("不可见时不调度，恢复可见后只执行一次", () => {
    const holder: { onVisible: (() => void) | null } = { onVisible: null };
    const run = vi.fn(async (): Promise<WarmupResult> => emptyResult());
    const deps = createFakeDeps([]);
    const cleanup = scheduleStatsWarmup({
      run,
      resolveDeps: () => deps,
      isDemo: () => false,
      isVisible: () => false,
      subscribeVisibility: (onVisible) => {
        holder.onVisible = onVisible;
        return () => {
          holder.onVisible = null;
        };
      },
      scheduleIdle: (callback) => {
        callback();
        return () => {};
      },
    });

    expect(run).not.toHaveBeenCalled();
    expect(holder.onVisible).not.toBeNull();

    holder.onVisible?.();
    expect(run).toHaveBeenCalledTimes(1);

    holder.onVisible?.(); // 已经启动过：可见性重复事件不再执行
    expect(run).toHaveBeenCalledTimes(1);

    cleanup();
    expect(holder.onVisible).toBeNull();
  });

  it("演示模式：不调度、不预取", () => {
    const run = vi.fn(async (): Promise<WarmupResult> => emptyResult());
    const deps = createFakeDeps([]);
    const cleanup = scheduleStatsWarmup({
      run,
      resolveDeps: () => deps,
      isDemo: () => true,
      isVisible: () => true,
      subscribeVisibility: () => () => {},
      scheduleIdle: (callback) => {
        callback();
        return () => {};
      },
    });

    expect(run).not.toHaveBeenCalled();
    cleanup();
  });

  it("空闲回调未执行前被清理（StrictMode 假卸载）后仍能重新调度一次", () => {
    const run = vi.fn(async (): Promise<WarmupResult> => emptyResult());
    const deps = createFakeDeps([]);

    // 第一次挂载：空闲回调还没跑就被卸载（StrictMode 的立即清理）。
    const cleanupFirst = scheduleStatsWarmup({
      run,
      resolveDeps: () => deps,
      isDemo: () => false,
      isVisible: () => true,
      subscribeVisibility: () => () => {},
      scheduleIdle: () => () => {},
    });
    expect(run).not.toHaveBeenCalled();
    cleanupFirst();

    // 第二次挂载：允许重新排队，最终只执行一次。
    scheduleStatsWarmup({
      run,
      resolveDeps: () => deps,
      isDemo: () => false,
      isVisible: () => true,
      subscribeVisibility: () => () => {},
      scheduleIdle: (callback) => {
        callback();
        return () => {};
      },
    });
    expect(run).toHaveBeenCalledTimes(1);
  });
});
