import { useEffect, useState } from "react";

import { TooltipProvider } from "@/components/ui/tooltip";
import { displayName } from "@/lib/account-display";
import { avatarTone } from "@/lib/avatar-tone";

import { AccountCardActions } from "./account-card/tool-actions";
import { AccountCardHeader } from "./account-card/header";
import { AccountCreditSection } from "./account-card/credit-section";
import type { AccountCardProps } from "./account-card/types";
import { AccountStatusChips } from "./account-card/status-chips";

export function AccountCard(props: AccountCardProps) {
  const {
    account,
    autoCheckinAllowed,
    codebuddyCliActive,
    codebuddyCnIdeActive,
    compact = false,
    creditPriority,
    jetbrainsActive,
    rateLimits,
    todayCheckedIn,
    travelStatus,
    vscodeExtActive,
    workbuddyActive,
  } = props;
  const [now, setNow] = useState(() => Date.now());
  /**
   * 限额图标要随官方恢复时刻自动消失（AC3），所以本地每秒走一次时钟。
   * 只在确实有受限模型时才开定时器，普通卡片不引入额外开销。
   */
  useEffect(() => {
    if (!rateLimits?.length) return;
    const timer = window.setInterval(() => setNow(Date.now()), 1_000);
    return () => window.clearInterval(timer);
  }, [rateLimits]);
  const name = displayName(account);
  const avatarClass = avatarTone(name);
  const activeProductCount = [workbuddyActive, codebuddyCliActive, codebuddyCnIdeActive, vscodeExtActive, jetbrainsActive].filter(Boolean).length;

  return (
    <TooltipProvider>
      <article className="flex min-w-0 flex-col overflow-hidden rounded-[14px] border border-border bg-card shadow-[var(--shadow-card),inset_0_1px_0_var(--card-highlight)] transition-shadow hover:shadow-[var(--shadow-card)]">
        <AccountCardHeader
          {...props}
          name={name}
          avatarClass={avatarClass}
          statusChips={
            <AccountStatusChips
              account={account}
              activeProductCount={activeProductCount}
              autoCheckinAllowed={autoCheckinAllowed}
              compact={compact}
              creditPriority={creditPriority}
              now={now}
              rateLimits={rateLimits}
              todayCheckedIn={todayCheckedIn}
              travelStatus={travelStatus}
            />
          }
        />
        <AccountCreditSection
          account={account}
          compact={compact}
          credit={props.credit}
          creditLoading={props.creditLoading}
          creditUpdatedAt={props.creditUpdatedAt}
          name={name}
        />
        {!compact && <AccountCardActions {...props} />}
      </article>
    </TooltipProvider>
  );
}
