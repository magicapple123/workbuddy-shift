import {
  booleanPreference,
  readPreference,
  usePersistedPreference,
  writePreference,
} from "@/lib/persisted-preference";

/**
 * 切换账号时默认勾选归档同步的偏好设置（Issue #165）。
 *
 * 语义：
 * - 开启后，切换账号时会预先勾选可仅同步归档状态的关联会话，保留正文。
 *   用户仍可随时取消勾选，确认切换后才执行；正文快进和覆盖沿用原有规则。
 * - 默认值：关闭（防误操作）。
 *
 * 持久化在 localStorage，键名 `workbuddy-shift.sync.default-archive`，值 `"1"` 为开，其余（`"0"`、缺省或异常）均视为关闭。
 */
const DEFAULT_ARCHIVE_SYNC_PREFERENCE = booleanPreference(
  "workbuddy-shift.sync.default-archive",
  "workbuddy-shift:sync-prefs-changed",
  false,
);

export function isDefaultArchiveSyncEnabled(): boolean {
  return readPreference(DEFAULT_ARCHIVE_SYNC_PREFERENCE);
}

export function setDefaultArchiveSyncEnabled(enabled: boolean): void {
  writePreference(DEFAULT_ARCHIVE_SYNC_PREFERENCE, enabled);
}

export function useDefaultArchiveSyncEnabled(): boolean {
  return usePersistedPreference(DEFAULT_ARCHIVE_SYNC_PREFERENCE);
}
