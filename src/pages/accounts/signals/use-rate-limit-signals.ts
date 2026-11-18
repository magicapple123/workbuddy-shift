import { useEffect, useRef, useState } from "react";

import { useVisibleInterval } from "@/hooks/use-visible-interval";
import * as api from "@/lib/api";
import type { RateLimitEntry } from "@/lib/types";

/**
 * 限额：CLI / WorkBuddy 由后端 hook 信号实时入账并推送，这里只兜底 IDE 日志扫描；
 * 后端按同一间隔节流扫描，前端再按 payload 的 `scannedAt` 判断「距上次扫描 ≥ 5 分钟」
 * 才发起，避免可见性切换/页面重挂载把扫描打散。
 */
const RATE_LIMIT_REFRESH_INTERVAL_MS = 5 * 60 * 1000;

/**
 * 模型限额台账（后端合并两条通路）：一次返回全部账号，这里转成「账号 id -> 受限模型」。
 *
 * 容错：老版本后端没有该命令、或扫描失败时按「无受限模型」处理（清空映射），
 * 不弹错误、不影响账号页其它功能。
 */
export function useRateLimitSignals() {
  const [rateLimitMap, setRateLimitMap] = useState<Record<string, RateLimitEntry[]>>({});
  /**
   * 「限额监听」开关（设置页）：关闭后不扫描、不渲染限额 chip。
   * `null` = 配置尚未读到，不得按默认 true 先扫一轮（关闭开关后进账号页会闪 chip / 误请求）。
   */
  const [rateLimitEnabled, setRateLimitEnabled] = useState<boolean | null>(null);
  const lastRateLimitScanRef = useRef(0);

  async function loadRateLimits(options?: { force?: boolean }) {
    if (rateLimitEnabled !== true) return;
    const scannedAt = lastRateLimitScanRef.current;
    if (
      !options?.force &&
      scannedAt > 0 &&
      Date.now() - scannedAt < RATE_LIMIT_REFRESH_INTERVAL_MS
    ) {
      return;
    }
    try {
      const payload = await api.getRateLimits();
      // `scannedAt` 是后端最近一次真实日志扫描的时刻：下一次扫描要等它满 5 分钟。
      lastRateLimitScanRef.current = payload.scannedAt || Date.now();
      const next: Record<string, RateLimitEntry[]> = {};
      for (const entry of payload.accounts ?? []) {
        if (entry.limited?.length) next[entry.accountId] = entry.limited;
      }
      setRateLimitMap(next);
    } catch {
      setRateLimitMap({});
    }
  }

  // 事件监听与定时器都需要「最新」的刷新函数：直接闭包捕获会指向旧引用。
  const loadRateLimitsRef = useRef(loadRateLimits);
  loadRateLimitsRef.current = loadRateLimits;

  // 兜底轮询：页面可见且距上次扫描 ≥ 5 分钟时拉一次。
  // 图标何时消失由卡片本地按 `resetAt` 每秒判定，不依赖这里的轮询。
  useVisibleInterval(
    () => void loadRateLimits(),
    RATE_LIMIT_REFRESH_INTERVAL_MS,
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

  return { rateLimitMap, rateLimitEnabled };
}
