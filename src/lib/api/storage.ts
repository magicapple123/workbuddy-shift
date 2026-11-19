import type {
  StorageCleanPlan,
  StorageCleanResult,
  StorageCleanTarget,
  StorageRestoreResult,
  StorageRevalidateItem,
  StorageScan,
  StorageSessionsPayload,
  StorageTrashPayload,
  WbVariant,
} from "../types";
import { call } from "./core";

/**
 * 总览与分类明细（服务端 60 秒快照；`cached=true` 表示返回缓存）。
 *
 * @param refresh 「重新扫描」：服务端作废快照与受保护项体积缓存后重算。
 */
export function storageScan(variant?: WbVariant, refresh = false): Promise<StorageScan> {
  const args: Record<string, unknown> = {};
  if (variant === "ai") args.variant = variant;
  if (refresh) args.refresh = true;
  return call("storage_scan", Object.keys(args).length > 0 ? args : undefined);
}

/** 全部会话资产包（含逐子项）。 */
export function storageSessions(variant?: WbVariant): Promise<StorageSessionsPayload> {
  return call("storage_sessions", variant === "ai" ? { variant } : {});
}

/**
 * 计算清理计划（默认 dry-run）。执行时显式传 `dryRun: false`。
 *
 * 服务端在任何一项校验失败时返回 400 且零写入，前端只负责展示原因。
 */
export function storageClean(
  targets: StorageCleanTarget[],
  dryRun: boolean,
  variant?: WbVariant,
): Promise<StorageCleanPlan | StorageCleanResult> {
  return call("storage_clean", {
    targets,
    dryRun,
    ...(variant === "ai" ? { variant } : {}),
  });
}

/** 回收站条目（体积来自同一份快照，条目清单每次现读）。 */
export function storageTrash(variant?: WbVariant): Promise<StorageTrashPayload> {
  return call("storage_trash", variant === "ai" ? { variant } : undefined);
}

export function storageTrashRestore(id: string): Promise<StorageRestoreResult> {
  return call("storage_trash_restore", { id });
}

/** `id` 为空表示清空回收站（彻底删除，不可恢复）。 */
export function storageTrashPurge(id?: string): Promise<{ deleted: number; bytes: number }> {
  return call("storage_trash_purge", id ? { id } : {});
}

/** 定位范围：`storage`（缺省，白名单内）/ `cwd`（只读打开任意绝对目录）。 */
export type StorageRevealScope = "storage" | "cwd";

/**
 * 在系统文件管理器中定位；`select` 选中文件，`open` 打开目录。
 *
 * `scope: "cwd"` 只用于会话的「工作目录」按钮（后端拒绝 UNC / 设备 / 相对路径，
 * 且不会执行目标文件）；其它按钮不传，走服务端缺省的 `storage` 白名单。
 */
export function storageReveal(
  path: string,
  mode: "select" | "open",
  scope: StorageRevealScope = "storage",
): Promise<{
  ok: boolean;
  path: string;
  fellBack: boolean;
  scope: StorageRevealScope;
}> {
  return call("storage_reveal", scope === "cwd" ? { path, mode, scope } : { path, mode });
}

/** 外部删除后重校验；只接受 `session:<id>` / `category:<id>` / `trash:<id>`。 */
export function storageRevalidate(
  targets: string[],
  variant?: WbVariant,
): Promise<{ items: StorageRevalidateItem[] }> {
  return call("storage_revalidate", {
    targets,
    ...(variant === "ai" ? { variant } : {}),
  });
}
