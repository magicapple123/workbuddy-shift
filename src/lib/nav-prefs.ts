import {
  booleanPreference,
  readPreference,
  usePersistedPreference,
  writePreference,
} from "@/lib/persisted-preference";

/**
 * 左侧导航入口的显示偏好。
 *
 * 语义（与设置页文案一致）：
 * - 关闭「关联会话」后，左侧导航不再渲染该入口；页面本身仍可经 URL 直达，
 *   会话数据、关联关系与账号库都不受影响，重新打开即恢复。
 * - 默认值：开启。
 *
 * 持久化在 localStorage（与 `workbuddy-shift.tools.<id>` / `workbuddy-shift.theme` 同风格），
 * 键名 `workbuddy-shift.nav.sessions`，值 `"1"` / `"0"`；缺省时为开启。
 */
const SESSIONS_NAV_PREFERENCE = booleanPreference(
  "workbuddy-shift.nav.sessions",
  "workbuddy-shift:nav-prefs-changed",
  true,
);

export function isSessionsNavEnabled(): boolean {
  return readPreference(SESSIONS_NAV_PREFERENCE);
}

export function setSessionsNavEnabled(enabled: boolean): void {
  writePreference(SESSIONS_NAV_PREFERENCE, enabled);
}

/** 订阅「关联会话」入口的显示偏好；设置页改动后（同一标签页）侧边栏立即同步。 */
export function useSessionsNavEnabled(): boolean {
  return usePersistedPreference(SESSIONS_NAV_PREFERENCE);
}
