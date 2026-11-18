/** 存储管理页的展示格式化：字节、时间与状态文案。 */

export function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB"];
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  const digits = value >= 100 || unit === 0 ? 0 : value >= 10 ? 1 : 2;
  return `${value.toFixed(digits)} ${units[unit]}`;
}

export function formatCount(files: number): string {
  return `${files.toLocaleString("zh-CN")} 个文件`;
}

export function formatTime(ms: number | null | undefined): string {
  if (!ms) return "—";
  const date = new Date(ms);
  if (Number.isNaN(date.getTime())) return "—";
  const pad = (value: number) => String(value).padStart(2, "0");
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())} ${pad(
    date.getHours(),
  )}:${pad(date.getMinutes())}`;
}

export const PRESENCE_LABELS: Record<string, string> = {
  present: "完整",
  partial: "部分缺失",
  bodyMissing: "正文缺失",
  orphan: "孤儿残留",
};

export const ROW_STATUS_LABELS: Record<string, string> = {
  live: "存活",
  softDeleted: "软删除",
  none: "无数据库行",
};

/**
 * 资产完整度的语义底色：完整 → success，部分缺失 → warning，
 * 正文缺失 → danger（比「部分缺失」更严重：会话主体已不可用），孤儿残留 → 中性。
 */
export const PRESENCE_TONES: Record<string, string> = {
  present: "border-success-border bg-success-soft text-success",
  partial: "border-warning-border bg-warning-soft text-warning",
  bodyMissing: "border-danger-border bg-danger-soft text-danger",
  orphan: "border-slate-400/40 bg-slate-400/10 text-slate-600",
};
