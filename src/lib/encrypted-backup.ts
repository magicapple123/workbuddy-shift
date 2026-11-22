/**
 * 加密备份（issue #186）的**纯逻辑**：口令策略 + 提交流程编排。
 *
 * 为什么单独一层而不是写在弹窗的 `onClick` 里：
 * - 口令策略必须与后端（`wb-shift-core` 的 `export_import::validate_password`）同口径，
 *   两边各写一份必然漂移；这里是最短长度与错误文案的唯一前端来源；
 * - "校验没过就绝不发请求"是一条**行为约束**，不是渲染细节。把它放在可单测的纯函数里，
 *   才能在 node 环境（仓库没有 testing-library）里用注入的假 `run` 证明命令没被调用。
 *
 * 本模块不 import `@/lib/api`（那会拖进 Tauri 插件与演示模式），只接受类型；
 * 真正的命令由调用方通过 `deps.run` 注入。
 */

import type { EncryptedExportResult, ImportResult } from "./types";

/**
 * 最短口令长度，必须与 `crates/wb-shift-core/src/modules/export_import/encrypted.rs`
 * 的 `MIN_PASSWORD_LEN` 一致（后端是最终裁决，这里只是让用户在提交前就看到错误）。
 */
export const MIN_PASSWORD_LENGTH = 8;

/** 口令长度校验；返回错误文案，`null` 表示通过。 */
export function passwordLengthError(password: string): string | null {
  // 按字符数而不是字节数：中文口令 8 个汉字就是 8 位，与后端 `chars().count()` 同口径。
  const length = [...password].length;
  if (length < MIN_PASSWORD_LENGTH) {
    return `密码至少 ${MIN_PASSWORD_LENGTH} 位（当前 ${length} 位）`;
  }
  return null;
}

/** 导出前校验：口令长度 + 两次输入必须一致。`null` 表示可以提交。 */
export function exportPasswordError(password: string, confirm: string): string | null {
  const lengthError = passwordLengthError(password);
  if (lengthError) return lengthError;
  if (password !== confirm) return "两次输入的密码不一致";
  return null;
}

/** 导入前校验：只有长度（导入是"输入已有口令"，没有二次确认）。 */
export function importPasswordError(password: string): string | null {
  return passwordLengthError(password);
}

/**
 * 加密导出的提交结果。
 *
 * - `invalid`：口令没过校验，**命令一次都没下发**；
 * - `canceled`：用户在保存对话框里取消；
 * - `done`：后端已写入并返回路径与账号数；
 * - `failed`：后端拒绝（路径非法、磁盘错误…），`error` 交给调用方格式化。
 */
export type EncryptedExportOutcome =
  | { status: "invalid"; error: string }
  | { status: "canceled" }
  | { status: "done"; path: string; count: number }
  | { status: "failed"; error: unknown };

/** 加密导入的提交结果（`done` 带后端返回的 `ImportResult` 摘要）。 */
export type EncryptedImportOutcome =
  | { status: "invalid"; error: string }
  | { status: "canceled" }
  | { status: "done"; report: ImportResult }
  | { status: "failed"; error: unknown };

/** 加密导出流程的注入依赖（测试替换这两项即可，不触碰真实对话框与命令）。 */
export interface EncryptedExportDeps {
  /** 弹出保存对话框；用户取消返回 `null`。 */
  pickPath: () => Promise<string | null>;
  /** 下发命令。 */
  run: (path: string, password: string) => Promise<EncryptedExportResult>;
}

/** 加密导入流程的注入依赖。 */
export interface EncryptedImportDeps {
  /** 弹出打开对话框；用户取消返回 `null`。 */
  pickPath: () => Promise<string | null>;
  /** 下发命令。 */
  run: (path: string, password: string) => Promise<ImportResult>;
}

/**
 * 加密导出：校验口令 → 选路径 → 下发命令。
 *
 * 顺序是刻意的：口令不合规时**连对话框都不弹**（用户还没选路径就先看到错误），
 * 也就不会在选了半天的路径之后被后端打回来。校验分支里 `deps.run` 不会被调用。
 */
export async function submitEncryptedExport(
  password: string,
  confirm: string,
  deps: EncryptedExportDeps,
): Promise<EncryptedExportOutcome> {
  const invalid = exportPasswordError(password, confirm);
  if (invalid) return { status: "invalid", error: invalid };

  const path = await deps.pickPath();
  if (!path) return { status: "canceled" };

  try {
    const result = await deps.run(path, password);
    // 后端回的 path 才是真正写入的位置；缺字段时退回用户选的路径，只是展示用途。
    return { status: "done", path: result.path || path, count: result.count };
  } catch (error) {
    return { status: "failed", error };
  }
}

/**
 * 加密导入：校验口令 → 选文件 → 下发命令。
 *
 * 与导出相反，这里先校验再选文件：口令不足 8 位时不解密任何东西，
 * 避免"选完文件才发现口令不合规"的多余操作。
 */
export async function submitEncryptedImport(
  password: string,
  deps: EncryptedImportDeps,
): Promise<EncryptedImportOutcome> {
  const invalid = importPasswordError(password);
  if (invalid) return { status: "invalid", error: invalid };

  const path = await deps.pickPath();
  if (!path) return { status: "canceled" };

  try {
    return { status: "done", report: await deps.run(path, password) };
  } catch (error) {
    return { status: "failed", error };
  }
}

/** 导入结果摘要文案（新增 / 覆盖 / 跳过），桌面与 WebUI 共用一句，便于测试固定措辞。 */
export function importSummary(report: ImportResult): string {
  return `新增 ${report.imported - report.overwritten} 个，覆盖 ${report.overwritten} 个，跳过 ${report.skipped} 个`;
}
