import { useEffect, useState } from "react";
import { toast } from "sonner";
import { Accordion } from "@/components/ui/accordion";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
  AlertDialogTrigger,
} from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import { CardContent } from "@/components/ui/card";
import * as api from "@/lib/api";
import type {
  AppNotification,
} from "@/lib/types";

import { cn } from "@/lib/utils";
import { AccordionSettingsRow, INSET_PANEL, SettingsGroup } from "./rows";

/** 外观：主题选择（持久化到 localStorage）。 */
const NOTIFICATION_LEVEL_LABEL: Record<AppNotification["level"], string> = {
  success: "成功",
  error: "错误",
  warning: "警告",
  info: "提示",
};

const NOTIFICATION_LEVEL_DOT: Record<AppNotification["level"], string> = {
  success: "bg-primary",
  error: "bg-destructive",
  warning: "bg-warning",
  info: "bg-muted-foreground/60",
};

/** 通知时间：当天只显示时分秒，更早显示完整时间。 */
function formatNotificationTime(at: number): string {
  const date = new Date(at);
  const sameDay = date.toDateString() === new Date().toDateString();
  return sameDay
    ? date.toLocaleTimeString("zh-CN", { hour12: false })
    : date.toLocaleString("zh-CN", { hour12: false });
}

/** 通知历史：最近 100 条应用内提示，供事后核对。 */
export function NotificationHistoryCard() {
  /** 手风琴展开的面板：应用内提示存档，默认收起。 */
  const [openSections, setOpenSections] = useState<string[]>([]);
  const open = openSections.includes("history");
  const [items, setItems] = useState<AppNotification[] | null>(null);
  const [error, setError] = useState("");

  useEffect(() => {
    /*
      挂载即预取，展开时再刷一次（T05 第 6 项）。
      此前只在展开时请求，导致「清空」按钮在收起态永远是禁用的（`items === null`），
      用户必须先展开才能清空——而清空按钮本来就在收起态可见。
      演示模式不预取：那条命令不在只读白名单里，预取只会把「演示模式不支持」的错误
      提前显示在面板里；演示页要展开时才看到提示即可。
    */
    if (!open && api.isDemoMode()) return;
    let cancelled = false;
    setError("");
    api
      .listNotifications()
      .then((res) => {
        if (!cancelled) setItems(res.items);
      })
      .catch((e) => {
        if (cancelled) return;
        setItems(null);
        setError(api.asError(e));
      });
    return () => {
      cancelled = true;
    };
  }, [open]);

  async function clearHistory() {
    try {
      await api.clearNotifications();
      setItems([]);
      toast.success("通知历史已清空");
    } catch (e) {
      toast.error("清空通知历史失败", { description: api.asError(e) });
    }
  }

  return (
    <SettingsGroup id="settings-notifications" title="通知历史">
      <CardContent className="space-y-0 p-0">
        <Accordion type="multiple" value={openSections} onValueChange={setOpenSections}>
          <AccordionSettingsRow
            value="history"
            label="应用内提示存档"
            description="保留最近 100 条，便于事后核对；本机明文保存，可能含账号昵称与本地路径。"
            divider={false}
            actions={
              <AlertDialog>
                <AlertDialogTrigger asChild>
                  <Button variant="ghost" size="sm" disabled={!items || items.length === 0}>
                    清空
                  </Button>
                </AlertDialogTrigger>
                <AlertDialogContent>
                  <AlertDialogHeader>
                    <AlertDialogTitle>清空通知历史？</AlertDialogTitle>
                    <AlertDialogDescription>
                      将删除本机保存的全部提示存档，无法恢复。
                    </AlertDialogDescription>
                  </AlertDialogHeader>
                  <AlertDialogFooter>
                    <AlertDialogCancel>取消</AlertDialogCancel>
                    <AlertDialogAction onClick={clearHistory}>清空</AlertDialogAction>
                  </AlertDialogFooter>
                </AlertDialogContent>
              </AlertDialog>
            }
          >
            <div className={INSET_PANEL}>
              {error ? (
                <p className="py-2 text-xs text-danger">{error}</p>
              ) : !items ? (
                <p className="py-2 text-xs text-muted-foreground">正在读取…</p>
              ) : items.length === 0 ? (
                <p className="py-2 text-xs text-muted-foreground">还没有记录到任何提示。</p>
              ) : (
                <ul className="max-h-72 divide-y divide-border/40 overflow-auto">
                  {items.map((item, index) => (
                    <li key={`${item.at}-${index}`} className="py-1.5">
                      <div className="flex items-center gap-1.5 text-[11px] leading-4 text-muted-foreground">
                        <span
                          className={cn(
                            "size-1.5 shrink-0 rounded-full",
                            NOTIFICATION_LEVEL_DOT[item.level],
                          )}
                          aria-hidden
                        />
                        <span>{NOTIFICATION_LEVEL_LABEL[item.level]}</span>
                        <span aria-hidden>·</span>
                        <span>{formatNotificationTime(item.at)}</span>
                      </div>
                      <div className="mt-0.5 text-xs leading-5">{item.title}</div>
                      {item.description && (
                        <div className="mt-0.5 break-all text-xs leading-5 text-muted-foreground">
                          {item.description}
                        </div>
                      )}
                    </li>
                  ))}
                </ul>
              )}
            </div>
          </AccordionSettingsRow>
        </Accordion>
      </CardContent>
    </SettingsGroup>
  );
}
