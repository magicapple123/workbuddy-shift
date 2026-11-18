/**
 * 展示层的时间与数值格式化。
 *
 * 五个页面此前各写了一份 `formatDate` / `formatDateTime` / `formatCredits`，同一语义在
 * 不同文件里对「缺失值」的判断还不一致（有的判 `null`，有的判 `!value`），导致时间戳 `0`
 * 时而显示 `—`、时而显示 1970 年。这里统一为一套实现与一套缺失判定。
 */

/** 缺失值占位符。 */
export const EMPTY_TEXT = "—";

/** 两位补零；`YYYY-MM-DD` / `HH:mm:ss` 拼接共用。 */
export function pad2(value: number): string {
  return String(value).padStart(2, "0");
}

/**
 * 时间戳是否可用。
 *
 * `0` 与负数按缺失处理：后端用 0 表示「未设置」，直接格式化会显示 1970 年。
 */
export function isUsableTimestamp(value: number | null | undefined): value is number {
  return typeof value === "number" && Number.isFinite(value) && value > 0;
}

/** 本地时区的 `YYYY-MM-DD`，图表横轴与查询参数共用。 */
export function dateKey(date: Date): string {
  return `${date.getFullYear()}-${pad2(date.getMonth() + 1)}-${pad2(date.getDate())}`;
}

/**
 * 以「今天中午」为锚点回退 N 天的 `YYYY-MM-DD`。
 *
 * 锚在中午而不是当前时刻：临近午夜时逐次调用不会跳日，夏令时切换也不会少算一天。
 */
export function dateDaysAgo(days: number): string {
  const date = new Date();
  date.setHours(12, 0, 0, 0);
  date.setDate(date.getDate() - days);
  return dateKey(date);
}

/** `2024/01/02`：只到日，用于流水与到期日。 */
export function formatDate(value: number | null | undefined, fallback = EMPTY_TEXT): string {
  if (!isUsableTimestamp(value)) return fallback;
  return new Date(value).toLocaleDateString("zh-CN", {
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
  });
}

/** `01/02 14:30`：日 + 时分，用于列表里的「最近一次」。 */
export function formatDateTime(value: number | null | undefined, fallback = EMPTY_TEXT): string {
  if (!isUsableTimestamp(value)) return fallback;
  return new Date(value).toLocaleString("zh-CN", {
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
  });
}

/** `2024/01/02 14:30`：带年份的完整时刻，用于积分到期这类需要跨年的场景。 */
export function formatFullDateTime(
  value: number | null | undefined,
  fallback = EMPTY_TEXT,
): string {
  if (!isUsableTimestamp(value)) return fallback;
  return new Date(value).toLocaleString("zh-CN", {
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
  });
}

/** 数量展示：千分位 + 最多两位小数；非有限值返回占位符。 */
export function formatAmount(value: number | null | undefined, fallback = EMPTY_TEXT): string {
  if (typeof value !== "number" || !Number.isFinite(value)) return fallback;
  return new Intl.NumberFormat("zh-CN", { maximumFractionDigits: 2 }).format(value);
}
