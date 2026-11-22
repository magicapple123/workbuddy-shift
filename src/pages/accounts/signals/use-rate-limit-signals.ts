import { useEffect, useRef, useState } from "react";

import { useVisibleInterval } from "@/hooks/use-visible-interval";
import * as api from "@/lib/api";
import type { RateLimitBorrowed, RateLimitEntry } from "@/lib/types";
import {
  RATE_LIMIT_FETCH_FLOOR_MS,
  shouldFetchRateLimits,
} from "./rate-limit-throttle";

/**
 * 限额：CLI / WorkBuddy 由后端 hook 信号实时入账并推送，这里只兜底 IDE 日志扫描。
 *
 * 节流基准是「上次成功拉取的时刻」（前端自己记 `Date.now()`），不用 payload 的
 * `scannedAt`：后者是后端上次真正扫描日志的时刻，可能已经过去几分钟，拿它做前端节流
 * 会让刚进页面时拉不到台账（chip 要等下一轮才出现）。后端命中缓存时该接口廉价，
 * 真正的日志扫描由后端自己按 5 分钟节流。
 */

/**
 * 模型限额台账（后端合并两条通路）：一次返回全部账号，这里转成「账号 id -> 受限模型」。
 *
 * 容错：老版本后端没有该命令、或本次拉取失败时**保留上一次的台账**——一次瞬时失败
 * 不应该把已经渲染出来的 chip 全部抹掉；首次加载时 map 本就为空，保留也不会误显示。
 */
export function useRateLimitSignals() {
  const [rateLimitMap, setRateLimitMap] = useState<Record<string, RateLimitEntry[]>>({});
  /** 跨客户端兜底（13-B）：只在后端给出 borrowed 时才有值。 */
  const [rateLimitBorrowedMap, setRateLimitBorrowedMap] = useState<
    Record<string, RateLimitBorrowed>
  >({});
  /**
   * 「限额监听」开关（设置页）：关闭后不扫描、不渲染限额 chip。
   * `null` = 配置尚未读到，不得按默认 true 先扫一轮（关闭开关后进账号页会闪 chip / 误请求）。
   */
  const [rateLimitEnabled, setRateLimitEnabled] = useState<boolean | null>(null);
  /** 上次成功拉取的时刻（`Date.now()`）；`0` = 本进程还没成功拉过。 */
  const lastFetchAtRef = useRef(0);

  async function loadRateLimits(options?: { force?: boolean }) {
    if (rateLimitEnabled !== true) return;
    if (!shouldFetchRateLimits(Date.now(), lastFetchAtRef.current, options?.force)) {
      return;
    }
    try {
      const payload = await api.getRateLimits();
      const next: Record<string, RateLimitEntry[]> = {};
      const borrowed: Record<string, RateLimitBorrowed> = {};
      for (const entry of payload.accounts ?? []) {
        if (entry.limited?.length) next[entry.accountId] = entry.limited;
        if (entry.borrowed) borrowed[entry.accountId] = entry.borrowed;
      }
      setRateLimitMap(next);
      setRateLimitBorrowedMap(borrowed);
      // 只在成功时推进节流基准：失败后下一轮立刻重试，不让一次瞬时失败把刷新推迟 1 分钟。
      lastFetchAtRef.current = Date.now();
    } catch {
      // 保留上一次的台账（见上方容错说明），不弹错误、不影响账号页其它功能。
    }
  }

  // 事件监听与定时器都需要「最新」的刷新函数：直接闭包捕获会指向旧引用。
  const loadRateLimitsRef = useRef(loadRateLimits);
  loadRateLimitsRef.current = loadRateLimits;

  // 兜底轮询：页面可见时按防抖下限（20s）唤醒，由 shouldFetchRateLimits 决定是否真拉。
  // 定时器必须比刷新间隔（1 分钟）更密：如果两者相等，每次 tick 都会因为「上次拉取
  // 晚了 200ms」而差一点点不够间隔，刷新实际退化成 2 倍间隔。
  // 图标何时消失由卡片本地按 `resetAt` 每秒判定，不依赖这里的轮询。
  useVisibleInterval(
    () => void loadRateLimits(),
    RATE_LIMIT_FETCH_FLOOR_MS,
    rateLimitEnabled === true,
  );

  // 后端入账 hook 事件后推送 → 立即拉取，秒级更新；这一路不看节流。
  useEffect(() => {
    if (api.isWebui()) return;
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void api.listenDesktopEvent("rate-limits-updated", () => {
      void loadRateLimitsRef.current({ force: true });
    }).then((fn) => {
      if (disposed) {
        fn();
        return;
      }
      unlisten = fn;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  // 配置读取失败（旧版本后端没有该命令）按默认开启处理，不影响账号页其它功能。
  useEffect(() => {
    let cancelled = false;
    void api
      .getRateLimitConfig()
      .then((config) => {
        if (!cancelled) setRateLimitEnabled(config.enabled);
      })
      .catch(() => {
        if (!cancelled) setRateLimitEnabled(true);
      });
    return () => {
      cancelled = true;
    };
  }, []);

  return { rateLimitMap, rateLimitBorrowedMap, rateLimitEnabled };
}
