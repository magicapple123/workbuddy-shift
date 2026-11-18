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

export function AccountCardHeader({ account, compact = false, codebuddyCliActive, codebuddyCliBusy, codebuddyCliConfigured, codebuddyCliLoading, codebuddyCnIdeActive, codebuddyCnIdeAvailable, codebuddyCnIdeBusy, codebuddyCnIdeLoading, enabledTools, featuresDisabled, jetbrainsActive, jetbrainsAvailable, jetbrainsBusy, jetbrainsInstalled, jetbrainsLoading, jetbrainsPluginInstalled, name, onCheckin, onDelete, onRefresh, onShowInfo, onSwitch, onSwitchCodebuddyCli, onSwitchCodebuddyCnIde, onSwitchJetbrains, onSwitchVscodeExt, statusChips, avatarClass, todayCheckedIn, vscodeExtActive, vscodeExtAvailable, vscodeExtBusy, vscodeExtExtensionInstalled, vscodeExtInstalled, vscodeExtLoading, workbuddyActive }: Props) {
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
          {workbuddyActive && (
            <div className={cn("absolute top-[64%] -translate-y-1/2 opacity-[0.075] saturate-50 grayscale-[10%]", codebuddyCliActive ? "right-[68px] rotate-[8deg]" : "right-5 rotate-[7deg]")}>
              <WorkBuddyMark size={compact ? 40 : 56} />
            </div>
          )}
          {codebuddyCliActive && (
            <div className={cn("absolute top-[63%] -translate-y-1/2 opacity-[0.065] saturate-50 grayscale-[18%]", workbuddyActive ? "right-1 -rotate-[8deg]" : "right-5 -rotate-[7deg]")}>
              <CodeBuddyMark size={compact ? 38 : 54} />
            </div>
          )}
          {codebuddyCnIdeActive && (
            /* 复用 WorkBuddy 的 SVG：两个产品的标记同形；CodeBuddyCnIdeMark 是位图 app 图标，
               放大到水印尺寸会是一块模糊方块。位置与旋转与 WorkBuddy 水印相同 —— 两者同时选中时
               完全重合，因此无需再引入第三套偏移规则。 */
            <div className={cn("absolute top-[64%] -translate-y-1/2 opacity-[0.075] saturate-50 grayscale-[10%]", codebuddyCliActive ? "right-[68px] rotate-[8deg]" : "right-5 rotate-[7deg]")}>
              <WorkBuddyMark size={compact ? 40 : 56} />
            </div>
          )}
          {vscodeExtActive && (
            /* 插件标记是 currentColor 字形（无底块）：显式取品牌绿，与 WorkBuddy 水印同色系；
               位置规则同 WorkBuddy / IDE，两者同时选中时重合（同样无需第三套偏移规则）。 */
            <div className={cn("absolute top-[64%] -translate-y-1/2 text-primary opacity-[0.075] saturate-50 grayscale-[10%]", codebuddyCliActive ? "right-[68px] rotate-[8deg]" : "right-5 rotate-[7deg]")}>
              <VscodeExtMark size={compact ? 40 : 56} />
            </div>
          )}
          {jetbrainsActive && (
            /* JetBrains 水印与 VS Code 同规则（深色字块 + 品牌色 tint），多产品同时
               选中时按既有约定与 WorkBuddy 水印重合，不引入新的偏移规则。 */
            <div className={cn("absolute top-[64%] -translate-y-1/2 text-primary opacity-[0.075] saturate-50 grayscale-[10%]", codebuddyCliActive ? "right-[68px] rotate-[8deg]" : "right-5 rotate-[7deg]")}>
              <JetbrainsMark size={compact ? 40 : 56} />
            </div>
          )}
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
          <div className="relative z-10 flex w-full min-w-0 items-center gap-2 pr-10">
            <h3 className="min-w-0 flex-1 truncate text-[13px] font-semibold leading-5" title={name}>{name}</h3>
            <div className="hidden shrink-0 items-center gap-1 min-[420px]:flex">{statusChips}</div>
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
        ) : (
          <div className={cn("relative z-10 flex w-full min-w-0 items-center gap-3", workbuddyActive || codebuddyCliActive ? "pr-[112px]" : "pr-10")}>
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
