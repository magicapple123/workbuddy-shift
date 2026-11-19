import { ArrowUp, Loader2 } from "lucide-react";

import { StatusDot } from "@/components/product-marks";
import { openUpdateDialog } from "@/components/update-host";
import { Button } from "@/components/ui/button";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import * as api from "@/lib/api";
import { demoModeEnabled } from "@/lib/demo-mode";
import { useUpdateState } from "@/hooks/use-update-state";
import { useAccountsStore } from "@/stores/accounts";

import { CompanionDemoFooter } from "./companion-demo-footer";
import { CompanionFooter } from "./companion-footer";

export function UpdateCenter({ running }: { running: boolean | undefined }) {
  const version = useAccountsStore((s) => s.status?.version);
  const snapshot = useUpdateState();
  const showCompanion = api.isDesktop() && !demoModeEnabled;
  // 演示模式只提供只读演示浮层，不渲染正式版的悬浮窗开关。
  const showCompanionDemo = demoModeEnabled;

  // 阶段由 Rust 更新服务经 `update-state` 推送（托盘同源），前端不再轮询检查。
  // 已知目标版本时，检查中 / 失败也要保留入口，与托盘「升级到 vX / 点击重试」对齐。
  const hasKnownTarget = Boolean(snapshot.latest);
  const hasUpdate =
    snapshot.phase === "available" ||
    snapshot.phase === "downloading" ||
    snapshot.phase === "readyToRestart" ||
    (hasKnownTarget && (snapshot.phase === "error" || snapshot.phase === "checking"));
  const updateHint =
    snapshot.phase === "downloading"
      ? snapshot.percent === null
        ? "正在下载更新…"
        : `正在下载更新 ${snapshot.percent}%`
      : snapshot.phase === "readyToRestart"
        ? "重启以完成升级"
        : snapshot.phase === "error"
          ? "更新失败，点击重试"
          : snapshot.phase === "checking"
            ? "正在检查…"
            : "更新";

  return (
    <section className="mt-auto border-t border-sidebar-border px-2 pt-3 text-xs">
      <div className="flex items-center gap-2 text-[13px] text-sidebar-foreground">
        {showCompanion ? <CompanionFooter /> : showCompanionDemo ? <CompanionDemoFooter /> : (
          <>
            <StatusDot on={Boolean(running)} />
            <span className="min-w-0 flex-1 truncate">WorkBuddy</span>
          </>
        )}
        <div className="ml-auto flex shrink-0 items-center gap-1.5">
          <span className="text-sidebar-foreground/50">v{version || "?"}</span>
          {hasUpdate && (
            <Tooltip>
              <TooltipTrigger asChild>
                <Button
                  type="button"
                  size="icon"
                  className="size-5 rounded-full p-0"
                  aria-label={updateHint}
                  onClick={() => openUpdateDialog()}
                >
                  {snapshot.phase === "downloading" || snapshot.phase === "checking" ? (
                    <Loader2 className="size-3 animate-spin" strokeWidth={2.5} aria-hidden="true" />
                  ) : (
                    <ArrowUp className="size-3" strokeWidth={2.5} aria-hidden="true" />
                  )}
                </Button>
              </TooltipTrigger>
              <TooltipContent side="top">{updateHint}</TooltipContent>
            </Tooltip>
          )}
        </div>
      </div>
    </section>
  );
}
