import { useState } from "react";
import { toast } from "sonner";
import { Settings } from "lucide-react";

import { cn } from "@/lib/utils";
import * as api from "@/lib/api";
import { Button } from "@/components/ui/button";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { Switch } from "@/components/ui/switch";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { changeCompanionEnabled, useCompanionEnabled } from "@/hooks/use-companion-enabled";
// 侧边栏底部的角色头像：取 assets/mascot 的透明底成品（mascot-color-128.png）。
// 不要用 source/mascot-color.jpg —— 那是白底原图，会在图标周围留一块白色方形底衬。
// 关闭态用 grayscale 呈现为灰色，开启后才露出品牌蓝。
import companionMark from "../../../assets/mascot/mascot-color-128.png";

/**
 * 悬浮窗面板：从 footer 向上滑出，开关与设置同屏。
 *
 * footer 只保留一个「图标位」——点图标弹面板，而不是并排第二个按钮：
 * 开关与设置都在面板里，关掉悬浮窗时也不必留一个灰掉的死图标。
 * 面板关闭后进入设置的常驻路径仍是：设置页 → 会话悬浮窗 → 悬浮窗设置。
 */
export const COMPANION_MENU_CONTENT = { side: "top", align: "start", sideOffset: 8, className: "w-56" } as const;

export function CompanionFooter() {
  const { enabled, busy } = useCompanionEnabled();
  /** 面板打开时收起 hover tooltip——两者都朝上弹，同时出现会叠在一起。 */
  const [menuOpen, setMenuOpen] = useState(false);
  const [tipOpen, setTipOpen] = useState(false);

  async function onToggle() {
    if (enabled === null) return;
    try {
      const confirmed = await changeCompanionEnabled(!enabled);
      toast.success(confirmed ? "已启用会话悬浮窗" : "已关闭会话悬浮窗");
    } catch (cause) {
      toast.error("悬浮窗设置失败", { description: api.asError(cause) });
    }
  }

  async function openSettings() {
    try {
      await api.openCompanionSettings();
    } catch (cause) {
      toast.error("打开悬浮窗设置失败", { description: api.asError(cause) });
    }
  }

  return (
    <DropdownMenu open={menuOpen} onOpenChange={setMenuOpen}>
      <Tooltip open={tipOpen && !menuOpen} onOpenChange={setTipOpen}>
        <TooltipTrigger asChild>
          <DropdownMenuTrigger asChild>
            <Button type="button" variant="ghost" size="icon" className="size-8 shrink-0 rounded-lg" aria-label="会话悬浮窗" disabled={enabled === null || busy}>
              <img src={companionMark} alt="" className={cn("size-6 shrink-0 object-contain transition-[filter,opacity]", !enabled && "grayscale opacity-90")} />
            </Button>
          </DropdownMenuTrigger>
        </TooltipTrigger>
        <TooltipContent side="top">会话悬浮窗</TooltipContent>
      </Tooltip>
      <DropdownMenuContent {...COMPANION_MENU_CONTENT}>
        {/* 整行即开关（方向键 / Enter 同语义）；行尾 Switch 只作视觉，点击穿透到行上，避免出现两个焦点。 */}
        <DropdownMenuItem
          className="gap-3"
          disabled={enabled === null || busy}
          onSelect={(event) => {
            // 面板保持打开：切换是异步的，让用户看到开关落定。
            event.preventDefault();
            void onToggle();
          }}
        >
          <span className="min-w-0 flex-1">会话悬浮窗</span>
          <Switch checked={Boolean(enabled)} tabIndex={-1} aria-hidden className="pointer-events-none" />
        </DropdownMenuItem>
        <p className="px-2.5 pb-1 text-xs leading-4 text-muted-foreground">
          {enabled ? "关闭后悬浮栏将隐藏，并停止 WorkBuddy Shift 中的会话监听。" : "开启后会显示悬浮栏，并开始监听会话状态。"}
        </p>
        <DropdownMenuSeparator />
        <DropdownMenuItem disabled={enabled === null || busy || !enabled} onSelect={() => void openSettings()}>
          <Settings />悬浮窗设置
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
