import { useCallback, useState } from "react";

import * as api from "@/lib/api";
import type { CreditStatistics } from "@/lib/types";

/** 三段进度：本地数字（磁盘）→ 积分余额（12 账号 × 3 接口）→ 官方用量（12 账号并发）。 */
export interface RefreshPhase {
  local: boolean;
  credits: boolean;
  official: boolean;
}

export interface RefreshTiming {
  localMs: number | null;
  creditsMs: number | null;
  officialMs: number | null;
}

const EMPTY_PHASE: RefreshPhase = { local: false, credits: false, official: false };
const EMPTY_TIMING: RefreshTiming = { localMs: null, creditsMs: null, officialMs: null };

interface Options {
  /** 写入页面状态并同步模块级缓存（同一会话内其它入口复用同一份）。 */
  commit: (next: CreditStatistics) => void;
  refreshCredits: (accountIds: string[]) => Promise<void>;
  /** 取最新账号 id（宿主负责先 `fetchAll`，保证 12 个账号都在）。 */
  loadAccountIds: () => Promise<string[]>;
}

/**
 * 渐进式刷新：先让本地数字立刻变，再并行补积分余额与官方用量。
 *
 * 顺序依据（AUDIT §3 T4）：本地 IPC <100 ms，积分约 1 s，官方用量 4–8 s；串行会让用户
 * 盯着旧数字五秒以上。官方用量返回后就地替换，不重排整页。
 */
/** 编排主体（与 React 解耦，便于单测调用顺序）。 */
export async function runProgressiveRefresh(
  deps: Options,
  hooks: {
    onLocal: (stats: CreditStatistics, ms: number) => void;
    onCredits: (stats: CreditStatistics, ms: number) => void;
    onOfficial: (officialUsage: CreditStatistics["officialUsage"], elapsedMs: number, ms: number) => void;
    onPhase: (patch: Partial<RefreshPhase>) => void;
  },
): Promise<void> {
  const ids = await deps.loadAccountIds();

  const localStart = performance.now();
  const local = await api.getCreditStatistics(false);
  deps.commit(local);
  hooks.onLocal(local, performance.now() - localStart);
  hooks.onPhase({ local: true });

  const creditsTask = (async () => {
    if (ids.length === 0) {
      hooks.onPhase({ credits: true });
      return;
    }
    const start = performance.now();
    await deps.refreshCredits(ids);
    const next = await api.getCreditStatistics(false);
    deps.commit(next);
    hooks.onCredits(next, performance.now() - start);
    hooks.onPhase({ credits: true });
  })();

  const officialTask = (async () => {
    const start = performance.now();
    const result = await api.refreshOfficialUsage();
    const base = await api.getCreditStatistics(false);
    deps.commit({ ...base, officialUsage: result.officialUsage, elapsedMs: result.elapsedMs });
    hooks.onOfficial(result.officialUsage, result.elapsedMs, performance.now() - start);
    hooks.onPhase({ official: true });
  })();

  await Promise.all([creditsTask, officialTask]);
}

export function useProgressiveRefresh(deps: Options) {
  const { commit, refreshCredits, loadAccountIds } = deps;
  const [running, setRunning] = useState(false);
  const [phase, setPhase] = useState<RefreshPhase>(EMPTY_PHASE);
  const [timing, setTiming] = useState<RefreshTiming>(EMPTY_TIMING);

  const run = useCallback(async () => {
    setRunning(true);
    setPhase(EMPTY_PHASE);
    setTiming(EMPTY_TIMING);
    try {
      await runProgressiveRefresh(
        { commit, refreshCredits, loadAccountIds },
        {
          onLocal: (_stats, ms) =>
            setTiming((previous) => ({ ...previous, localMs: ms })),
          onCredits: (_stats, ms) =>
            setTiming((previous) => ({ ...previous, creditsMs: ms })),
          onOfficial: (_usage, _elapsed, ms) =>
            setTiming((previous) => ({ ...previous, officialMs: ms })),
          onPhase: (patch) => setPhase((previous) => ({ ...previous, ...patch })),
        },
      );
    } finally {
      setRunning(false);
    }
  }, [commit, refreshCredits, loadAccountIds]);

  const done = (phase.credits ? 1 : 0) + (phase.official ? 1 : 0);

  return { running, phase, timing, done, run };
}
