/**
 * 存储管理数据的进程内 single-flight + 成功结果缓存（第三轮第 6 条 6.4）。
 *
 * 背景：存储页的三个接口（总览 / 会话资产 / 回收站）都很贵——总览要遍历
 * `~/.workbuddy` 的元数据，会话资产要读 `projects/**` 与四类索引。改造前页面挂载时
 * `Promise.all` 三个一起发、全部完成才渲染，且启动时没有任何预热；用户来回切页签就把
 * 同一份扫描重复跑几遍。
 *
 * 语义与 `@/lib/stats-cache` 保持一致：
 * - 同一 key 在途时，后来者 join **同一个** Promise，绝不重复请求（`force` 也一样）；
 * - 只缓存**成功**结果，并记录写入时刻 `at = Date.now()`；
 * - 失败时清空在途、不写缓存、错误原样抛给调用方（页面据此展示重试入口）；
 * - TTL 60 秒（与服务端快照一致）；页面首帧用缓存值，超过 TTL 后台刷新
 *   （stale-while-revalidate）。
 *
 * 依赖方向：`src/lib/*.ts` 纯逻辑层，只依赖 `@/lib/api` 与 `@/lib/types`，不引入 React。
 */

import * as api from "@/lib/api";
import type { StorageScan, StorageSessionsPayload, StorageTrashPayload, WbVariant } from "@/lib/types";

/** 缓存新鲜窗口：与服务端快照 TTL 一致。 */
export const STORAGE_CACHE_TTL_MS = 60 * 1000;

/** 缓存作用域：`all` 表示三份数据全清。 */
export type StorageCacheScope = "scan" | "sessions" | "trash" | "all";

/** 缓存项：`at` 是写入缓存的时刻（毫秒）。 */
interface CacheEntry<T> {
  value: T;
  at: number;
}

/** 三个接口各自一份缓存；键里带档位，切国内 / 国际不会串号。 */
interface Slot<T> {
  entry: CacheEntry<T> | null;
  inflight: Map<string, Promise<T>>;
}

function emptySlot<T>(): Slot<T> {
  return { entry: null, inflight: new Map() };
}

const scanSlot = emptySlot<StorageScan>();
const sessionsSlot = emptySlot<StorageSessionsPayload>();
const trashSlot = emptySlot<StorageTrashPayload>();

/** 档位键：缺省（国内版）与 `ai` 分开存。 */
function variantKey(variant?: WbVariant): string {
  return variant === "ai" ? "ai" : "cn";
}

/**
 * 读缓存：`null` 表示还没有成功结果。
 *
 * 只返回**任意年龄**的值（页面首帧先用它渲染），是否新鲜用
 * [`isStorageScanCacheFresh`] 判断。
 */
export function readStorageScanCache(variant?: WbVariant): StorageScan | null {
  return scanSlot.entry !== null && variantKey(variant) === scanSlot.entry.value.variant
    ? scanSlot.entry.value
    : null;
}

export function readStorageSessionsCache(variant?: WbVariant): StorageSessionsPayload | null {
  const entry = sessionsSlot.entry;
  if (entry === null) return null;
  return variantKey(variant) === entry.value.variant ? entry.value : null;
}

export function readStorageTrashCache(): StorageTrashPayload | null {
  return trashSlot.entry === null ? null : trashSlot.entry.value;
}

/** 总览缓存是否仍在 60 秒新鲜窗口内（页面据此决定要不要后台刷新）。 */
export function isStorageScanCacheFresh(
  variant?: WbVariant,
  now = Date.now(),
): boolean {
  const value = readStorageScanCache(variant);
  if (value === null) return false;
  return now - value.generatedAt < STORAGE_CACHE_TTL_MS;
}

/** 会话资产缓存是否新鲜（用写入时刻，响应里没有 `generatedAt`）。 */
export function isStorageSessionsCacheFresh(now = Date.now()): boolean {
  if (sessionsSlot.entry === null) return false;
  return now - sessionsSlot.entry.at < STORAGE_CACHE_TTL_MS;
}

/** 通用 single-flight：`load` 只在没有在途请求且（无缓存或 `force`）时真正发请求。 */
function loadWithCache<T>(
  slot: Slot<T>,
  key: string,
  force: boolean,
  fetch: () => Promise<T>,
): Promise<T> {
  const inflight = slot.inflight.get(key);
  // 有在途请求就 join：`force` 表示「不要拿旧缓存」，不代表「再扫一遍盘」。
  if (inflight !== undefined) return inflight;
  if (!force && slot.entry !== null) return Promise.resolve(slot.entry.value);

  const request = fetch().then((value) => {
    slot.entry = { value, at: Date.now() };
    return value;
  });
  slot.inflight.set(key, request);
  const settle = (): void => {
    if (slot.inflight.get(key) === request) slot.inflight.delete(key);
  };
  // 失败路径只做清理、不吞错误：调用方拿到的仍是原始 rejection，下次可以重试。
  request.then(settle, settle);
  return request;
}

/** 读总览；`force` 忽略缓存值（在途请求仍然 join）。 */
export function loadStorageScan(opts?: {
  force?: boolean;
  variant?: WbVariant;
}): Promise<StorageScan> {
  const variant = opts?.variant;
  return loadWithCache(scanSlot, variantKey(variant), opts?.force === true, () =>
    api.storageScan(variant, opts?.force === true),
  );
}

/** 读会话资产（只在切到「会话资产」页签或空闲预取时调用）。 */
export function loadStorageSessions(opts?: {
  force?: boolean;
  variant?: WbVariant;
}): Promise<StorageSessionsPayload> {
  const variant = opts?.variant;
  return loadWithCache(sessionsSlot, variantKey(variant), opts?.force === true, () =>
    api.storageSessions(variant),
  );
}

/** 读回收站。 */
export function loadStorageTrash(opts?: { force?: boolean }): Promise<StorageTrashPayload> {
  return loadWithCache(trashSlot, "default", opts?.force === true, () => api.storageTrash());
}

/**
 * 丢弃缓存值（在途请求不受影响）。
 *
 * 清理 / 恢复 / 清空回收站之后必须调用：服务端快照已被作废，客户端不能让用户继续
 * 看到几秒前的旧数字。
 */
export function invalidateStorageCache(scope: StorageCacheScope = "all"): void {
  if (scope === "scan" || scope === "all") scanSlot.entry = null;
  if (scope === "sessions" || scope === "all") sessionsSlot.entry = null;
  if (scope === "trash" || scope === "all") trashSlot.entry = null;
}

/**
 * 清空缓存与在途状态。**仅供单测**（命名带 `__` 前缀，页面不得调用）：
 * 否则用例之间会互相看到对方写入的缓存。
 */
export function __resetStorageCacheForTests(): void {
  scanSlot.entry = null;
  scanSlot.inflight.clear();
  sessionsSlot.entry = null;
  sessionsSlot.inflight.clear();
  trashSlot.entry = null;
  trashSlot.inflight.clear();
}
