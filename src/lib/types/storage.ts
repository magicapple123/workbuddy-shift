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

/** 扫描各阶段耗时（毫秒），用于诊断与「改前 / 改后」对照。 */
export interface StorageTimings {
  totalMs: number;
  catalogMs: number;
  sessionsMs: number;
  trashMs: number;
  /** 客户端进程探测耗时（Windows 上数百毫秒，与扫描无关）。 */
  probeMs?: number;
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
  /** 受保护大目录还在后台计量：`totals` / 客户端卡是**部分和**，必须标注。 */
  protectedPending?: boolean;
  /** 后台补算是否正在运行（诊断用）。 */
  protectedFilling?: boolean;
  timings?: StorageTimings;
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
  /** 复用服务端快照（同一 TTL 内会话树只走一遍）。 */
  cached?: boolean;
  clientRunning: boolean;
  /**
   * 「一键清理冗余副本」实际会删掉的条数（Σ(每组每账号存活数-1)，每组留最新一条）。
   *
   * 由服务端算，与 `redundantAll` 的取舍同源：按钮上的数字必须等于点下去真正删掉的条数，
   * 前端不自己数（自己数会和「软删成员算不算」的口径打架）。
   */
  redundantCleanable: number;
  sessions: StorageSession[];
}

/** 清理目标：分类 / 会话子项 / 冗余副本（单组或一键全部）。 */
export type StorageCleanTarget =
  | { kind: "category"; id: string }
  | { kind: "session"; id: string; entries?: StorageEntryKind[]; deleteRow?: boolean }
  | { kind: "redundant"; groupId: string; keepSessionId?: string }
  /** 一键清理：所有「同组同账号多份」的副本，每组每账号保留最新一条（跨账号副本不删）。 */
  | { kind: "redundantAll" };

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
  /** 面向用户的一句话说明（一键清理冗余副本讲清「删几条、保留哪条」）。 */
  note?: string | null;
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
  /** 与计划里同一条说明：删了几条重复副本、按什么规则保留。 */
  note?: string | null;
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
  cached?: boolean;
  timings?: StorageTimings;
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
