/**
 * 统计启动预热的调度层（唯一有副作用的地方）。
 *
 * 职责边界：
 * - `@/lib/stats-warmup` 只做纯编排（顺序 + 门控），可单测；
 * - 本文件负责空闲调度、可见性订阅、真实 api / store 注入，以及卸载时的清理。
 *
 * 调度策略（交付规格 4.3 T2.2）：
 * - `Layout` 挂载后进入空闲：`requestIdleCallback(cb, { timeout: 2000 })`，
 *   没有该 API（Safari < 15.4）时兜底 `setTimeout(cb, 1500)`；
 * - 模块级 `scheduled` 标记保证同一模块生命周期（= 一次应用会话）只预热一次，
 *   React StrictMode 的「挂载 → 卸载 → 再挂载」不会跑两遍；
 * - 演示模式直接跳过；主窗口不可见（托盘静默启动）时先订阅可见性，恢复后再执行；
 * - 卸载时清理监听与空闲回调。
 */

import { useEffect } from "react";
import { listen } from "@tauri-apps/api/event";

import * as api from "@/lib/api";
import {
  loadCreditStatistics,
  loadTokenStatistics,
  readCreditStatisticsCache,
} from "@/lib/stats-cache";
import { runStatsWarmup, type WarmupDeps, type WarmupResult } from "@/lib/stats-warmup";
import { useAccountsStore } from "@/stores/accounts";

/** 空闲回调的超时上限：即使浏览器一直忙也要在这个时间点跑预热。 */
const IDLE_TIMEOUT_MS = 2000;
/** 没有 `requestIdleCallback` 时的兜底延迟。 */
const IDLE_FALLBACK_MS = 1500;
/** 等账号 store 在途请求结束的轮询间隔与上限（store 没有暴露 Promise，只能看 `loading`）。 */
const ACCOUNTS_WAIT_STEP_MS = 50;
const ACCOUNTS_WAIT_TIMEOUT_MS = 5000;

/** 模块级调度标记：一次应用会话只预热一次。 */
let scheduled = false;
/** 本模块自己发起的账号拉取（用于 join，不会产生第二份请求）。 */
let accountsFetch: Promise<void> | null = null;

/** 可见性订阅/退订：返回退订函数。 */
type VisibilitySubscriber = (onVisible: () => void) => () => void;
/** 空闲调度：返回取消函数。 */
type IdleScheduler = (callback: () => void) => () => void;

/** 调度层可替换的依赖（单测注入 fake，默认走真实实现）。 */
export interface WarmupSchedulerDeps {
  /** 真正执行预热的入口；缺省 `runStatsWarmup`。 */
  run: (deps: WarmupDeps) => Promise<WarmupResult>;
  /** 组装真实依赖；缺省 `createWarmupDeps`。 */
  resolveDeps: () => WarmupDeps;
  /** 演示模式判定；缺省 `api.isDemoMode`。 */
  isDemo: () => boolean;
  /** 主窗口可见判定；缺省读 `document.visibilityState`。 */
  isVisible: () => boolean;
  /** 可见性订阅；缺省 `visibilitychange` + Tauri `main-window-visible`。 */
  subscribeVisibility: VisibilitySubscriber;
  /** 空闲调度；缺省 `requestIdleCallback` / `setTimeout`。 */
  scheduleIdle: IdleScheduler;
}

/** 主窗口是否可见（非浏览器环境按可见处理，便于 node 单测）。 */
export function isMainWindowVisible(): boolean {
  if (typeof document === "undefined") return true;
  return document.visibilityState !== "hidden";
}

/**
 * 可见性订阅：WebUI 用 `visibilitychange`，桌面额外监听 Tauri `main-window-visible`
 * （Tauri 窗口的显示/隐藏不会触发 `visibilitychange`）。
 *
 * `listen` 在没有 Tauri 运行时的环境会失败，必须 try/catch：WebUI 下只能保持沉默。
 */
function subscribeVisibility(onVisible: () => void): () => void {
  if (typeof document === "undefined") return () => {};

  const onVisibilityChange = (): void => {
    if (isMainWindowVisible()) onVisible();
  };
  document.addEventListener("visibilitychange", onVisibilityChange);

  let disposed = false;
  let unlisten: (() => void) | null = null;
  // `api.isWebui()` 在非浏览器环境返回 false，因此先判 window 是否存在。
  if (typeof window !== "undefined" && !api.isWebui()) {
    try {
      void listen<boolean>("main-window-visible", (event) => {
        if (event.payload !== false) onVisible();
      })
        .then((fn) => {
          // StrictMode：「挂载 → 卸载 → 再挂载」时 Promise 可能在清理后才 resolve。
          if (disposed) {
            fn();
            return;
          }
          unlisten = fn;
        })
        .catch((cause: unknown) => {
          console.info("[stats-warmup] tauri visibility listener unavailable", api.asError(cause));
        });
    } catch (cause) {
      console.info("[stats-warmup] tauri visibility listen threw", api.asError(cause));
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

/** 等账号 store 的在途 `fetchAll` 结束（store 只暴露 `loading`，没有 Promise）。 */
function waitForAccountsSettled(): Promise<void> {
  const startedAt = Date.now();
  return new Promise<void>((resolve) => {
    const tick = (): void => {
      const busy = useAccountsStore.getState().loading;
      if (!busy || Date.now() - startedAt >= ACCOUNTS_WAIT_TIMEOUT_MS) {
        resolve();
        return;
      }
      setTimeout(tick, ACCOUNTS_WAIT_STEP_MS);
    };
    tick();
  });
}

/**
 * 账号 store 就绪：已有在途就等它结束，否则调用一次 `fetchAll` 并记住这个 Promise。
 *
 * 预热与账号页共用同一个 zustand store，重复 `fetchAll` 会多打两次 IPC，因此这里
 * 自己做一次 join（不引入新的账号请求，只是复用 store 的既有能力）。
 */
function fetchAccounts(): Promise<void> {
  if (accountsFetch !== null) return accountsFetch;
  const store = useAccountsStore.getState();
  if (store.loading) return waitForAccountsSettled();
  const request = store.fetchAll().finally(() => {
    if (accountsFetch === request) accountsFetch = null;
  });
  accountsFetch = request;
  return request;
}

/** T2.5：只预取两个统计页的 chunk（不触发任何接口请求），失败静默。 */
function prefetchStatsRoutes(): void {
  void import("@/pages/token-stats").catch((cause: unknown) => {
    console.info("[stats-warmup] prefetch token-stats failed", api.asError(cause));
  });
  void import("@/pages/credit-stats").catch((cause: unknown) => {
    console.info("[stats-warmup] prefetch credit-stats failed", api.asError(cause));
  });
}

/** 组装真实依赖：所有 `api.*` / store 访问集中在这里，纯编排层保持零副作用。 */
function createWarmupDeps(): WarmupDeps {
  return {
    isDemo: api.isDemoMode,
    isVisible: isMainWindowVisible,
    fetchAccounts,
    hasAccountsInflight: () => useAccountsStore.getState().loading,
    getAccounts: () => useAccountsStore.getState().accounts,
    getOfficialCollectedAt: () => readCreditStatisticsCache()?.officialUsage?.collectedAt ?? null,
    loadCredit: (refresh?: boolean) => loadCreditStatistics(refresh === true),
    loadToken: () => loadTokenStatistics(),
    refreshOfficialUsage: () => api.refreshOfficialUsage(),
    now: () => Date.now(),
    prefetchRoutes: prefetchStatsRoutes,
  };
}

/**
 * 调度一次统计预热；返回清理函数（卸载时调用）。
 *
 * 重复调用（StrictMode 双挂载、多组件误用）在第一次真正执行后一律无效。
 */
export function scheduleStatsWarmup(overrides: Partial<WarmupSchedulerDeps> = {}): () => void {
  if (scheduled) return () => {};

  const isDemo = overrides.isDemo ?? api.isDemoMode;
  if (isDemo()) {
    // 演示模式没有真实后端：标记为已处理，避免反复尝试。
    console.info("[stats-warmup] skipped (demo)");
    scheduled = true;
    return () => {};
  }

  const isVisible = overrides.isVisible ?? isMainWindowVisible;
  const subscribe = overrides.subscribeVisibility ?? subscribeVisibility;
  const schedule = overrides.scheduleIdle ?? scheduleIdleCallback;
  const resolveDeps = overrides.resolveDeps ?? createWarmupDeps;
  const run = overrides.run ?? runStatsWarmup;

  scheduled = true;

  let cancelIdle: (() => void) | null = null;
  let unsubscribe: (() => void) | null = null;
  /** 已经启动（可见性事件只应触发一次）。 */
  let started = false;
  /** 空闲回调真的跑过（StrictMode 假卸载的判定依据）。 */
  let executed = false;

  const start = (): void => {
    if (started) return;
    started = true;
    unsubscribe?.();
    unsubscribe = null;
    cancelIdle = schedule(() => {
      cancelIdle = null;
      executed = true;
      void run(resolveDeps()).catch((cause: unknown) => {
        console.info("[stats-warmup] dispatch failed", api.asError(cause));
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
    // 回调还没跑过就被清理 = StrictMode 的假卸载（生产环境不会发生）：把模块级标记
    // 还回去，否则开发模式下预热永远不会执行。真跑过就不再重置，一次会话只预热一次。
    if (!executed) scheduled = false;
  };
}

/** 在 `Layout` 挂载后调度一次统计预热（调度本身只生效一次）。 */
export function useStatsWarmup(): void {
  useEffect(() => scheduleStatsWarmup(), []);
}

/**
 * 重置调度与账号 join 状态。**仅供单测**（命名带 `__` 前缀）：用例之间互相隔离，
 * 页面与组件不得调用。
 */
export function __resetWarmupScheduleForTests(): void {
  scheduled = false;
  accountsFetch = null;
}
