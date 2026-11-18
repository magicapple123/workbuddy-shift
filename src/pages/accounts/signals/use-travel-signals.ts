import { useEffect, useMemo, useState } from "react";

import { useVisibleInterval } from "@/hooks/use-visible-interval";
import * as api from "@/lib/api";
import type { AccountMeta, TravelConfig, TravelStatus } from "@/lib/types";

import { fetchTravelMap } from "./fetch-maps";

/** 旅行：后台派发/领取循环最快 15 分钟变一次状态，1 分钟用于及时反映到期领取。 */
const TRAVEL_REFRESH_INTERVAL_MS = 60 * 1000;

interface Options {
  visibleAccounts: AccountMeta[];
  /** 当前档位是否支持成长中心（仅国内版）。 */
  travelAvailable: boolean;
}

/**
 * 自动旅行配置与今日旅行状态。
 *
 * 配置未读到/读取失败按「未开启」处理：不展示 chip、不轮询；开启后按可见性轮询，
 * 反映后台派发/领取带来的状态变化。
 */
export function useTravelSignals({ visibleAccounts, travelAvailable }: Options) {
  const [travelMap, setTravelMap] = useState<Record<string, TravelStatus>>({});
  /**
   * 自动旅行配置（只读）：未开启时不渲染账号卡片的旅行 chip、也不轮询旅行状态。
   * `null` = 配置尚未读到，按未开启处理。
   */
  const [autoTravelConfig, setAutoTravelConfig] = useState<TravelConfig | null>(null);

  useEffect(() => {
    let cancelled = false;
    void api
      .getAutoTravelConfig()
      .then((config) => {
        if (!cancelled) setAutoTravelConfig(config);
      })
      .catch(() => {
        /* 配置读取失败：按未开启处理 */
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const autoTravelEnabled = travelAvailable && autoTravelConfig?.enabled === true;
  const travelAccountIds = useMemo(
    () => visibleAccounts.map((account) => account.id),
    [visibleAccounts],
  );

  async function loadTravelMap(accountIds: string[], isStale?: () => boolean) {
    const next = await fetchTravelMap(accountIds, isStale);
    if (!isStale?.() && Object.keys(next).length > 0) {
      setTravelMap((prev) => ({ ...prev, ...next }));
    }
  }

  useVisibleInterval(
    () => void loadTravelMap(travelAccountIds),
    TRAVEL_REFRESH_INTERVAL_MS,
    autoTravelEnabled && travelAccountIds.length > 0,
  );

  return { travelMap, autoTravelEnabled, loadTravelMap };
}
