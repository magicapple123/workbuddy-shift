import { useState } from "react";
import { toast } from "sonner";
import { Play, Settings } from "lucide-react";

import { CompanionDemoDialog } from "@/components/companion-demo-dialog";
import { Button } from "@/components/ui/button";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { Switch } from "@/components/ui/switch";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { DEMO_UNAVAILABLE_MESSAGE } from "@/lib/demo-mode";
import companionMark from "../../../assets/mascot/mascot-color-128.png";

import { COMPANION_MENU_CONTENT } from "./companion-footer";

/**
 * 演示模式的悬浮窗入口：与桌面正式版同形（一个图标位 + 向上弹出的面板），
 * 但面板里的开关与设置只提示「演示不可用」，不触发任何本机命令；
 * 多一条「查看悬浮栏演示」，打开只读演示浮层。
 */
export function CompanionDemoFooter() {
  const [open, setOpen] = useState(false);
  /** 面板打开时收起 hover tooltip，与正式版同一处理。 */
  const [menuOpen, setMenuOpen] = useState(false);
  const [tipOpen, setTipOpen] = useState(false);

  /** 演示面板里的行：保持可聚焦（键盘也能走一遍），但只说明不可用。 */
  function unavailable(event: Event) {
    event.preventDefault();
    toast.info(DEMO_UNAVAILABLE_MESSAGE);
  }

  return (
    <DropdownMenu open={menuOpen} onOpenChange={setMenuOpen}>
      <Tooltip open={tipOpen && !menuOpen} onOpenChange={setTipOpen}>
        <TooltipTrigger asChild>
          <DropdownMenuTrigger asChild>
            <Button type="button" variant="ghost" size="icon" className="size-8 shrink-0 rounded-lg" aria-label="会话悬浮窗">
              <img src={companionMark} alt="" className="size-6 shrink-0 object-contain" />
            </Button>
          </DropdownMenuTrigger>
        </TooltipTrigger>
        <TooltipContent side="top">会话悬浮窗</TooltipContent>
      </Tooltip>
      <DropdownMenuContent {...COMPANION_MENU_CONTENT}>
        <DropdownMenuItem className="gap-3" onSelect={unavailable}>
          <span className="min-w-0 flex-1">会话悬浮窗</span>
          <Switch checked tabIndex={-1} aria-hidden className="pointer-events-none" />
        </DropdownMenuItem>
        <p className="px-2.5 pb-1 text-xs leading-4 text-muted-foreground">关闭后悬浮栏将隐藏，并停止 WorkBuddy Shift 中的会话监听。</p>
        <DropdownMenuSeparator />
        <DropdownMenuItem onSelect={unavailable}>
          <Settings />悬浮窗设置
        </DropdownMenuItem>
        <DropdownMenuSeparator />
        <DropdownMenuItem onSelect={() => setOpen(true)}>
          <Play />查看悬浮栏演示
        </DropdownMenuItem>
      </DropdownMenuContent>
      <CompanionDemoDialog open={open} onOpenChange={setOpen} />
    </DropdownMenu>
  );
}
