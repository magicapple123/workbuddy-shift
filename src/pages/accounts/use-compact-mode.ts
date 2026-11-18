import { booleanPreference, usePersistedPreferenceState } from "@/lib/persisted-preference";

const COMPACT_PREFERENCE = booleanPreference(
  "workbuddy-shift.compact",
  "workbuddy-shift:compact-changed",
  true,
);

/**
 * 紧凑模式：卡片更小、同屏更多列；默认开启，持久化到 localStorage。
 *
 * 存储不可用（隐私模式）时静默降级为内存状态，不阻塞页面。
 */
export function useCompactMode(): [boolean, () => void] {
  const [compact, setCompact] = usePersistedPreferenceState(COMPACT_PREFERENCE);
  return [compact, () => setCompact(!compact)];
}
