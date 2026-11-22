import { Loader2, Sparkles, X } from "lucide-react";

import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import type { AccountMeta } from "@/lib/types";
import { accountVariant } from "@/lib/variant";

import {
  dailyActivityAccountLabel,
  describeDailyActivityRow,
  summarizeDailyActivity,
} from "../daily-activity";
import type { DailyActivityController } from "../signals/use-daily-activity";

interface Props {
  /** 账号库全量（含另一档位）：结果行的 `accountId` 要在这里查显示名。 */
  accounts: AccountMeta[];
  controller: DailyActivityController;
}

/**
 * 账号页「每日活跃」的二次确认框 + 逐账号结果面板。
 *
 * 为什么要有确认框：这一轮会**真的建会话并跑完**，可能消耗积分、在网页端留下会话记录，
 * 而且同一账号一天只值一次（后端有去重状态）。用户必须显式点「开始」才会发请求。
 *
 * 结果面板的四种状态（成功 / 今日已跳过 / 不支持 / 失败）由 `../daily-activity.ts` 判定，
 * 这里只负责渲染，状态映射本身有单测。
 */
export function DailyActivityPanel({ accounts, controller }: Props) {
  const { state, confirmOpen, cancel, confirm, dismiss } = controller;
  const { running, result, error } = state;
  const internationalCount = accounts.filter((account) => accountVariant(account) === "ai").length;
  const visible = running || result !== null || error !== null;

  return (
    <>
      <AlertDialog
        open={confirmOpen}
        onOpenChange={(open) => {
          if (!open) cancel();
        }}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>为国际版账号跑一轮每日活跃？</AlertDialogTitle>
            <AlertDialogDescription asChild>
              <div className="space-y-1.5 text-left text-xs leading-5">
                <div className="text-foreground">
                  会为每个国际版账号创建一条会话并跑完，可能消耗积分、产生会话记录。
                </div>
                <div>
                  本轮将处理{" "}
                  <span className="font-medium text-foreground tabular-nums">
                    {internationalCount}
                  </span>{" "}
                  个国际版账号；国内版账号与缺少凭据的账号不发请求，会在结果里标注「不支持」。
                </div>
                <div>
                  今天已经完成过的账号会直接跳过（每日去重）。每个账号最多等一轮会话，
                  账号多时可能要几分钟，期间请勿关闭窗口。
                </div>
                {internationalCount === 0 && (
                  <div>当前账号库里没有国际版账号，点「开始」只会得到一份「不支持」的结果。</div>
                )}
              </div>
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>取消</AlertDialogCancel>
            <AlertDialogAction onClick={confirm}>开始</AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>

      {visible && (
        <div
          className="mb-4 rounded-xl border border-border bg-muted/30 px-4 py-3"
          role="status"
          aria-live="polite"
          aria-busy={running || undefined}
        >
          <div className="flex items-start justify-between gap-3">
            <div className="min-w-0">
              <h3 className="flex items-center gap-1.5 text-sm font-semibold text-foreground">
                <Sparkles className="size-3.5" aria-hidden="true" />
                每日活跃
                {result && (
                  <span className="font-normal text-muted-foreground tabular-nums">
                    {result.date}
                  </span>
                )}
              </h3>
              <p className="mt-1 text-xs leading-5 text-muted-foreground">
                {running
                  ? "正在为账号逐个跑会话（建会话 → 跑完 → 收尾），请勿关闭窗口…"
                  : result
                    ? summarizeDailyActivity(result)
                    : "上一轮没有拿到结果"}
              </p>
            </div>
            <div className="flex shrink-0 items-center gap-1">
              {running && (
                <Loader2 className="size-4 animate-spin text-muted-foreground" aria-hidden="true" />
              )}
              <Button
                variant="ghost"
                size="icon"
                className="size-7 rounded-md"
                onClick={dismiss}
                disabled={running}
                aria-label="关闭每日活跃结果面板"
                title={running ? "本轮还在进行中，完成后再关闭" : "关闭"}
              >
                <X />
              </Button>
            </div>
          </div>

          {/*
            整体失败（例如连不上本地服务）：只在面板里加一条红色说明，**不清空**已有结果行，
            用户仍能看到上一轮的逐账号结论。
          */}
          {error && (
            <p className="mt-2 rounded-md border border-destructive/30 bg-destructive/10 px-2.5 py-1.5 text-xs leading-5 text-destructive">
              本轮失败：{error}
            </p>
          )}

          {result && result.accounts.length > 0 && (
            <ul className="mt-3 space-y-1.5" role="list">
              {result.accounts.map((row) => {
                const view = describeDailyActivityRow(row);
                const label = dailyActivityAccountLabel(accounts, row.accountId);
                return (
                  <li
                    key={row.accountId || `${row.phase}-${row.elapsedMs}`}
                    className="flex items-start justify-between gap-3 rounded-lg border border-border/60 bg-background/60 px-3 py-2"
                  >
                    <div className="min-w-0">
                      <p className="truncate text-xs font-medium text-foreground">{label}</p>
                      <p className="mt-0.5 text-[11px] leading-4 text-muted-foreground">
                        {view.detail}
                      </p>
                    </div>
                    <Badge variant={view.badge} className="mt-0.5 shrink-0">
                      {view.label}
                    </Badge>
                  </li>
                );
              })}
            </ul>
          )}
        </div>
      )}
    </>
  );
}
