import type { AccountMark, CreditExpiry, CreditResource, TravelStatus } from "@/lib/types";

export function formatCreditExpiry(ts: number | null): string {
  if (!ts) return "长期有效";
  const date = new Date(ts);
  if (Number.isNaN(date.getTime())) return "长期有效";
  const hh = String(date.getHours()).padStart(2, "0");
  const mm = String(date.getMinutes()).padStart(2, "0");
  return `${String(date.getMonth() + 1).padStart(2, "0")}/${String(date.getDate()).padStart(2, "0")} ${hh}:${mm}`;
}

export function formatCreditUpdatedAt(ts: number | undefined): string {
  if (!ts) return "—";
  const date = new Date(ts);
  if (Number.isNaN(date.getTime())) return "—";
  return `${String(date.getHours()).padStart(2, "0")}:${String(date.getMinutes()).padStart(2, "0")}`;
}

export function expiryClass(expired: boolean, expiringSoon: boolean): string {
  if (expired) return "text-danger";
  if (expiringSoon) return "text-warning";
  return "text-muted-foreground";
}

export function creditResources(credit?: CreditExpiry): CreditResource[] {
  return (credit?.resources ?? [])
    .filter((resource) => resource.remaining > 0)
    .map((resource, index) => ({ resource, index }))
    .sort((left, right) => {
      const leftExpiry = left.resource.expireAt ?? Number.POSITIVE_INFINITY;
      const rightExpiry = right.resource.expireAt ?? Number.POSITIVE_INFINITY;
      return leftExpiry === rightExpiry ? left.index - right.index : leftExpiry - rightExpiry;
    })
    .map(({ resource }) => resource);
}

export function formatTravelRemaining(arriveAt: number | null | undefined): string | null {
  if (!arriveAt || arriveAt <= 0) return null;
  const arriveMs = arriveAt > 1e12 ? arriveAt : arriveAt * 1000;
  const remainingMs = arriveMs - Date.now();
  if (remainingMs <= 0) return "即将到达";
  const totalMinutes = Math.max(1, Math.ceil(remainingMs / 60_000));
  const hours = Math.floor(totalMinutes / 60);
  const minutes = totalMinutes % 60;
  if (hours > 0 && minutes > 0) return `剩余 ${hours} 小时 ${minutes} 分钟`;
  if (hours > 0) return `剩余 ${hours} 小时`;
  return `剩余 ${minutes} 分钟`;
}

export function travelTooltip(status: TravelStatus): string {
  const place = status.locationName?.trim();
  const credit = status.rewardCredit;
  const points = credit != null ? `+${credit}` : null;
  const remaining = formatTravelRemaining(status.arriveAt);
  if (status.label === "traveling") {
    const parts = [place, points ? `预计 ${points}` : "旅行中", remaining].filter(Boolean);
    return parts.length > 0 ? parts.join(" · ") : "旅行中";
  }
  if (status.label === "finished") {
    if (place && points) return `${place} · ${points}`;
    if (place) return `${place} · 已结束`;
    if (points) return `已结束 · ${points}`;
    return "已结束";
  }
  if (status.label === "no-buddy") return "无 Buddy";
  return "未旅行";
}

export function vscodeExtTooltip(installed?: boolean, extensionInstalled?: boolean): string {
  if (!installed) return "未检测到 VS Code";
  if (!extensionInstalled) return "未检测到 VS Code CodeBuddy 插件";
  return "切换 VS Code CodeBuddy 插件账号（可选复制会话；可自动关闭并重开）";
}

export function jetbrainsTooltip(installed?: boolean, pluginInstalled?: boolean): string {
  if (!installed) return "未检测到 JetBrains IDE";
  if (!pluginInstalled) return "未检测到 JetBrains IDE CodeBuddy 插件";
  return "切换 JetBrains IDE（IDEA / PyCharm）CodeBuddy 插件账号（可自动关闭并重开）";
}

export function formatRateLimitRemaining(resetAt: number, now: number): string {
  const remainingMs = resetAt - now;
  if (remainingMs <= 0) return "即将恢复";
  const totalMinutes = Math.max(1, Math.ceil(remainingMs / 60_000));
  const hours = Math.floor(totalMinutes / 60);
  const minutes = totalMinutes % 60;
  if (hours > 0 && minutes > 0) return `${hours}h${minutes}m 后恢复`;
  if (hours > 0) return `${hours}h 后恢复`;
  return `${minutes}m 后恢复`;
}

export function formatRateLimitClock(resetAt: number, now: number): string {
  const reset = new Date(resetAt);
  const time = `${String(reset.getHours()).padStart(2, "0")}:${String(reset.getMinutes()).padStart(2, "0")}`;
  const midnight = (date: Date) => new Date(date.getFullYear(), date.getMonth(), date.getDate()).getTime();
  const days = Math.round((midnight(reset) - midnight(new Date(now))) / 86_400_000);
  if (days <= 0) return time;
  if (days === 1) return `明天 ${time}`;
  return `${reset.getMonth() + 1}/${reset.getDate()} ${time}`;
}

/* -------------------------------------------------------------------------- */
/* 账号标记（R5-04）                                                          */
/* -------------------------------------------------------------------------- */

/** 标记文本在「…」菜单里的截断长度（菜单宽 160px，8 个字刚好看得出是哪一条）。 */
export const MARK_MENU_TEXT_MAX = 8;

/** 标记是否已过期（`expiresAt === null` = 不自动过期，永远不算过期）。 */
export function isMarkExpired(mark: AccountMark, now: number): boolean {
  return typeof mark.expiresAt === "number" && mark.expiresAt <= now;
}

/**
 * 卡片上应当展示的标记：已过期返回 `null`（图标隐藏），**记录仍然保留**在账号库里
 * ——编辑对话框据此显示「已过期」，用户可以续期或改成永久。
 */
export function activeMark(
  account: { mark?: AccountMark | null },
  now: number,
): AccountMark | null {
  const mark = account.mark;
  if (!mark) return null;
  return isMarkExpired(mark, now) ? null : mark;
}

/**
 * 剩余时长文案：`剩 2 小时` / `剩 32 分钟` / `剩 3 天`；
 * 已过期返回「已过期」，`expiresAt === null`（手动解除）返回 `null`。
 *
 * 与限额倒计时同款「向上取整到分钟」：剩余 30 秒也显示「剩 1 分钟」，
 * 不会出现「剩 0 分钟」这种看起来已经失效的文案。
 */
export function formatMarkRemaining(expiresAt: number | null, now: number): string | null {
  if (typeof expiresAt !== "number" || !Number.isFinite(expiresAt)) return null;
  const remainingMs = expiresAt - now;
  if (remainingMs <= 0) return "已过期";
  const totalMinutes = Math.max(1, Math.ceil(remainingMs / 60_000));
  if (totalMinutes >= 1_440) {
    // ≥1 天按天数表达，避免出现「剩 71 小时」。
    return `剩 ${Math.round(totalMinutes / 1_440)} 天`;
  }
  const hours = Math.floor(totalMinutes / 60);
  const minutes = totalMinutes % 60;
  if (hours > 0 && minutes > 0) return `剩 ${hours} 小时 ${minutes} 分钟`;
  if (hours > 0) return `剩 ${hours} 小时`;
  return `剩 ${minutes} 分钟`;
}

/** 绝对时刻：`2026/11/20 22:50`（本地时区，标记的到期与创建时间都用它）。 */
export function formatMarkClock(ts: number): string {
  const date = new Date(ts);
  if (Number.isNaN(date.getTime())) return "—";
  const pad = (value: number) => String(value).padStart(2, "0");
  return `${date.getFullYear()}/${pad(date.getMonth() + 1)}/${pad(date.getDate())} ${pad(date.getHours())}:${pad(date.getMinutes())}`;
}

/**
 * tooltip 第二行：`至 2026/11/20 22:50 · 剩 2 小时`；
 * 手动解除（`expiresAt === null`）显示「手动解除」；已过期只显示「已过期」。
 */
export function formatMarkExpiryLine(mark: AccountMark, now: number): string {
  if (typeof mark.expiresAt !== "number") return "手动解除";
  const clock = formatMarkClock(mark.expiresAt);
  const remaining = formatMarkRemaining(mark.expiresAt, now);
  if (!remaining || remaining === "已过期") return `至 ${clock} · 已过期`;
  return `至 ${clock} · ${remaining}`;
}

/**
 * 图标的读屏文案：`标记：即将限额，剩 2 小时，点击编辑`。
 *
 * 没有到期时间时省略剩余时长那一段（`标记：即将限额，手动解除，点击编辑`），
 * 过期后固定为「已过期」，保证读屏用户拿到的信息与视觉一致。
 */
export function markAriaLabel(mark: AccountMark, now: number): string {
  const remaining = formatMarkRemaining(mark.expiresAt, now);
  const state = remaining ?? "手动解除";
  return `标记：${mark.text}，${state}，点击编辑`;
}

/** 「…」菜单文案里的标记文本截断：超过 8 个字用省略号。 */
export function truncateMarkText(text: string): string {
  const chars = [...text];
  if (chars.length <= MARK_MENU_TEXT_MAX) return text;
  return `${chars.slice(0, MARK_MENU_TEXT_MAX).join("")}…`;
}
