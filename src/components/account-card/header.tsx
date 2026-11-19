import { CircleCheck, Ellipsis, Info, Loader2, RefreshCw, Trash2 } from "lucide-react";
import type { ReactNode } from "react";

import { Button } from "@/components/ui/button";
import { DemoAction } from "@/components/demo-action";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { CodeBuddyCnIdeMark, CodeBuddyMark, JetbrainsMark, VscodeExtMark, WorkBuddyMark } from "@/components/product-marks";
import { accountIdentity } from "@/lib/account-display";
import { demoModeEnabled } from "@/lib/demo-mode";
import { toolEntryEnabled } from "@/lib/supported-tools";
import { cn } from "@/lib/utils";

import type { AccountCardProps } from "./types";
import { jetbrainsTooltip, vscodeExtTooltip } from "./status-chips";
import { ProductCurrentState } from "./tool-actions";

interface Props extends AccountCardProps {
  name: string;
  avatarClass: string;
  statusChips: ReactNode;
}

/** 同时在线多端时水印的扇位（第三轮第 14 条）。 */
type WatermarkKind = "workbuddy" | "codebuddyIde" | "vscodeExt" | "jetbrains" | "codebuddyCli";

/**
 * 水印槽位：固定坐标，不做 DOM 测量（纯 CSS 定位）。
 *
 * 三个槽位的 size 与不透明度递减，避免「三个 56px 全叠在右侧」再次糊成一片；
 * 横向占用 16–114px，与文本区的 `pr-[112px]` 对得上，也不碰右上角的「…」按钮。
 * 之前按「CLI 是否在线」写死 `right-[68px]` / `right-1`，一旦「支持工具」开关
 * 增减入口就会失配，所以这里只按索引定位。
 */
const WATERMARK_SLOTS = [
  { position: "right-4 top-[68%] rotate-[7deg]", size: 56, opacity: "opacity-[0.075]" },
  { position: "right-[46px] top-[54%] -rotate-[10deg]", size: 42, opacity: "opacity-[0.06]" },
  { position: "right-[80px] top-[70%] rotate-[14deg]", size: 34, opacity: "opacity-[0.05]" },
] as const;

/** 水印里最多画 3 个：第 4、5 个不增加信息量，图标行已经完整表达。 */
const MAX_WATERMARKS = WATERMARK_SLOTS.length;

/**
 * 水印字形。`codebuddyIde` 复用 WorkBuddy 的 SVG（`CodeBuddyCnIdeMark` 是位图 app
 * 图标，放大到水印尺寸会是一块模糊方块），而 VS Code / JetBrains 的标记是
 * `currentColor` 字形，需要外层显式给品牌色。
 */
function WatermarkMark({ kind, size }: { kind: WatermarkKind; size: number }) {
  switch (kind) {
    case "workbuddy":
    case "codebuddyIde":
      return <WorkBuddyMark size={size} />;
    case "vscodeExt":
      return <VscodeExtMark size={size} />;
    case "jetbrains":
      return <JetbrainsMark size={size} />;
    case "codebuddyCli":
      return <CodeBuddyMark size={size} />;
  }
}

/** 只有 `currentColor` 字形需要外层染色；两个 SVG 标记自带填充色。 */
function watermarkTint(kind: WatermarkKind): string | undefined {
  return kind === "vscodeExt" || kind === "jetbrains" ? "text-primary" : undefined;
}

export function AccountCardHeader({ account, compact = false, codebuddyCliActive, codebuddyCliBusy, codebuddyCliConfigured, codebuddyCliLoading, codebuddyCnIdeActive, codebuddyCnIdeAvailable, codebuddyCnIdeBusy, codebuddyCnIdeLoading, enabledTools, featuresDisabled, jetbrainsActive, jetbrainsAvailable, jetbrainsBusy, jetbrainsInstalled, jetbrainsLoading, jetbrainsPluginInstalled, name, onCheckin, onDelete, onRefresh, onShowInfo, onSwitch, onSwitchCodebuddyCli, onSwitchCodebuddyCnIde, onSwitchJetbrains, onSwitchVscodeExt, statusChips, avatarClass, todayCheckedIn, vscodeExtActive, vscodeExtAvailable, vscodeExtBusy, vscodeExtExtensionInstalled, vscodeExtInstalled, vscodeExtLoading, workbuddyActive }: Props) {
  /**
   * 活跃端按固定优先级收集：workbuddy > codebuddyIde > vscodeExt > jetbrains > codebuddyCli。
   *
   * 同字形去重：`codebuddyIde` 与 `workbuddy` 是同一张剪影，WorkBuddy 在线时跳过前者，
   * 否则会出现两个一模一样的剪影并排。取前 3 个按索引落槽（见 WATERMARK_SLOTS）。
   */
  const activeWatermarks: WatermarkKind[] = (
    [
      workbuddyActive ? "workbuddy" : null,
      codebuddyCnIdeActive ? "codebuddyIde" : null,
      vscodeExtActive ? "vscodeExt" : null,
      jetbrainsActive ? "jetbrains" : null,
      codebuddyCliActive ? "codebuddyCli" : null,
    ] as (WatermarkKind | null)[]
  )
    .filter((kind): kind is WatermarkKind => kind !== null)
    .filter((kind) => !(kind === "codebuddyIde" && workbuddyActive))
    .slice(0, MAX_WATERMARKS);

  return (
      <header
        className={cn(
          "relative flex items-center border-b border-border",
          compact ? "min-h-[52px] px-3.5 py-1.5" : "min-h-[104px] px-5 py-3",
          /* 选中态染色，优先级：任一产品在用（主色淡底）> CodeBuddy CLI（中性灰）> 默认。
             多个产品同时选中时取优先级最高者；具体哪几个产品在使用由 header 的标记+勾选角标表达。
             原任务用 CodeBuddy IDE 专属的 violet-500 区分产品，主题里没有对应语义 token，
             且客户端差异已由产品图标与水印表达，故统一收敛到品牌主色淡底（bg-primary-soft）。 */
          workbuddyActive || vscodeExtActive || codebuddyCnIdeActive ? "bg-primary-soft/60" : codebuddyCliActive ? "bg-muted/60" : "bg-muted/30",
        )}
      >
        <div className="pointer-events-none absolute inset-0 overflow-hidden">
          <div
            className={cn(
              "absolute -right-10 -top-16 rounded-full blur-2xl",
              compact ? "size-20" : "size-24",
              workbuddyActive || vscodeExtActive || codebuddyCnIdeActive ? "bg-primary-soft" : codebuddyCliActive ? "bg-muted/50" : "bg-muted/30",
            )}
          />
          {/*
            水印只在非紧凑卡渲染（第三轮第 14 条）：紧凑卡只有 52px 高，图标行与「…」
            按钮就在右侧，任何水印都会压住可点区域。紧凑卡右上仍保留上面的 blur 光斑。
            渲染顺序反转，让索引 0 画在最上层（扇心最清晰）。
          */}
          {!compact &&
            [...activeWatermarks].reverse().map((kind) => {
              const slot = WATERMARK_SLOTS[activeWatermarks.indexOf(kind)];
              return (
                <div
                  key={kind}
                  className={cn(
                    "absolute -translate-y-1/2 saturate-50 grayscale-[10%]",
                    slot.position,
                    slot.opacity,
                    watermarkTint(kind),
                  )}
                >
                  <WatermarkMark kind={kind} size={slot.size} />
                </div>
              );
            })}
        </div>

        <div className={cn("absolute z-20", compact ? "right-2.5 top-1/2 -translate-y-1/2" : "right-3.5 top-3.5")}>
          {/* 演示模式只保留「账号信息」（纯本地查看 / 备注），写操作菜单项不渲染。 */}
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <Button variant="ghost" size="icon" className={cn("rounded-lg text-muted-foreground hover:text-foreground", compact ? "size-7" : "size-8")} aria-label={`管理账号 ${name}`} title="更多账号操作">
                <Ellipsis />
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end" className="w-40">
              {!demoModeEnabled && (
                <DropdownMenuItem disabled={featuresDisabled || !onRefresh} onSelect={() => onRefresh?.(account)}>
                  <RefreshCw />刷新 Token
                </DropdownMenuItem>
              )}
              <DropdownMenuItem disabled={featuresDisabled || !onShowInfo} onSelect={() => onShowInfo?.(account)}>
                <Info />账号信息
              </DropdownMenuItem>
              {!demoModeEnabled && onCheckin && todayCheckedIn !== true && (
                <DropdownMenuItem disabled={featuresDisabled || !onCheckin} onSelect={() => onCheckin?.(account)}>
                  <CircleCheck />手动签到
                </DropdownMenuItem>
              )}
              {!demoModeEnabled && (
                <>
                  <DropdownMenuSeparator />
                  <DropdownMenuItem className="text-danger focus:bg-danger-soft focus:text-danger" onSelect={() => onDelete(account)}>
                    <Trash2 />删除账号
                  </DropdownMenuItem>
                </>
              )}
            </DropdownMenuContent>
          </DropdownMenu>
        </div>

        {compact ? (
          <div className="relative z-10 flex w-full min-w-0 flex-col gap-1 pr-10">
            <div className="flex w-full min-w-0 items-center gap-2">
            <h3 className="min-w-0 flex-1 truncate text-[13px] font-semibold leading-5" title={name}>{name}</h3>
            <div className="ml-auto flex shrink-0 items-center gap-1">
              {toolEntryEnabled(enabledTools, "workbuddy") && (workbuddyActive ? (
                <ProductCurrentState product="workbuddy" compact iconOnly />
              ) : demoModeEnabled ? (
                <DemoAction>
                  <Button variant="outline" size="icon" className="size-7 rounded-lg" aria-label="设为 WorkBuddy 当前账号">
                    <WorkBuddyMark size={15} />
                  </Button>
                </DemoAction>
              ) : (
                <Tooltip>
                  <TooltipTrigger asChild>
                    <Button variant="outline" size="icon" className="size-7 rounded-lg" disabled={featuresDisabled || !onSwitch} onClick={() => onSwitch?.(account)} aria-label="设为 WorkBuddy 当前账号">
                      <WorkBuddyMark size={15} />
                    </Button>
                  </TooltipTrigger>
                  <TooltipContent side="top">设为 WorkBuddy 当前账号（会重启 WorkBuddy）</TooltipContent>
                </Tooltip>
              ))}
              {toolEntryEnabled(enabledTools, "codebuddyIde") && (codebuddyCnIdeActive ? (
                <ProductCurrentState product="codebuddy-cn" compact iconOnly />
              ) : demoModeEnabled ? (
                <DemoAction>
                  <Button variant="outline" size="icon" className="relative size-7 rounded-lg" aria-label="切换到 CodeBuddy IDE">
                    <CodeBuddyCnIdeMark size={15} />
                  </Button>
                </DemoAction>
              ) : (
                <Tooltip>
                  <TooltipTrigger asChild>
                    <Button variant="outline" size="icon" className="relative size-7 rounded-lg" disabled={featuresDisabled || !codebuddyCnIdeAvailable || !onSwitchCodebuddyCnIde || codebuddyCnIdeBusy} onClick={() => onSwitchCodebuddyCnIde?.(account)} aria-label={codebuddyCnIdeLoading ? "正在切换 CodeBuddy IDE" : "切换到 CodeBuddy IDE"} aria-busy={codebuddyCnIdeLoading}>
                      {codebuddyCnIdeLoading ? <Loader2 className="size-3.5 animate-spin" /> : <CodeBuddyCnIdeMark size={15} />}
                    </Button>
                  </TooltipTrigger>
                  <TooltipContent side="top">{codebuddyCnIdeAvailable ? "切换到 CodeBuddy IDE（会重启 IDE）" : "未检测到 CodeBuddy IDE"}</TooltipContent>
                </Tooltip>
              ))}
              {toolEntryEnabled(enabledTools, "vscodeExt") && (vscodeExtActive ? (
                <ProductCurrentState product="vscode-ext" compact iconOnly />
              ) : demoModeEnabled ? (
                <DemoAction>
                  <Button variant="outline" size="icon" className="relative size-7 rounded-lg" aria-label="切换到 VS Code CodeBuddy 插件（可复制会话）">
                    <VscodeExtMark size={15} />
                  </Button>
                </DemoAction>
              ) : (
                <Tooltip>
                  <TooltipTrigger asChild>
                    <Button variant="outline" size="icon" className="relative size-7 rounded-lg" disabled={featuresDisabled || !vscodeExtAvailable || !onSwitchVscodeExt || vscodeExtBusy} onClick={() => onSwitchVscodeExt?.(account)} aria-label="切换到 VS Code CodeBuddy 插件（可复制会话）" aria-busy={vscodeExtLoading}>
                      {vscodeExtLoading ? <Loader2 className="size-3.5 animate-spin" /> : <VscodeExtMark size={15} />}
                    </Button>
                  </TooltipTrigger>
                  <TooltipContent side="top">{vscodeExtTooltip(vscodeExtInstalled, vscodeExtExtensionInstalled)}</TooltipContent>
                </Tooltip>
              ))}
              {toolEntryEnabled(enabledTools, "jetbrains") && (jetbrainsActive ? (
                <ProductCurrentState product="jetbrains" compact iconOnly />
              ) : demoModeEnabled ? (
                <DemoAction>
                  <Button variant="outline" size="icon" className="relative size-7 rounded-lg" aria-label="切换到 JetBrains IDE CodeBuddy 插件">
                    <JetbrainsMark size={15} />
                  </Button>
                </DemoAction>
              ) : (
                <Tooltip>
                  <TooltipTrigger asChild>
                    <Button variant="outline" size="icon" className="relative size-7 rounded-lg" disabled={featuresDisabled || !jetbrainsAvailable || !onSwitchJetbrains || jetbrainsBusy} onClick={() => onSwitchJetbrains?.(account)} aria-label="切换到 JetBrains IDE CodeBuddy 插件" aria-busy={jetbrainsLoading}>
                      {jetbrainsLoading ? <Loader2 className="size-3.5 animate-spin" /> : <JetbrainsMark size={15} />}
                    </Button>
                  </TooltipTrigger>
                  <TooltipContent side="top">{jetbrainsTooltip(jetbrainsInstalled, jetbrainsPluginInstalled)}</TooltipContent>
                </Tooltip>
              ))}
              {toolEntryEnabled(enabledTools, "codebuddyCli") && (codebuddyCliActive ? (
                <ProductCurrentState product="codebuddy" compact iconOnly />
              ) : demoModeEnabled ? (
                <DemoAction>
                  <Button variant="outline" size="icon" className="size-7 rounded-lg" aria-label="设为 CodeBuddy CLI 当前账号">
                    <CodeBuddyMark size={15} />
                  </Button>
                </DemoAction>
              ) : (
                <Tooltip>
                  <TooltipTrigger asChild>
                    <Button variant="outline" size="icon" className="size-7 rounded-lg" disabled={featuresDisabled || !codebuddyCliConfigured || !onSwitchCodebuddyCli || codebuddyCliBusy} onClick={() => onSwitchCodebuddyCli?.(account)} aria-label={codebuddyCliLoading ? "正在切换 CodeBuddy CLI 当前账号" : "设为 CodeBuddy CLI 当前账号"} aria-busy={codebuddyCliLoading}>
                      {codebuddyCliLoading ? <Loader2 className="size-3.5 animate-spin" /> : <CodeBuddyMark size={15} />}
                    </Button>
                  </TooltipTrigger>
                  <TooltipContent side="top">{codebuddyCliConfigured ? "设为 CodeBuddy CLI 当前账号" : "请先接入 CodeBuddy CLI"}</TooltipContent>
                </Tooltip>
              ))}
            </div>
            </div>
            {/* 限额等文案 chip 单独一行：与账号名争宽度会把名字挤成 0（1280 宽下必现）。 */}
            <div className="flex min-w-0 flex-wrap items-center gap-1 empty:hidden">{statusChips}</div>
          </div>
        ) : (
          /* 文本让位按「有没有水印」而不是按产品：之前只有 VS Code / JetBrains 在线时
             仍只留 pr-10，文字会压到扇形水印上。 */
          <div className={cn("relative z-10 flex w-full min-w-0 items-center gap-3", activeWatermarks.length > 0 ? "pr-[112px]" : "pr-10")}>
            <div className={cn("flex size-12 shrink-0 items-center justify-center rounded-full text-base font-semibold ring-4 ring-card/65", avatarClass)}>{name.charAt(0).toUpperCase()}</div>
            <div className="min-w-0 flex-1">
              <h3 className="truncate text-sm font-semibold leading-5" title={name}>{name}</h3>
              <p className="mt-0.5 truncate text-xs leading-5 text-muted-foreground" title={account.email || account.uid || account.id}>{accountIdentity(account)}</p>
              <div className="mt-1.5 flex min-w-0 flex-wrap items-center gap-1.5">{statusChips}</div>
            </div>
          </div>
        )}
      </header>
  );
}
