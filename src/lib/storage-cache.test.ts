import { beforeEach, describe, expect, it, vi } from "vitest";

// 只替换存储相关的入口：本模块不碰其它 api，避免把整个 barrel 都 mock 掉。
vi.mock("@/lib/api", () => ({
  storageScan: vi.fn(),
  storageSessions: vi.fn(),
  storageTrash: vi.fn(),
}));

import * as api from "@/lib/api";
import type { StorageScan, StorageSessionsPayload, StorageTrashPayload } from "@/lib/types";

import {
  __resetStorageCacheForTests,
  invalidateStorageCache,
  isStorageScanCacheFresh,
  isStorageSessionsCacheFresh,
  loadStorageScan,
  loadStorageSessions,
  loadStorageTrash,
  readStorageScanCache,
  readStorageSessionsCache,
  readStorageTrashCache,
  STORAGE_CACHE_TTL_MS,
} from "./storage-cache";

const storageScan = vi.mocked(api.storageScan);
const storageSessions = vi.mocked(api.storageSessions);
const storageTrash = vi.mocked(api.storageTrash);

/** 手写 deferred：制造「在途」状态，不依赖 fake timers。 */
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

function makeScan(generatedAt: number): StorageScan {
  return {
    generatedAt,
    variant: "cn",
    cached: false,
    roots: [{ id: "store", label: "工具数据", path: "/store", exists: true, bytes: 1, files: 1 }],
    totals: { bytes: 1, files: 1 },
    categories: [],
    sessions: {
      counts: {
        live: 0,
        softDeleted: 0,
        orphan: 0,
        redundant: 0,
        present: 0,
        partial: 0,
        bodyMissing: 0,
      },
      bytes: 0,
      files: 0,
    },
    trash: { entries: 0, bytes: 0, files: 0 },
    clientRunning: false,
    warnings: [],
  };
}

function makeSessions(): StorageSessionsPayload {
  return { variant: "cn", clientRunning: false, redundantCleanable: 0, sessions: [] };
}

function makeTrash(): StorageTrashPayload {
  return { entries: [], problems: [], bytes: 0, files: 0 };
}

beforeEach(() => {
  __resetStorageCacheForTests();
  storageScan.mockReset();
  storageSessions.mockReset();
  storageTrash.mockReset();
});

describe("存储数据缓存与 single-flight", () => {
  it("同一时刻的两次总览请求只发一次（后来者 join 在途）", async () => {
    const deferred = createDeferred<StorageScan>();
    storageScan.mockReturnValue(deferred.promise);

    const first = loadStorageScan();
    const second = loadStorageScan();
    deferred.resolve(makeScan(Date.now()));

    await expect(first).resolves.toBeTruthy();
    await expect(second).resolves.toBeTruthy();
    expect(storageScan).toHaveBeenCalledTimes(1);
  });

  it("force 绕过缓存值但仍然 join 在途请求", async () => {
    const deferred = createDeferred<StorageScan>();
    storageScan.mockReturnValue(deferred.promise);

    const first = loadStorageScan();
    const forced = loadStorageScan({ force: true });
    deferred.resolve(makeScan(Date.now()));
    await Promise.all([first, forced]);
    expect(storageScan).toHaveBeenCalledTimes(1);

    // 在途结束后 force 必须真的重新请求，并带 refresh=true。
    storageScan.mockResolvedValue(makeScan(Date.now()));
    await loadStorageScan({ force: true });
    expect(storageScan).toHaveBeenCalledTimes(2);
    expect(storageScan).toHaveBeenLastCalledWith(undefined, true);
  });

  it("缓存命中时不再请求", async () => {
    storageScan.mockResolvedValue(makeScan(Date.now()));
    await loadStorageScan();
    await loadStorageScan();
    expect(storageScan).toHaveBeenCalledTimes(1);
    expect(readStorageScanCache()).not.toBeNull();
  });

  it("失败不写缓存，且下一次可以重试", async () => {
    storageScan.mockRejectedValueOnce(new Error("扫描失败"));
    await expect(loadStorageScan()).rejects.toThrow("扫描失败");
    expect(readStorageScanCache()).toBeNull();

    storageScan.mockResolvedValueOnce(makeScan(Date.now()));
    await expect(loadStorageScan()).resolves.toBeTruthy();
    expect(storageScan).toHaveBeenCalledTimes(2);
  });

  it("总览新鲜度按响应自带 generatedAt 计算（60 秒）", async () => {
    const generatedAt = Date.now();
    storageScan.mockResolvedValue(makeScan(generatedAt));
    await loadStorageScan();

    expect(isStorageScanCacheFresh(undefined, generatedAt + STORAGE_CACHE_TTL_MS - 1)).toBe(true);
    expect(isStorageScanCacheFresh(undefined, generatedAt + STORAGE_CACHE_TTL_MS)).toBe(false);
  });

  it("会话资产与回收站各自独立缓存", async () => {
    storageSessions.mockResolvedValue(makeSessions());
    storageTrash.mockResolvedValue(makeTrash());

    await loadStorageSessions();
    await loadStorageSessions();
    await loadStorageTrash();
    await loadStorageTrash();

    expect(storageSessions).toHaveBeenCalledTimes(1);
    expect(storageTrash).toHaveBeenCalledTimes(1);
    expect(readStorageSessionsCache()).not.toBeNull();
    expect(readStorageTrashCache()).not.toBeNull();
  });

  it("会话缓存新鲜度用写入时刻（响应里没有 generatedAt）", async () => {
    storageSessions.mockResolvedValue(makeSessions());
    await loadStorageSessions();
    expect(isStorageSessionsCacheFresh()).toBe(true);
    expect(isStorageSessionsCacheFresh(Date.now() + STORAGE_CACHE_TTL_MS + 1)).toBe(false);
  });

  it("invalidate 只清指定 scope", async () => {
    storageScan.mockResolvedValue(makeScan(Date.now()));
    storageSessions.mockResolvedValue(makeSessions());
    storageTrash.mockResolvedValue(makeTrash());
    await Promise.all([loadStorageScan(), loadStorageSessions(), loadStorageTrash()]);

    invalidateStorageCache("sessions");
    expect(readStorageScanCache()).not.toBeNull();
    expect(readStorageSessionsCache()).toBeNull();
    expect(readStorageTrashCache()).not.toBeNull();

    invalidateStorageCache("all");
    expect(readStorageScanCache()).toBeNull();
    expect(readStorageTrashCache()).toBeNull();
  });

  it("清理后（invalidate all）再读必须重新请求", async () => {
    storageScan.mockResolvedValue(makeScan(Date.now()));
    await loadStorageScan();
    invalidateStorageCache("all");
    await loadStorageScan();
    expect(storageScan).toHaveBeenCalledTimes(2);
  });
});
