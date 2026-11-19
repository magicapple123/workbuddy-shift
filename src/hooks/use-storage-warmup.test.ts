import { beforeEach, describe, expect, it, vi } from "vitest";

import {
  __resetStorageWarmupForTests,
  runStorageWarmup,
  scheduleStorageWarmup,
  type StorageWarmupDeps,
} from "@/hooks/use-storage-warmup";

/** 手写 deferred：精确控制「统计预热什么时候结束」。 */
function createDeferred(): { promise: Promise<void>; resolve: () => void } {
  let resolve: () => void = () => {};
  const promise = new Promise<void>((res) => {
    resolve = res;
  });
  return { promise, resolve };
}

function deps(overrides: Partial<StorageWarmupDeps> = {}): StorageWarmupDeps {
  return {
    isDemo: () => false,
    isVisible: () => true,
    subscribeVisibility: () => () => {},
    scheduleIdle: (callback) => {
      callback();
      return () => {};
    },
    waitForStats: () => Promise.resolve(),
    hasFreshCache: () => false,
    loadScan: () => Promise.resolve(),
    ...overrides,
  };
}

beforeEach(() => {
  __resetStorageWarmupForTests();
});

describe("存储总览预热", () => {
  it("演示模式直接跳过，不发任何请求", async () => {
    const loadScan = vi.fn(() => Promise.resolve());
    await expect(runStorageWarmup(deps({ isDemo: () => true, loadScan }))).resolves.toBe("skipped");
    expect(loadScan).not.toHaveBeenCalled();
  });

  it("缓存已新鲜时返回 hit，不重复扫描", async () => {
    const loadScan = vi.fn(() => Promise.resolve());
    await expect(
      runStorageWarmup(deps({ hasFreshCache: () => true, loadScan })),
    ).resolves.toBe("hit");
    expect(loadScan).not.toHaveBeenCalled();
  });

  it("先等统计预热结束，再读总览（统计优先）", async () => {
    const stats = createDeferred();
    const order: string[] = [];
    const loadScan = vi.fn(() => {
      order.push("scan");
      return Promise.resolve();
    });

    const pending = runStorageWarmup(
      deps({
        waitForStats: () => {
          order.push("stats");
          return stats.promise;
        },
        loadScan,
      }),
    );
    // 统计还没结束，总览不能开跑。
    await Promise.resolve();
    expect(order).toEqual(["stats"]);

    stats.resolve();
    await expect(pending).resolves.toBe("loaded");
    expect(order).toEqual(["stats", "scan"]);
  });

  it("统计预热卡住时按兜底超时继续（不会永远不预热）", async () => {
    const loadScan = vi.fn(() => Promise.resolve());
    const state = await runStorageWarmup(
      deps({
        waitForStats: () => new Promise<void>(() => {}),
        loadScan,
        statsTimeoutMs: 5,
      }),
    );
    expect(state).toBe("loaded");
    expect(loadScan).toHaveBeenCalledTimes(1);
  });

  it("预热失败静默返回 failed，不抛给调用方", async () => {
    const consoleInfo = vi.spyOn(console, "info").mockImplementation(() => {});
    await expect(
      runStorageWarmup(deps({ loadScan: () => Promise.reject(new Error("扫描失败")) })),
    ).resolves.toBe("failed");
    consoleInfo.mockRestore();
  });

  it("调度只生效一次（StrictMode 双挂载不会预热两遍）", async () => {
    const loadScan = vi.fn(() => Promise.resolve());
    const cleanup = scheduleStorageWarmup(
      deps({ loadScan, scheduleIdle: (callback) => (callback(), () => {}) }),
    );
    const secondCleanup = scheduleStorageWarmup(deps({ loadScan }));
    cleanup();
    secondCleanup();
    await vi.waitFor(() => {
      expect(loadScan).toHaveBeenCalledTimes(1);
    });
  });

  it("演示模式下调度也不发请求", async () => {
    const loadScan = vi.fn(() => Promise.resolve());
    const cleanup = scheduleStorageWarmup(deps({ isDemo: () => true, loadScan }));
    cleanup();
    expect(loadScan).not.toHaveBeenCalled();
  });
});
