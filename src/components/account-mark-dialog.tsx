import { useEffect, useMemo, useState } from "react";
import { Loader2, Tag, Trash2 } from "lucide-react";
import { toast } from "sonner";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import * as api from "@/lib/api";
import { cn } from "@/lib/utils";
import type {
  AccountMark,
  AccountMarkColor,
  AccountMeta,
  CreditExpiry,
  RateLimitEntry,
} from "@/lib/types";

import { earliestRemainingExpiry, soonestRateLimitResetAt } from "@/pages/accounts/ordering";

import { formatCreditExpiry, formatMarkClock, formatRateLimitClock, isMarkExpired } from "./account-card/formatters";

/** 标记文本上限：与后端 `MARK_TEXT_MAX_CHARS` 保持一致。 */
const MARK_TEXT_MAX_LENGTH = 16;
const MINUTE = 60_000;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;

/** 常用标记：点击只填入文本，**不改时长**（用户可能只是换个说法）。 */
const QUICK_TEXTS = ["即将限额", "积分将过期", "暂不使用", "待确认"] as const;

/** 5 个语义色圆点（与后端 `MARK_COLORS` 一一对应）。 */
const COLOR_OPTIONS: ReadonlyArray<{ value: AccountMarkColor; label: string; dotClass: string }> = [
  { value: "warning", label: "警示", dotClass: "bg-warning" },
  { value: "danger", label: "紧急", dotClass: "bg-danger" },
  { value: "info", label: "提示", dotClass: "bg-primary" },
  { value: "success", label: "正常", dotClass: "bg-success" },
  { value: "neutral", label: "中性", dotClass: "bg-secondary-foreground" },
];

/** 持续时间档位（默认「不自动过期」）。 */
type DurationKey = "30m" | "2h" | "today" | "3d" | "custom" | "never";

const DURATION_OPTIONS: ReadonlyArray<{ value: DurationKey; label: string }> = [
  { value: "30m", label: "30 分钟" },
  { value: "2h", label: "2 小时" },
  { value: "today", label: "今天结束" },
  { value: "3d", label: "3 天" },
  { value: "custom", label: "自定义…" },
  { value: "never", label: "不自动过期" },
];

/** 本地次日 00:00（「今天结束」，按本地时区计算）。 */
function endOfToday(now: number): number {
  const date = new Date(now);
  return new Date(date.getFullYear(), date.getMonth(), date.getDate() + 1).getTime();
}

/** `<input type="datetime-local">` 的取值格式（本地时区，精确到分钟）。 */
function toDateTimeLocalValue(ms: number): string {
  const date = new Date(ms);
  const pad = (value: number) => String(value).padStart(2, "0");
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}T${pad(date.getHours())}:${pad(date.getMinutes())}`;
}

/** `datetime-local` 文本 → 绝对毫秒；空/非法返回 `null`。 */
function fromDateTimeLocalValue(value: string): number | null {
  if (!value) return null;
  const ms = new Date(value).getTime();
  return Number.isFinite(ms) ? ms : null;
}

/**
 * 由已有的 `expiresAt` 反推档位（编辑态回显）。
 *
 * 与预设差 90 秒以内即视为该预设（用户可能是在「今天结束」附近手动改过时间），
 * 否则落到「自定义…」并用真实时间预填。
 */
function inferDuration(expiresAt: number | null, now: number): DurationKey {
  if (typeof expiresAt !== "number") return "never";
  const diff = expiresAt - now;
  const near = (ms: number) => Math.abs(diff - ms) < 90_000;
  if (near(30 * MINUTE)) return "30m";
  if (near(2 * HOUR)) return "2h";
  if (near(3 * DAY)) return "3d";
  if (Math.abs(expiresAt - endOfToday(now)) < 90_000) return "today";
  return "custom";
}

/** 按档位算绝对到期时刻；`never` 与「自定义未填」返回 `null`（不自动过期）。 */
function resolveExpiry(duration: DurationKey, now: number, customMs: number | null): number | null {
  switch (duration) {
    case "30m":
      return now + 30 * MINUTE;
    case "2h":
      return now + 2 * HOUR;
    case "today":
      return endOfToday(now);
    case "3d":
      return now + 3 * DAY;
    case "custom":
      return customMs;
    case "never":
    default:
      return null;
  }
}

interface Props {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** 目标账号；为空时不渲染内容（关闭动画期间沿用快照）。 */
  account: AccountMeta | null;
  /** 该账号的积分到期信息（智能提示用，与账号页同一份数据）。 */
  credit?: CreditExpiry;
  /** 该账号当前受限的模型（智能提示用）。 */
  rateLimits?: RateLimitEntry[];
  /** 保存 / 移除成功后回调。 */
  onSaved?: (account: AccountMeta) => void;
}

/**
 * 账号标记对话框（R5-04）：自定义文本 + 语义色 + 到期时间，支持手动解除。
 *
 * 标记只写本机账号库（`accounts.json` 的 `mark` 字段），不同步官方；
 * 到期后卡片上的图标自动隐藏，但记录保留，再次打开本弹窗会看到「已过期」并可续期。
 *
 * 智能提示复用账号页排序的取值函数（`soonestRateLimitResetAt` / `earliestRemainingExpiry`），
 * 不在这里另写一份「最早恢复 / 最早到期」的逻辑。
 */
export function AccountMarkDialog({ open, onOpenChange, account, credit, rateLimits, onSaved }: Props) {
  const [text, setText] = useState("");
  const [color, setColor] = useState<AccountMarkColor>("warning");
  const [duration, setDuration] = useState<DurationKey>("never");
  const [customValue, setCustomValue] = useState("");
  const [busy, setBusy] = useState(false);
  // 关闭时父组件会清空 account：保留最后一次的账号继续渲染，让退出动画播完再卸载。
  const [snapshot, setSnapshot] = useState<AccountMeta | null>(null);

  /** 每秒走一次的时钟：让「剩 32 分钟」自己变，不必手动重开弹窗。 */
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!open) return;
    setNow(Date.now());
    const timer = window.setInterval(() => setNow(Date.now()), 1_000);
    return () => window.clearInterval(timer);
  }, [open]);

  // 每次打开按目标账号的现有标记重置草稿。
  useEffect(() => {
    if (!open || !account) return;
    const openedAt = Date.now();
    setNow(openedAt);
    const mark: AccountMark | null = account.mark ?? null;
    setText(mark?.text ?? "");
    setColor(mark?.color ?? "warning");
    setBusy(false);
    if (!mark) {
      setDuration("never");
      setCustomValue(toDateTimeLocalValue(openedAt + 2 * HOUR));
      return;
    }
    // 已过期的标记：保存即「续期」，默认改回「不自动过期」，由用户重新选时长。
    const expired = isMarkExpired(mark, openedAt);
    setDuration(expired ? "never" : inferDuration(mark.expiresAt, openedAt));
    setCustomValue(
      toDateTimeLocalValue(
        typeof mark.expiresAt === "number" && mark.expiresAt > openedAt
          ? mark.expiresAt
          : openedAt + 2 * HOUR,
      ),
    );
  }, [open, account]);

  useEffect(() => {
    if (account) setSnapshot(account);
  }, [account]);

  const current = account ?? snapshot;

  /** 智能提示：限额恢复优先，其次积分到期（同一时间只显示一条）。 */
  const hint = useMemo(() => {
    const limitResetAt = soonestRateLimitResetAt(rateLimits, now);
    if (limitResetAt !== null) {
      return {
        kind: "rateLimit" as const,
        resetAt: limitResetAt,
        label: `该账号 ${formatRateLimitClock(limitResetAt, now)} 解除限额`,
        fillText: "即将限额",
      };
    }
    const creditExpiry = earliestRemainingExpiry(credit);
    if (creditExpiry !== null && creditExpiry > now) {
      return {
        kind: "credit" as const,
        resetAt: creditExpiry,
        label: `最近一笔积分将于 ${formatCreditExpiry(creditExpiry)} 到期`,
        fillText: "积分将过期",
      };
    }
    return null;
  }, [credit, now, rateLimits]);

  if (!current) return null;

  const accountId = current.id;
  const existing = current.mark ?? null;
  const expiredExisting = existing !== null && isMarkExpired(existing, now);
  const trimmedText = text.replace(/[\r\n]/g, "").trim();
  const charCount = [...text.replace(/[\r\n]/g, "")].length;
  const customMs = fromDateTimeLocalValue(customValue);
  const expiry = resolveExpiry(duration, now, customMs);
  const customInvalid = duration === "custom" && (customMs === null || customMs <= now);
  const canSave = trimmedText.length > 0 && !customInvalid && !busy;

  /** 「用作到期时间」：把提示的时刻填进自定义时间，并按需补文本 / 颜色。 */
  function applyHintAsExpiry(resetAt: number, fillText: string) {
    setDuration("custom");
    setCustomValue(toDateTimeLocalValue(resetAt));
    if (!trimmedText) {
      setText(fillText);
      setColor("warning");
    }
  }

  async function save() {
    if (!canSave) return;
    setBusy(true);
    try {
      const res = await api.updateAccountDisplay(accountId, {
        mark: { text: trimmedText, color, expiresAt: expiry },
      });
      onSaved?.(res.account);
      toast.success(existing ? "标记已更新" : "标记已添加", {
        description: expiry === null ? `${trimmedText}（手动解除）` : `${trimmedText}（至 ${formatMarkClock(expiry)}）`,
      });
      onOpenChange(false);
    } catch (e) {
      toast.error("保存标记失败", { description: api.asError(e) });
    } finally {
      setBusy(false);
    }
  }

  async function remove() {
    setBusy(true);
    try {
      const res = await api.updateAccountDisplay(accountId, { mark: null });
      onSaved?.(res.account);
      toast.success("已移除标记");
      onOpenChange(false);
    } catch (e) {
      toast.error("移除标记失败", { description: api.asError(e) });
    } finally {
      setBusy(false);
    }
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle className="flex items-center gap-2">
            <Tag className="size-4" aria-hidden="true" />
            {existing ? "编辑标记" : "添加标记"}
          </DialogTitle>
          <DialogDescription>
            只保存在本机账号库，不同步到官方；到期后自动从卡片隐藏。
          </DialogDescription>
        </DialogHeader>

        {expiredExisting && existing && (
          <p className="rounded-lg border border-warning-border bg-warning-soft px-3 py-2 text-[11px] leading-4 text-warning">
            此标记已于 {formatMarkClock(existing.expiresAt ?? now)} 过期，卡片上已隐藏。保存后按下方选择的时长重新生效
            （默认不自动过期）。
          </p>
        )}

        <div className="space-y-1.5">
          <div className="flex items-baseline justify-between">
            <Label htmlFor="account-mark-text" className="text-xs font-medium text-muted-foreground">
              标记内容
            </Label>
            <span className="text-[11px] tabular-nums text-muted-foreground">
              {charCount}/{MARK_TEXT_MAX_LENGTH}
            </span>
          </div>
          <Input
            id="account-mark-text"
            value={text}
            maxLength={MARK_TEXT_MAX_LENGTH}
            placeholder="例如：即将限额"
            onChange={(e) => setText(e.target.value.replace(/[\r\n]/g, ""))}
          />
          <div className="flex flex-wrap gap-1.5">
            {QUICK_TEXTS.map((quick) => (
              <button
                key={quick}
                type="button"
                // 只填文本，不改时长（用户可能只是想换个说法）。
                onClick={() => setText(quick)}
                className="rounded-md border border-border px-2 py-0.5 text-[11px] text-muted-foreground transition-colors hover:bg-muted hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
              >
                {quick}
              </button>
            ))}
          </div>
        </div>

        <div className="space-y-1.5">
          <Label className="text-xs font-medium text-muted-foreground">颜色</Label>
          <div className="flex items-center gap-2" role="radiogroup" aria-label="标记颜色">
            {COLOR_OPTIONS.map((option) => {
              const selected = color === option.value;
              return (
                <button
                  key={option.value}
                  type="button"
                  role="radio"
                  aria-checked={selected}
                  aria-label={option.label}
                  title={option.label}
                  onClick={() => setColor(option.value)}
                  className={cn(
                    "flex size-6 items-center justify-center rounded-full border transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring",
                    selected ? "border-foreground/40 bg-muted" : "border-transparent hover:bg-muted/60",
                  )}
                >
                  <span className={cn("size-3.5 rounded-full", option.dotClass)} aria-hidden="true" />
                </button>
              );
            })}
          </div>
        </div>

        <div className="space-y-1.5">
          <Label className="text-xs font-medium text-muted-foreground">持续时间</Label>
          <div className="grid grid-cols-3 gap-1.5">
            {DURATION_OPTIONS.map((option) => {
              const selected = duration === option.value;
              return (
                <button
                  key={option.value}
                  type="button"
                  aria-pressed={selected}
                  onClick={() => setDuration(option.value)}
                  className={cn(
                    "rounded-lg border px-2 py-1.5 text-xs transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring",
                    selected
                      ? "border-primary/60 bg-primary/5 font-medium text-foreground"
                      : "border-border text-muted-foreground hover:bg-muted/40",
                  )}
                >
                  {option.label}
                </button>
              );
            })}
          </div>
          {duration === "custom" && (
            <div className="space-y-1">
              <Input
                type="datetime-local"
                aria-label="自定义到期时间"
                value={customValue}
                min={toDateTimeLocalValue(now + MINUTE)}
                onChange={(e) => setCustomValue(e.target.value)}
              />
              {customInvalid && (
                <p className="text-[11px] text-danger">到期时间必须晚于当前时间</p>
              )}
            </div>
          )}
          <p className="text-[11px] leading-4 text-muted-foreground">
            {expiry === null
              ? "不自动过期：标记一直保留，直到手动解除。"
              : `到期时间：${formatMarkClock(expiry)}`}
          </p>
        </div>

        {hint && (
          <div className="flex items-start justify-between gap-2 rounded-lg border border-border bg-muted/40 px-3 py-2">
            <span className="min-w-0 text-[11px] leading-4 text-muted-foreground">{hint.label}</span>
            <Button
              variant="ghost"
              size="sm"
              className="h-6 shrink-0 px-1.5 text-[11px]"
              onClick={() => applyHintAsExpiry(hint.resetAt, hint.fillText)}
            >
              用作到期时间
            </Button>
          </div>
        )}

        <DialogFooter className="sm:justify-between">
          {existing ? (
            <Button variant="ghost" className="text-danger hover:bg-danger-soft hover:text-danger" disabled={busy} onClick={() => void remove()}>
              <Trash2 />
              移除标记
            </Button>
          ) : (
            <Badge variant="secondary" className="self-center text-[11px] font-normal text-muted-foreground">
              仅本机可见
            </Badge>
          )}
          <div className="flex items-center gap-2">
            <Button variant="outline" disabled={busy} onClick={() => onOpenChange(false)}>
              取消
            </Button>
            <Button disabled={!canSave} onClick={() => void save()}>
              {busy && <Loader2 className="animate-spin" />}
              保存标记
            </Button>
          </div>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
