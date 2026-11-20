/**
 * 「设置跨账号同步」卡片两块明细的折叠摘要（R5-06）。
 *
 * 抽成纯函数是为了能在 node 环境的 vitest 里单测计数文案——
 * 前端目前没有组件测试基建（无 jsdom / testing-library），
 * 而「折叠时标题行摘要必须正确」是这条整改的验收点之一。
 */

/** 折叠块标识：本机账号表 / 本次预览。 */
export type SettingsSyncFoldBlock = "accounts" | "preview";

/**
 * 两块明细的默认展开状态：**默认收起**。
 *
 * 刻意不持久化到 localStorage：离开设置页再回来仍是折叠态，
 * 与「默认折叠」保持同一语义（清单明确要求不持久化）。
 */
export const SETTINGS_SYNC_FOLD_DEFAULT_OPEN: Record<SettingsSyncFoldBlock, boolean> = {
  accounts: false,
  preview: false,
};

/** 摘要只需要「有没有本机偏好目录」一个字段，避免依赖完整的 `SettingsSyncAccount`。 */
export interface FoldAccountRow {
  hasDirectory: boolean;
}

/**
 * 本机账号表的折叠摘要：`10 个账号 · 9 个可同步 · 1 个未登录`。
 *
 * 三个计数都从同一批行现算，未登录数 = 总数 - 可同步数，因此恒等于总数。
 */
export function accountsFoldSummary(rows: readonly FoldAccountRow[]): string {
  const total = rows.length;
  const syncable = rows.filter((row) => row.hasDirectory).length;
  const loggedOut = total - syncable;
  return `${total} 个账号 · ${syncable} 个可同步 · ${loggedOut} 个未登录`;
}

/**
 * 「本次预览」的折叠摘要：有待同步账号时 `待同步 3 个账号 · 7 项`，否则 `已一致`。
 *
 * 与顶部状态文案（`statusText`）用同一套口径：只看 `status === "changed"` 的目标，
 * 项数不含「仅主题归属差异」。
 */
export function previewFoldSummary(changedAccountCount: number, pendingItemCount: number): string {
  if (changedAccountCount <= 0) return "已一致";
  return `待同步 ${changedAccountCount} 个账号 · ${pendingItemCount} 项`;
}
