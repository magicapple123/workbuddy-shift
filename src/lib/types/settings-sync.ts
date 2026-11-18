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
  scopes: string[];
}

export interface SettingsSyncStatus {
  config: { enabled: boolean; autoOnSwitch: boolean };
  accounts: SettingsSyncAccount[];
  lastResult: SettingsSyncLastResult | null;
  denylist: string[];
}

export interface SettingsSyncChange {
  key: string;
  action: "add" | "overwrite";
  from: unknown;
  to: unknown;
}

export interface SettingsSyncPlanTarget {
  uid: string;
  label: string;
  skipped: boolean;
  reason?: string;
  changes: SettingsSyncChange[];
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
  dryRun: true;
}

export interface SettingsSyncTargetResult extends SettingsSyncPlanTarget {
  backup?: string | null;
  written?: string[];
  note?: string;
}

export interface SettingsSyncLastResult {
  at: number;
  variant: string | null;
  source: SettingsSyncPlan["source"] | null;
  targets: SettingsSyncTargetResult[];
  backupRoot: string;
}
