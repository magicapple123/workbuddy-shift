import { useEffect, useMemo, useState } from "react";

import * as api from "@/lib/api";
import type { AccountMeta, CheckinConfig } from "@/lib/types";

import { fetchTodayCheckinMap } from "./fetch-maps";

interface Options {
  visibleAccounts: AccountMeta[];
  /** 当前档位是否提供签到接口（国际版不查状态）。 */
  checkinAvailable: boolean;
}

/**
 * 签到相关的只读配置与今日状态。
 *
 * 自动签到配置只用于决定卡片是否展示「自动签到已关闭」chip、以及查询/刷新时跳过哪些
 * 账号；控制入口在设置页。配置读取失败按空名单处理，不打扰用户。
 */
export function useCheckinSignals({ visibleAccounts, checkinAvailable }: Options) {
  /** 账号 id -> 今日是否已签到（undefined=查询中/未知） */
  const [checkinMap, setCheckinMap] = useState<Record<string, boolean>>({});
  const [autoCheckinConfig, setAutoCheckinConfig] = useState<CheckinConfig | null>(null);
  /** 配置是否已读取完毕（成功或失败）：区分「尚未读到」与「读取失败」。 */
  const [autoCheckinSettled, setAutoCheckinSettled] = useState(false);

  useEffect(() => {
    let cancelled = false;
    void api
      .getAutoCheckinConfig()
      .then((config) => {
        if (!cancelled) setAutoCheckinConfig(config);
      })
      .catch(() => {
        /* 配置读取失败：按空名单处理 */
      })
      .finally(() => {
        if (!cancelled) setAutoCheckinSettled(true);
      });
    return () => {
      cancelled = true;
    };
  }, []);

  /** 关闭自动签到的账号 id（配置未读到/读取失败 = 空名单）。 */
  const excludedCheckinIds = useMemo(
    () => new Set(autoCheckinConfig?.excluded_account_ids ?? []),
    [autoCheckinConfig],
  );

  /** 今日签到状态只查未关闭自动签到的账号；配置就绪前不发请求。 */
  const autoCheckinAccountIds = useMemo(() => {
    if (!checkinAvailable || !autoCheckinSettled) return [];
    return visibleAccounts
      .filter((account) => !excludedCheckinIds.has(account.id))
      .map((account) => account.id);
  }, [visibleAccounts, checkinAvailable, autoCheckinSettled, excludedCheckinIds]);

  useEffect(() => {
    if (!autoCheckinAccountIds.length) return;
    let cancelled = false;
    void fetchTodayCheckinMap(autoCheckinAccountIds, () => cancelled).then((next) => {
      if (!cancelled && Object.keys(next).length > 0) {
        setCheckinMap((prev) => ({ ...prev, ...next }));
      }
    });
    return () => {
      cancelled = true;
    };
  }, [autoCheckinAccountIds]);

  return { checkinMap, setCheckinMap, excludedCheckinIds, autoCheckinSettled };
}
