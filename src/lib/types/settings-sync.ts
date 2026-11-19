/** WorkBuddy 设置跨账号同步（T5）的契约类型。 */

export interface SettingsSyncAccount {
  accountId: string | null;
  uid: string;
  label: string;
  variant: string;
  /** 该账号在本机是否有 `storage/user-<uid>-personal` 目录（从未登录过为 false）。 */
  hasDirectory: boolean;
  directory: string;
  theme: string | null;
  themeResource: string | null;
  /** `storage/user-<uid>-personal/scoped/` 下的工作区配置目录名。 */
  scopes: string[];
}

/** 一次执行的整体结果；`partial` = 部分账号写入成功、部分失败。 */
export type SettingsSyncStatusKind =
  | "synced"
  | "partial"
  | "unchanged"
  | "skipped"
  | "failed"
  | "rejected";

/** 触发来源：切号钩子自动执行，还是用户在设置页点「同步」。 */
export type SettingsSyncTrigger = "auto" | "manual";

export interface SettingsSyncStatus {
  config: { enabled: boolean; autoOnSwitch: boolean };
  accounts: SettingsSyncAccount[];
  /** 最近一次执行记录（自动同步也算），从未执行过为 null。 */
  lastResult: SettingsSyncLastResult | null;
  denylist: string[];
}

export interface SettingsSyncChange {
  key: string;
  action: "add" | "overwrite";
  from: unknown;
  to: unknown;
}

/** 只差 `revision`（主题归属已经是目标账号）的变化，不计入「覆盖 N 项」。 */
export interface SettingsSyncCosmeticChange {
  key: string;
  action: "revision";
  from: unknown;
  to: unknown;
}

/** 预览 / 执行里单个目标账号的状态。 */
export type SettingsSyncTargetStatus = "changed" | "unchanged" | "skipped" | "synced" | "failed";

export interface SettingsSyncPlanTarget {
  uid: string;
  label: string;
  status: SettingsSyncTargetStatus;
  reason?: string;
  /** 真实变更（需要落盘）。 */
  changes: SettingsSyncChange[];
  /** 仅改写归属 / revision 的变化（不落盘、不计入项数）。 */
  cosmetic?: SettingsSyncCosmeticChange[];
  /** 写入时是否会连带改写主题归属 / revision。 */
  themeRewrite?: boolean;
  skippedKeys?: { key: string; reason: string }[];
  scopes?: {
    synced: { scope: string; changes: SettingsSyncChange[] }[];
    skipped: { scope: string; reason: string }[];
  };
}

export interface SettingsSyncPlan {
  variant: string;
  source: { uid: string; label: string; directory: string };
  targets: SettingsSyncPlanTarget[];
  /** 备份根目录（`~/.workbuddy-shift/backups/settings`）。 */
  backupRoot: string;
  dryRun: true;
}

export interface SettingsSyncTargetResult extends SettingsSyncPlanTarget {
  backup?: string | null;
  written?: string[];
  note?: string;
}

export interface SettingsSyncCounts {
  synced: number;
  unchanged: number;
  skipped: number;
  failed: number;
}

export interface SettingsSyncLastResult {
  at: number;
  trigger: SettingsSyncTrigger;
  status: SettingsSyncStatusKind;
  reason?: string | null;
  variant: string | null;
  source: SettingsSyncPlan["source"] | null;
  targets: SettingsSyncTargetResult[];
  counts?: SettingsSyncCounts;
  backupRoot: string;
}

/** 某账号的一份设置备份。 */
export interface SettingsSyncBackupEntry {
  /** `YYYYMMDD-HHMMSS`。 */
  stamp: string;
  createdAt: number | null;
  target: string | null;
  files: { file: string; bytes: number; existed: boolean }[];
  path: string;
  hasManifest: boolean;
}

export interface SettingsSyncBackupList {
  uid: string;
  backupRoot: string;
  entries: SettingsSyncBackupEntry[];
}

export interface SettingsSyncRestoreResult {
  uid: string;
  stamp: string;
  restoredAt: number;
  target: string;
  files: { file: string; action: "restored" | "removed" | "absent" }[];
  /** 还原前对当前文件做的备份目录。 */
  preRestoreBackup: string;
}
