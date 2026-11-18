/**
 * 存储管理（T3）的返回结构。
 *
 * 与 `wb_shift_core::modules::storage` 的 JSON 逐字段对齐；分类 / 会话 / 回收站
 * 都带真实字节数与文件数，页面只做展示与排序，不自行估算。
 */

export type StorageCategoryGroup = "tool" | "client";

export interface StorageRoot {
  id: string;
  label: string;
  path: string;
  exists: boolean;
  bytes?: number;
  files?: number;
}

export interface StorageCategory {
  id: string;
  group: StorageCategoryGroup;
  label: string;
  path: string;
  bytes: number;
  files: number;
  /** 可删除单元数（目录 / 文件个数）。 */
  units: number;
  deletable: boolean;
  note: string;
  protected: string | null;
}

export interface StorageScan {
  generatedAt: number;
  variant: string;
  cached: boolean;
  roots: StorageRoot[];
  totals: { bytes: number; files: number };
  categories: StorageCategory[];
  sessions: {
    counts: {
      live: number;
      softDeleted: number;
      orphan: number;
      redundant: number;
      present: number;
      partial: number;
      bodyMissing: number;
    };
    bytes: number;
    files: number;
  };
  trash: { entries: number; bytes: number; files: number };
  clientRunning: boolean;
  warnings: string[];
}

export type StorageEntryKind =
  | "body"
  | "sidecar"
  | "artifact-index"
  | "file-history"
  | "changes-index"
  | "changes-detail";

export interface StorageSessionEntry {
  kind: StorageEntryKind;
  label: string;
  path: string;
  bytes: number;
  files: number;
  exists: boolean;
}

export interface StorageSession {
  id: string;
  uid: string;
  title: string;
  cwd: string;
  updatedAt: number;
  variant: string;
  /** `live` / `softDeleted` / `none`。 */
  rowStatus: "live" | "softDeleted" | "none";
  /** `present` / `partial` / `bodyMissing` / `orphan`。 */
  presence: "present" | "partial" | "bodyMissing" | "orphan";
  redundant: boolean;
  groupId: string | null;
  liveCopies: number;
  bytes: number;
  files: number;
  entries: StorageSessionEntry[];
}

export interface StorageSessionsPayload {
  variant: string;
  clientRunning: boolean;
  sessions: StorageSession[];
}

/** 清理目标：分类 / 会话子项 / 冗余副本。 */
export type StorageCleanTarget =
  | { kind: "category"; id: string }
  | { kind: "session"; id: string; entries?: StorageEntryKind[]; deleteRow?: boolean }
  | { kind: "redundant"; groupId: string; keepSessionId?: string };

export interface StorageCleanPlanItem {
  path: string;
  category: string;
  sessionId: string | null;
  bytes: number;
  files: number;
  exists: boolean;
}

export interface StorageCleanPlan {
  dryRun: true;
  items: StorageCleanPlanItem[];
  rows: { sessionId: string; title: string; variant: string; rowStatus: string }[];
  labels: string[];
  bytes: number;
  files: number;
  warnings: string[];
}

export interface StorageCleanResult {
  dryRun: false;
  trashId: string;
  moved: number;
  deletedRows: number;
  bytes: number;
  files: number;
  failures: { path?: string; sessionId?: string; error: string }[];
  warnings: string[];
  labels: string[];
  manifest: string;
}

export interface StorageTrashItem {
  rootId: string;
  originalPath: string;
  bytes: number;
  files: number;
  mtime: number;
  category: string;
  sessionId: string | null;
  kind: string;
  result: string;
  state: "restorable" | "conflict" | "missing";
}

export interface StorageTrashEntry {
  id: string;
  createdAt: number;
  bytes: number;
  files: number;
  rows: number;
  dbBackups: string[];
  items: StorageTrashItem[];
}

export interface StorageTrashPayload {
  entries: StorageTrashEntry[];
  problems: string[];
  bytes: number;
  files: number;
}

export interface StorageRestoreResult {
  id: string;
  restoredFiles: number;
  restoredRows: number;
  failures: { path?: string; row?: unknown; error: string }[];
  purged: boolean;
}

export interface StorageRevalidateItem {
  target: string;
  exists: boolean;
  presence?: string;
  rowStatus?: string;
  bytes: number;
  files: number;
  deletable?: boolean;
  entries?: { kind: string; exists: boolean; bytes: number }[];
}
