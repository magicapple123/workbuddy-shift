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

/** 总览与分类明细（服务端 60 秒缓存；`cached=true` 表示返回缓存）。 */
export function storageScan(variant?: WbVariant): Promise<StorageScan> {
  return call("storage_scan", variant === "ai" ? { variant } : undefined);
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

export function storageTrash(): Promise<StorageTrashPayload> {
  return call("storage_trash");
}

export function storageTrashRestore(id: string): Promise<StorageRestoreResult> {
  return call("storage_trash_restore", { id });
}

/** `id` 为空表示清空回收站（彻底删除，不可恢复）。 */
export function storageTrashPurge(id?: string): Promise<{ deleted: number; bytes: number }> {
  return call("storage_trash_purge", id ? { id } : {});
}

/** 在系统文件管理器中定位；`select` 选中文件，`open` 打开目录。 */
export function storageReveal(path: string, mode: "select" | "open"): Promise<{
  ok: boolean;
  path: string;
  fellBack: boolean;
}> {
  return call("storage_reveal", { path, mode });
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
