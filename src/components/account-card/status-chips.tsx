import { Fragment, type ReactNode } from "react";
import { CalendarCheck2, CalendarDays, CalendarOff, Gauge, PlaneTakeoff, Star } from "lucide-react";

import { Badge } from "@/components/ui/badge";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";
import type {
  AccountMark,
  AccountMeta,
  RateLimitBorrowed,
  RateLimitEntry,
  TravelStatus,
} from "@/lib/types";

import { chipClass } from "./chip-class";
import {
  formatRateLimitClock,
  formatRateLimitRemaining,
  jetbrainsTooltip,
  travelTooltip,
  vscodeExtTooltip,
} from "./formatters";
import { AccountMarkChip } from "./mark-chip";
import { HiddenStatusChips } from "./overflow-chip";
import { selectCompactStatusChips, STATUS_PRIORITY, type StatusChipDescriptor } from "./status-selection";

// 兼容既有导入方（`credit-section.tsx` 仍从本模块取 chipClass）。
export { chipClass };

/**
 * 卡片头部的状态 chip 区：标记 / 企业版 / 签到 / 旅行 / 限额 / 失效 / 建议优先 / 多端在用。
 *
 * R5-05 起把「一串条件 JSX」重构成数据数组 `StatusChipDescriptor[]`：
 * - 非紧凑卡：全部渲染，顺序与改造前一致（视觉零变化）；
 * - 紧凑卡：按优先级只留 3 个槽位（标记钉住、永不收起），其余折叠成 `+N`，
 *   保证 340px 宽度下账号名可读、标记不被挤掉。
 */
export function AccountStatusChips({
  account,
  activeProductCount,
  autoCheckinAllowed,
  compact,
  creditPriority,
  now,
  onEditMark,
  rateLimits,
  rateLimitBorrowed,
  todayCheckedIn,
  travelStatus,
}: {
  account: AccountMeta;
  /** 正在使用的客户端数量（≥2 且非紧凑时显示汇总 chip）。 */
  activeProductCount: number;
  autoCheckinAllowed?: boolean;
  compact: boolean;
  creditPriority?: boolean;
  /** 限额倒计时的本地时钟（每秒推进）。 */
  now: number;
  /** 点击标记图标 → 打开标记对话框（有标记时才渲染图标）。 */
  onEditMark?: () => void;
  rateLimits?: RateLimitEntry[];
  rateLimitBorrowed?: RateLimitBorrowed | null;
  todayCheckedIn?: boolean;
  travelStatus?: TravelStatus;
}) {
  const chips: StatusChipDescriptor[] = [];

  // 1）用户标记（R5-04）：永远排在状态区首位，且钉住不被 `+N` 收起。
  //    已过期的标记在这里被过滤掉（`activeMark`），但记录仍保留在账号库里。
  const mark = activeMarkOf(account, now);
  if (mark) {
    chips.push({
      key: "mark",
      priority: STATUS_PRIORITY.mark,
      pinned: true,
      detail: mark.text,
      node: <AccountMarkChip mark={mark} now={now} onEdit={onEditMark} />,
    });
  }

  // 2）企业版标注（issue #167）：身份属性优先于状态 chip。
  //    紧凑卡片单行头部放不下（会挤掉账号名），徽标改在积分区显示。
  if (!compact && account.isEnterprise) {
    chips.push({
      key: "enterprise",
      priority: STATUS_PRIORITY.enterprise,
      detail: "企业版",
      node: <Badge variant="secondary" className={chipClass}>企业版</Badge>,
    });
  }

  // 3）自动签到被关闭（配置状态）。
  if (autoCheckinAllowed === false) {
    chips.push({
      key: "autoCheckinOff",
      priority: STATUS_PRIORITY.autoCheckinOff,
      detail: "自动签到已关闭",
      node: statusIconChip({
        icon: <CalendarOff className="size-3.5" />,
        label: "自动签到已关闭",
        tooltip: `${todayCheckedIn === undefined ? "" : todayCheckedIn ? "今日已签到。" : "今日未签到。"}该账号已关闭自动签到，刷新时也会忽略，仍可手动签到`,
        variant: "secondary",
      }),
    });
  }

  // 4）今日签到状态。
  if (autoCheckinAllowed !== false && todayCheckedIn !== undefined) {
    chips.push(
      todayCheckedIn
        ? {
            key: "checkinDone",
            priority: STATUS_PRIORITY.checkinDone,
            detail: "今日已签到",
            node: statusIconChip({
              icon: <CalendarCheck2 className="size-3.5" />,
              label: "今日已签到",
              tooltip: "今日已签到",
              variant: "success",
            }),
          }
        : {
            key: "checkinPending",
            priority: STATUS_PRIORITY.checkinPending,
            detail: "今日未签到",
            node: statusIconChip({
              icon: <CalendarDays className="size-3.5" />,
              label: "今日未签到",
              tooltip: "今日未签到",
              variant: "secondary",
            }),
          },
    );
  }

  // 5）旅行状态（纯信息）。
  const travel = travelChip(travelStatus, compact);
  if (travel && travelStatus) {
    chips.push({
      key: "travel",
      priority: STATUS_PRIORITY.travel,
      detail: travelTooltip(travelStatus),
      node: travel,
    });
  }

  // 6）模型限额（直接影响当前任务）。
  const limits = rateLimitInfo(rateLimits, now, rateLimitBorrowed);
  if (limits) {
    chips.push({
      key: "rateLimit",
      priority: STATUS_PRIORITY.rateLimit,
      detail: limits.label,
      node: rateLimitChip(rateLimits, now, rateLimitBorrowed, compact),
    });
  }

  // 7）需重新登录 / Token 已过期（阻断使用）。
  const reloginLabel = account.needsRelogin
    ? "需重新登录"
    : typeof account.expiresAt === "number" && account.expiresAt < Date.now()
      ? "Token 已过期"
      : null;
  if (reloginLabel) {
    chips.push({
      key: "relogin",
      priority: STATUS_PRIORITY.relogin,
      detail: reloginLabel,
      node: <Badge variant="warning" className={chipClass}>{reloginLabel}</Badge>,
    });
  }

  // 8）建议优先使用（有即将到期积分）。
  if (creditPriority) {
    chips.push({
      key: "creditPriority",
      priority: STATUS_PRIORITY.creditPriority,
      detail: "建议优先使用",
      node: (
        <Tooltip>
          <TooltipTrigger asChild>
            <Badge variant="warning" className={cn(chipClass, "px-1")} aria-label="建议优先">
              <Star className="size-3.5" />
            </Badge>
          </TooltipTrigger>
          <TooltipContent side="top">建议优先使用</TooltipContent>
        </Tooltip>
      ),
    });
  }

  // 9）多端在用汇总（非紧凑，纯信息）。
  if (!compact && activeProductCount >= 2) {
    chips.push({
      key: "multiTool",
      priority: STATUS_PRIORITY.multiTool,
      detail: `${activeProductCount} 个工具正在使用`,
      node: (
        <Badge variant="secondary" className={cn(chipClass, "text-muted-foreground")}>
          {activeProductCount} 个工具正在使用
        </Badge>
      ),
    });
  }

  // 非紧凑卡：全部渲染，不排序、不截断（与改造前视觉一致）。
  if (!compact) {
    return (
      <>
        {chips.map((chip) => (
          <Fragment key={chip.key}>{chip.node}</Fragment>
        ))}
      </>
    );
  }

  // 紧凑卡：3 槽 + `+N`。
  const { visible, hidden } = selectCompactStatusChips(chips);
  return (
    <>
      {visible.map((chip) => (
        <Fragment key={chip.key}>{chip.node}</Fragment>
      ))}
      {hidden.length > 0 && <HiddenStatusChips hidden={hidden} />}
    </>
  );
}

/** 卡片上应当展示的标记（已过期返回 null，与 `formatters.activeMark` 同语义）。 */
function activeMarkOf(account: AccountMeta, now: number): AccountMark | null {
  const mark = account.mark;
  if (!mark) return null;
  if (typeof mark.expiresAt === "number" && mark.expiresAt <= now) return null;
  return mark;
}

export function statusIconChip({
  icon,
  label,
  tooltip,
  variant,
  count,
  muted,
}: {
  icon: ReactNode;
  label: string;
  tooltip: ReactNode;
  variant: "secondary" | "success" | "warning" | "danger";
  /** 数量角标；≤1 时不显示（单个受限模型不需要角标）。 */
  count?: number;
  /** 置灰（未激活状态，如「未旅行」）：图标与角标一起使用 muted 前景色。 */
  muted?: boolean;
}) {
  const badge = count != null && count > 1;
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <Badge
          variant={variant}
          className={cn(chipClass, "px-1", badge && "gap-0.5", muted && "text-muted-foreground")}
          aria-label={label}
        >
          {icon}
          {badge ? (
            <span
              aria-hidden="true"
              className="flex h-3 min-w-3 items-center justify-center rounded-full bg-warning px-0.5 text-[9px] font-semibold leading-none tabular-nums text-white"
            >
              {count}
            </span>
          ) : null}
        </Badge>
      </TooltipTrigger>
      <TooltipContent side="top">{tooltip}</TooltipContent>
    </Tooltip>
  );
}

function travelIconChip({
  label,
  tooltip,
  variant,
  muted,
}: {
  label: string;
  tooltip: string;
  variant: "secondary" | "success";
  muted?: boolean;
}) {
  return statusIconChip({ icon: <PlaneTakeoff className="size-3.5" />, label, tooltip, variant, muted });
}

export function travelChip(status: TravelStatus | undefined, compact = false) {
  if (!status) return null;
  // 紧凑卡片：只保留「正在旅行 / 已结束」这类有信息量的状态；「未旅行」「无 Buddy」
  // 是多数账号的常态，留着只会和账号名、限额 chip 争宽度。
  if (compact && (status.label === "untraveled" || status.label === "no-buddy")) return null;
  switch (status.label) {
    case "no-buddy":
      return <Badge variant="secondary" className={cn(chipClass, "text-muted-foreground")}>无 Buddy</Badge>;
    case "traveling":
      return travelIconChip({ label: travelTooltip(status), tooltip: travelTooltip(status), variant: "secondary" });
    case "finished":
      return travelIconChip({ label: travelTooltip(status), tooltip: travelTooltip(status), variant: "success" });
    case "untraveled":
    default:
      // 未旅行是多数账号的常态：只留置灰图标，文案交给 tooltip。
      return travelIconChip({ label: "未旅行", tooltip: "未旅行", variant: "secondary", muted: true });
  }
}

/** 来源标签（与后端 `limits::payload::source_label` 同一套取值）。 */
const SOURCE_LABELS: Record<string, string> = {
  "workbuddy-cn": "WorkBuddy",
  "workbuddy-ai": "WorkBuddy 国际版",
  "codebuddy-cli": "CodeBuddy CLI",
  "codebuddy-ide": "CodeBuddy IDE",
  "codebuddy-cn-ide": "CodeBuddy CN IDE",
  "vscode-ext": "VS Code 插件",
};

const sourceLabelOf = (source?: string) =>
  source ? (SOURCE_LABELS[source] ?? "其它客户端") : null;

/** 限额的纯文本摘要（chip 的 aria-label 与 `+N` 的 detail 共用，保证两处文案一致）。 */
function rateLimitInfo(
  limits: RateLimitEntry[] | undefined,
  now: number,
  borrowed?: RateLimitBorrowed | null,
): { label: string; lines: string[]; count: number; borrowed: RateLimitBorrowed | null } | null {
  const active = (limits ?? [])
    .filter((limit) => Number.isFinite(limit.resetAt) && limit.resetAt > now)
    .sort((left, right) => left.resetAt - right.resetAt);
  const ownVscode = active.some((limit) => limit.source === "vscode-ext");
  const activeBorrowed =
    borrowed && Number.isFinite(borrowed.resetAt) && borrowed.resetAt > now && !ownVscode
      ? borrowed
      : null;
  if (active.length === 0 && !activeBorrowed) return null;
  const lines = active.map((limit) => {
    const source = sourceLabelOf(limit.source);
    return `${limit.model ?? "未知模型"} · ${formatRateLimitRemaining(limit.resetAt, now)}（${formatRateLimitClock(limit.resetAt, now)}）${source ? ` · 来自${source}` : ""}`;
  });
  if (activeBorrowed) {
    const clock = formatRateLimitClock(activeBorrowed.resetAt, now);
    return {
      // 文案用**真实来源**（WorkBuddy / WorkBuddy 国际版 / CodeBuddy CLI…），不再笼统说
      // 「其它客户端」：用户看到 WorkBuddy 桌面端限流却写着「其它客户端」会以为是误报。
      label: `${activeBorrowed.sourceLabel} 限流至 ${clock}`,
      lines,
      count: active.length,
      borrowed: activeBorrowed,
    };
  }
  return { label: `模型限额：${lines.join("；")}`, lines, count: active.length, borrowed: null };
}

export function rateLimitChip(
  limits: RateLimitEntry[] | undefined,
  now: number,
  borrowed?: RateLimitBorrowed | null,
  /** 紧凑卡片空间不足：兜底提示退化为图标 + tooltip，避免挤掉账号名。 */
  compact = false,
) {
  const info = rateLimitInfo(limits, now, borrowed);
  if (!info) return null;
  const { label, lines, count, borrowed: activeBorrowed } = info;

  // 13-B：VS Code 无自有记录时只给「兜底提示」，不把其它客户端的模型限额冒充成 VS Code 的。
  if (activeBorrowed) {
    const tooltip = (
      <span className="flex flex-col gap-0.5 text-[11px] leading-4">
        <span>来自 {activeBorrowed.sourceLabel} 的记录；当前客户端用到同一账号/模型时可能同样受限。</span>
        {lines.map((line) => (
          <span key={line}>{line}</span>
        ))}
      </span>
    );
    // 紧凑卡片：图标 + tooltip，给账号名让出宽度。
    if (compact) {
      return statusIconChip({
        icon: <Gauge className="size-3.5" />,
        label,
        tooltip,
        variant: "warning",
      });
    }
    return (
      <Tooltip>
        <TooltipTrigger asChild>
          <Badge variant="warning" className={cn(chipClass, "gap-1 px-1.5")}>
            <Gauge className="size-3.5" aria-hidden="true" />
            {label}
          </Badge>
        </TooltipTrigger>
        <TooltipContent side="top">{tooltip}</TooltipContent>
      </Tooltip>
    );
  }
  return statusIconChip({
    icon: <Gauge className="size-3.5" />,
    label: `模型限额：${lines.join("；")}`,
    // 多模型时会有多行，字号比全局 TooltipContent（text-xs）再小一号。
    tooltip: (
      <span className="flex flex-col gap-0.5 text-[11px] leading-4">
        {lines.map((line) => (
          <span key={line}>{line}</span>
        ))}
      </span>
    ),
    variant: "warning",
    count: count,
  });
}

export { jetbrainsTooltip, vscodeExtTooltip };
