import { useEffect, useState } from "react";
import { Loader2 } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { CardContent } from "@/components/ui/card";
import * as api from "@/lib/api";
import { canPersistErrorLog } from "@/lib/error-report";
import { cn } from "@/lib/utils";
import { DemoAction } from "@/components/demo-action";
import { SettingsGroup } from "./rows";

/** 错误日志：前端崩溃与未捕获错误的落盘位置（排障用；与通知历史同为事后核对入口）。 */
export function ErrorLogCard() {
  const [path, setPath] = useState<string | null>(null);
  const [pathError, setPathError] = useState(false);
  const [revealing, setRevealing] = useState(false);
  // 浏览器演示页没有 Tauri，`isWebui()` 也为真；截图仍要看到路径和按钮。
  const showReveal = canPersistErrorLog() || api.isDemoMode();

  useEffect(() => {
    let cancelled = false;
    void api
      .getErrorLogPath()
      .then((value) => {
        if (!cancelled) setPath(value);
      })
      .catch(() => {
        if (!cancelled) {
          setPath("");
          setPathError(true);
        }
      });
    return () => {
      cancelled = true;
    };
  }, []);

  async function revealLog() {
    setRevealing(true);
    try {
      await api.revealErrorLog();
    } catch (e) {
      toast.error("打开日志位置失败", { description: api.asError(e) });
    } finally {
      setRevealing(false);
    }
  }

  const pathText =
    path === null
      ? "正在读取…"
      : path
        ? path
        : pathError
          ? "未能读取错误日志路径"
          : "浏览器模式下不会写入本机错误日志";

  return (
    <SettingsGroup id="settings-error-log" title="错误日志">
      <CardContent className="space-y-0 p-0">
        <div className="border-b border-border/50 px-4 py-3 text-xs leading-5 text-muted-foreground sm:px-5">
          {showReveal
            ? "界面崩溃与未捕获的错误会记录在这里（最多保留最近 200 条），反馈问题时可直接附上。"
            : "浏览器模式下，界面错误只会在页面上提示，不会写入本机错误日志。"}
        </div>
        <div
          className={cn(
            "break-all bg-foreground/[0.04] px-4 py-3 font-mono text-[11px] leading-5 text-muted-foreground sm:px-5",
            showReveal && "border-b border-border/50",
          )}
        >
          {pathText}
        </div>
        {showReveal && (
          <div className="flex flex-wrap gap-2 px-4 py-3 sm:px-5">
            <DemoAction>
              <Button
                size="sm"
                variant="outline"
                disabled={revealing || !path}
                onClick={() => void revealLog()}
              >
                {revealing ? <Loader2 className="animate-spin" /> : null}打开日志位置
              </Button>
            </DemoAction>
          </div>
        )}
      </CardContent>
    </SettingsGroup>
  );
}
