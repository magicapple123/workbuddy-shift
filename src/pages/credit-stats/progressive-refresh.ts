import { useCallback, useState } from "react";

import * as api from "@/lib/api";
import type { CreditOfficialUsage, CreditStatistics } from "@/lib/types";

/**
 * 官方用量「整页不可用」占位对象。
 *
 * 渐进式刷新的官方任务一旦抛错（网络层 reject、IPC 失败、命令超时），旧实现没有 `catch`，
 * `officialUsage` 会停在 `undefined`：既没有警告条，四张卡也悄悄退回本地口径——比
 * 「官方面返回 unavailable」更静默（issue #225 根因 C）。这里把异常归一成与后端
 * `status: "unavailable"` 完全同构的响应，保证页面一定有横幅与失败原因。
 */
export function unavailableOfficialUsage(reason: string): CreditOfficialUsage {
  return {
    status: "unavailable",
    rangeStart: "",
    rangeEnd: "",
    summary: { usageToday: 0, usage7Days: 0, usageThisMonth: 0 },
    daily: [],
    accounts: [],
    requests: [],
    models: [],
    detailLimitPerAccount: 0,
    errors: [{ accountId: "", accountName: "官方用量", error: reason }],
  };
}

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
  /** 当前范围（30d/90d/all）：刷新后回读同一范围，避免把 30 天结果盖到「全部」上。 */
  range?: CreditStatistics["range"];
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
  const local = await api.getCreditStatistics(false, deps.range);
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
    const next = await api.getCreditStatistics(false, deps.range);
    deps.commit(next);
    hooks.onCredits(next, performance.now() - start);
    hooks.onPhase({ credits: true });
  })();

  const officialTask = (async () => {
    const start = performance.now();
    try {
      const result = await api.refreshOfficialUsage();
      const base = await api.getCreditStatistics(false, deps.range);
      deps.commit({ ...base, officialUsage: result.officialUsage, elapsedMs: result.elapsedMs });
      hooks.onOfficial(result.officialUsage, result.elapsedMs, performance.now() - start);
    } catch (cause) {
      // 失败原因必须留证并回写到页面状态：不抛、不静默，也不让官方的坏结果覆盖本地数字。
      const reason = api.asError(cause);
      api.logError("frontend_unhandled", "官方用量刷新失败", reason).catch(() => {});
      const failure = unavailableOfficialUsage(reason);
      try {
        const base = await api.getCreditStatistics(false, deps.range);
        deps.commit({ ...base, officialUsage: failure });
      } catch {
        // 连本地统计都取不到时保持页面现状：横幅仍由 `officialUsage` 缺失时的默认分支兜住。
      }
      hooks.onOfficial(failure, 0, performance.now() - start);
    } finally {
      hooks.onPhase({ official: true });
    }
  })();


  await Promise.all([creditsTask, officialTask]);
}

export function useProgressiveRefresh(deps: Options) {
  const { commit, refreshCredits, loadAccountIds, range } = deps;
  const [running, setRunning] = useState(false);
  const [phase, setPhase] = useState<RefreshPhase>(EMPTY_PHASE);
  const [timing, setTiming] = useState<RefreshTiming>(EMPTY_TIMING);

  const run = useCallback(async () => {
    setRunning(true);
    setPhase(EMPTY_PHASE);
    setTiming(EMPTY_TIMING);
    try {
      await runProgressiveRefresh(
        { commit, refreshCredits, loadAccountIds, range },
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
  }, [commit, refreshCredits, loadAccountIds, range]);

  const done = (phase.credits ? 1 : 0) + (phase.official ? 1 : 0);

  return { running, phase, timing, done, run };
}
