import type {
  SettingsSyncBackupList,
  SettingsSyncLastResult,
  SettingsSyncPlan,
  SettingsSyncRestoreResult,
  SettingsSyncStatus,
} from "../types";
import { call } from "./core";

/** 配置 + 账号主题快照 + 最近一次同步结果。 */
export function getSettingsSyncStatus(): Promise<SettingsSyncStatus> {
  return call("get_settings_sync_status");
}

/** 保存开关；未传的字段保持原值。 */
export function saveSettingsSyncConfig(patch: {
  enabled?: boolean;
  autoOnSwitch?: boolean;
}): Promise<{ enabled: boolean; autoOnSwitch: boolean }> {
  return call("save_settings_sync_config", patch);
}

/** 只读预览：源账号 → 同档位其余账号会改哪些键。 */
export function previewSettingsSync(sourceAccountId?: string): Promise<SettingsSyncPlan> {
  return call(
    "preview_settings_sync",
    sourceAccountId ? { sourceAccountId } : {},
  );
}

/** 执行同步（`dryRun` 缺省 true；写前备份，备份失败零写入）。 */
export function syncSettings(
  sourceAccountId: string | undefined,
  dryRun: boolean,
): Promise<SettingsSyncLastResult> {
  return call("sync_settings", {
    ...(sourceAccountId ? { sourceAccountId } : {}),
    dryRun,
  });
}

/** 列举某账号的设置备份（时间戳倒序）。 */
export function listSettingsSyncBackups(uid: string): Promise<SettingsSyncBackupList> {
  return call("list_settings_sync_backups", { uid });
}

/** 还原某次备份；服务端会先把当前文件再备份一份。 */
export function restoreSettingsSyncBackup(
  uid: string,
  stamp: string,
): Promise<SettingsSyncRestoreResult> {
  return call("restore_settings_sync_backup", { uid, stamp });
}
