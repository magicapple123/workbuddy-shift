/**
 * 存储总览的启动预热（第三轮第 6 条 6.4）。
 *
 * 职责边界：
 * - 纯编排（「等统计预热 → 读总览 → 归类结果」）在本文件，可单测；
 * - 空闲调度、可见性订阅、真实 api 注入也在这里（唯一有副作用的地方）；
 * - 预热范围**只有总览**（`loadStorageScan`）：会话资产太贵，启动时不扫，
 *   由存储页自己按需加载 + 页面内空闲预取。
 *
 * 为什么排在统计之后：Token / 积分预热要先跑，存储总览是重 IO 的全树元数据遍历，
 * 排在后面才不会把首屏关键路径挤掉（若用户提前点开存储页，页面自己立即发起，不排队）。
 *
 * 调度策略与 `use-stats-warmup` 一致：
 * - `requestIdleCallback(cb, { timeout: 2000 })`，没有该 API 时兜底 `setTimeout 1500`；
 * - 模块级 `scheduled` 标记保证一次应用会话只预热一次（React StrictMode 双挂载安全）；
 * - 演示模式 / 主窗口不可见时跳过；卸载时清理监听与空闲回调。
 */

import { useEffect } from "react";
import { listen } from "@tauri-apps/api/event";

import * as api from "@/lib/api";
import { isStorageScanCacheFresh, loadStorageScan } from "@/lib/storage-cache";
import { whenStatsWarmupSettled } from "@/hooks/use-stats-warmup";

/** 空闲回调的超时上限：即使浏览器一直忙也要在这个时间点跑预热。 */
const IDLE_TIMEOUT_MS = 2000;
/** 没有 `requestIdleCallback` 时的兜底延迟。 */
const IDLE_FALLBACK_MS = 1500;
/** 等统计预热结束的兜底超时：统计预热异常卡住时不能让存储预热永远不跑。 */
export const STATS_WAIT_TIMEOUT_MS = 60_000;

/** 预热结果：`hit` 复用新鲜缓存、`loaded` 本次加载成功、`failed` 静默失败、`skipped` 未执行。 */
export type StorageWarmupState = "hit" | "loaded" | "failed" | "skipped";

/** 调度层可替换的依赖（单测注入 fake，默认走真实实现）。 */
export interface StorageWarmupDeps {
  /** 演示模式判定；缺省 `api.isDemoMode`。 */
  isDemo: () => boolean;
  /** 主窗口可见判定；缺省读 `document.visibilityState`。 */
  isVisible: () => boolean;
  /** 可见性订阅；缺省 `visibilitychange` + Tauri `main-window-visible`。 */
  subscribeVisibility: (onVisible: () => void) => () => void;
  /** 空闲调度；缺省 `requestIdleCallback` / `setTimeout`。 */
  scheduleIdle: (callback: () => void) => () => void;
  /** 等统计预热结束；缺省 `whenStatsWarmupSettled`。 */
  waitForStats: () => Promise<void>;
  /** 是否已有新鲜总览缓存；缺省 `isStorageScanCacheFresh()`。 */
  hasFreshCache: () => boolean;
  /** 读总览；缺省 `loadStorageScan()`。 */
  loadScan: () => Promise<unknown>;
  /** 兜底超时（毫秒），单测注入小值。 */
  statsTimeoutMs?: number;
}

/** 模块级调度标记：一次应用会话只预热一次。 */
let scheduled = false;

/** 主窗口是否可见（非浏览器环境按可见处理，便于 node 单测）。 */
export function isMainWindowVisible(): boolean {
  if (typeof document === "undefined") return true;
  return document.visibilityState !== "hidden";
}

/** 可见性订阅：WebUI 用 `visibilitychange`，桌面额外监听 Tauri `main-window-visible`。 */
function subscribeVisibility(onVisible: () => void): () => void {
  if (typeof document === "undefined") return () => {};

  const onVisibilityChange = (): void => {
    if (isMainWindowVisible()) onVisible();
  };
  document.addEventListener("visibilitychange", onVisibilityChange);

  let disposed = false;
  let unlisten: (() => void) | null = null;
  if (typeof window !== "undefined" && !api.isWebui()) {
    try {
      void listen<boolean>("main-window-visible", (event) => {
        if (event.payload !== false) onVisible();
      })
        .then((fn) => {
          if (disposed) {
            fn();
            return;
          }
          unlisten = fn;
        })
        .catch((cause: unknown) => {
          console.info("[storage-warmup] tauri visibility listener unavailable", api.asError(cause));
        });
    } catch (cause) {
      console.info("[storage-warmup] tauri visibility listen threw", api.asError(cause));
    }
  }

  return () => {
    disposed = true;
    document.removeEventListener("visibilitychange", onVisibilityChange);
    unlisten?.();
    unlisten = null;
  };
}

/** 浏览器空闲时执行；返回取消函数。 */
function scheduleIdleCallback(callback: () => void): () => void {
  const scope = globalThis as unknown as {
    requestIdleCallback?: (cb: () => void, opts?: { timeout?: number }) => number;
    cancelIdleCallback?: (handle: number) => void;
  };
  if (typeof scope.requestIdleCallback === "function") {
    const handle = scope.requestIdleCallback(callback, { timeout: IDLE_TIMEOUT_MS });
    return () => {
      scope.cancelIdleCallback?.(handle);
    };
  }
  const timer: ReturnType<typeof setTimeout> = setTimeout(callback, IDLE_FALLBACK_MS);
  return () => {
    clearTimeout(timer);
  };
}

/** 给 Promise 加一个「到点就放弃等待」的超时（不取消底层请求，只放行后续步骤）。 */
function withTimeout(promise: Promise<void>, timeoutMs: number): Promise<void> {
  return new Promise<void>((resolve) => {
    let settled = false;
    const finish = (): void => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      resolve();
    };
    const timer: ReturnType<typeof setTimeout> = setTimeout(finish, timeoutMs);
    void promise.then(finish, finish);
  });
}

/**
 * 执行一次存储预热（永不 reject）。返回结果供 `console.info` 与 QA 断言。
 */
export async function runStorageWarmup(
  overrides: Partial<StorageWarmupDeps> = {},
): Promise<StorageWarmupState> {
  const isDemo = overrides.isDemo ?? api.isDemoMode;
  const hasFreshCache = overrides.hasFreshCache ?? (() => isStorageScanCacheFresh());
  const waitForStats = overrides.waitForStats ?? whenStatsWarmupSettled;
  const loadScan = overrides.loadScan ?? (() => loadStorageScan());

  if (isDemo()) return "skipped";

  // 已经预热过（例如用户先点开了存储页）：不发请求，避免重复扫描。
  if (hasFreshCache()) return "hit";

  try {
    await withTimeout(waitForStats(), overrides.statsTimeoutMs ?? STATS_WAIT_TIMEOUT_MS);
    await loadScan();
    return "loaded";
  } catch (cause) {
    console.info("[storage-warmup] failed", api.asError(cause));
    return "failed";
  }
}

/**
 * 调度一次存储预热；返回清理函数（卸载时调用）。
 *
 * 重复调用（StrictMode 双挂载、多组件误用）在第一次真正执行后一律无效。
 */
export function scheduleStorageWarmup(
  overrides: Partial<StorageWarmupDeps> = {},
): () => void {
  if (scheduled) return () => {};

  const isDemo = overrides.isDemo ?? api.isDemoMode;
  if (isDemo()) {
    console.info("[storage-warmup] skipped (demo)");
    scheduled = true;
    return () => {};
  }

  const isVisible = overrides.isVisible ?? isMainWindowVisible;
  const subscribe = overrides.subscribeVisibility ?? subscribeVisibility;
  const schedule = overrides.scheduleIdle ?? scheduleIdleCallback;

  scheduled = true;

  let cancelIdle: (() => void) | null = null;
  let unsubscribe: (() => void) | null = null;
  let started = false;
  let executed = false;

  const start = (): void => {
    if (started) return;
    started = true;
    unsubscribe?.();
    unsubscribe = null;
    cancelIdle = schedule(() => {
      cancelIdle = null;
      executed = true;
      void runStorageWarmup(overrides).then((state) => {
        console.info("[storage-warmup]", state);
      });
    });
  };

  if (isVisible()) start();
  else unsubscribe = subscribe(start);

  return () => {
    cancelIdle?.();
    cancelIdle = null;
    unsubscribe?.();
    unsubscribe = null;
    // 回调还没跑过就被清理 = StrictMode 的假卸载（生产环境不会发生）。
    if (!executed) scheduled = false;
  };
}

/** 在 `Layout` 挂载后调度一次存储总览预热（调度本身只生效一次）。 */
export function useStorageWarmup(): void {
  useEffect(() => scheduleStorageWarmup(), []);
}

/** 重置调度状态。**仅供单测**（命名带 `__` 前缀），页面不得调用。 */
export function __resetStorageWarmupForTests(): void {
  scheduled = false;
}
