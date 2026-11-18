import * as api from "@/lib/api";
import type { TravelStatus } from "@/lib/types";

/**
 * 并行查询今日签到；失败/过期的账号不写入，由调用方保留原值。
 *
 * 每个账号独立 catch：单个账号接口失败不应让其余账号的签到状态一起丢失。
 */
export async function fetchTodayCheckinMap(
  accountIds: string[],
  isStale?: () => boolean,
): Promise<Record<string, boolean>> {
  const entries = await Promise.all(
    accountIds.map(async (id) => {
      try {
        const res = await api.getCheckinStatus(id);
        if (isStale?.() || !res.ok || typeof res.todayCheckedIn !== "boolean") return null;
        return [id, res.todayCheckedIn] as const;
      } catch {
        return null;
      }
    }),
  );
  const next: Record<string, boolean> = {};
  for (const entry of entries) {
    if (entry) next[entry[0]] = entry[1];
  }
  return next;
}

interface TravelStatusRow {
  accountId: string;
  label: TravelStatus["label"];
  rewardCredit: number | null;
  locationName?: string | null;
  arriveAt?: number | null;
}

/**
 * 查询各账号今日旅行状态；失败/过期的账号不写入，由调用方保留原值。
 *
 * webui（非演示模式）的 `GET /api/travel/status` 本身就是批量接口（一次 reconcile 全账号），
 * 逐账号调用会让 N 个账号重复 N 次全量扫描；这里按 webui 一次取回再过滤。
 * 桌面命令是单账号接口，仍逐账号调用（本地读状态文件，开销很小）。
 */
export async function fetchTravelMap(
  accountIds: string[],
  isStale?: () => boolean,
): Promise<Record<string, TravelStatus>> {
  if (api.isWebui() && !api.isDemoMode() && accountIds.length > 1) {
    try {
      const payload = await api.httpCall<{ accounts?: TravelStatusRow[] }>("get_travel_status");
      if (isStale?.()) return {};
      const wanted = new Set(accountIds);
      const next: Record<string, TravelStatus> = {};
      for (const row of payload.accounts ?? []) {
        if (!wanted.has(row.accountId)) continue;
        next[row.accountId] = {
          label: row.label,
          rewardCredit: row.rewardCredit,
          locationName: row.locationName ?? null,
          arriveAt: row.arriveAt ?? null,
        };
      }
      return next;
    } catch {
      return {};
    }
  }
  const entries = await Promise.all(
    accountIds.map(async (id) => {
      try {
        const res = await api.getTravelStatus(id);
        if (isStale?.()) return null;
        return [id, res] as const;
      } catch {
        return null;
      }
    }),
  );
  const next: Record<string, TravelStatus> = {};
  for (const entry of entries) {
    if (entry) next[entry[0]] = entry[1];
  }
  return next;
}
