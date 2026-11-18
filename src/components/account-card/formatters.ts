import type { CreditExpiry, CreditResource, TravelStatus } from "@/lib/types";

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
