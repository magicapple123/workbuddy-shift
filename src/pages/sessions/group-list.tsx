import { useState } from "react";
import { ChevronDown, Ellipsis, Folder, MessageCircle, Trash2 } from "lucide-react";
import { CodeBuddyCnIdeMark, JetbrainsMark, VscodeExtMark, WorkBuddyAiMark, WorkBuddyMark } from "@/components/product-marks";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { Skeleton } from "@/components/ui/skeleton";
import { formatDateTime } from "@/lib/format";
import type {
  SessionGroupClient,
  SessionGroupSummary,
  WbVariant,
} from "@/lib/types";
import type { SessionGroupSection } from "./hooks/use-session-groups";

export function ClientMark({ client, variantScope }: { client: SessionGroupClient; variantScope?: WbVariant }) {
  if (client === "workbuddy") return variantScope === "ai" ? <WorkBuddyAiMark size={34} /> : <WorkBuddyMark size={34} />;
  if (client === "codebuddyIde") return <CodeBuddyCnIdeMark size={34} />;
  if (client === "jetbrains") return <JetbrainsMark size={34} className="text-foreground" />;
  return <VscodeExtMark size={34} className="text-foreground" />;
}

export function GroupSkeleton() {
  return <div className="grid min-w-0 grid-cols-1 gap-3 sm:grid-cols-2" aria-label="正在加载会话组">
    {Array.from({ length: 4 }, (_, index) => <Card key={index} className="gap-3 rounded-xl py-3 shadow-none"><CardContent className="space-y-3 px-3"><Skeleton className="h-4 w-2/3" /><Skeleton className="h-3 w-1/2" /><Skeleton className="h-7 w-full" /></CardContent></Card>)}
  </div>;
}

export function SessionGroupCard({ group, selected, onSelect, onRequestDelete }: { group: SessionGroupSummary; selected: boolean; onSelect: () => void; onRequestDelete: () => void }) {
  // JetBrains 的「工作区」不是 md5(工作目录) 而是安装级会话桶（全机同一个值），只有项目名能
  // 区分同一标题的两条会话（「无项目」桶 vs 某个项目桶）——所以第 4 张卡把项目放到**行尾**。
  // 其余三个客户端维持既有版式：位置标签在行首、相对时间在行尾。
  const projectAtTail = group.client === "jetbrains";
  const projectLabel = group.projectLabel || "未标记项目";
  // 「…」不能嵌在整卡按钮里（button 不能嵌套 button），所以绝对定位到卡片右上角。
  return <div className="relative min-w-0">
    <button type="button" onClick={onSelect} aria-pressed={selected} className={`w-full min-w-0 cursor-pointer rounded-lg border bg-card px-4 py-3.5 text-left transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring ${selected ? "border-brand shadow-sm" : "border-border hover:border-brand/50"}`}>
      <div className="flex min-w-0 items-center justify-between gap-2 pr-7 text-xs text-muted-foreground">
        <span className="flex min-w-0 items-center gap-2">
          <Folder className="size-4 shrink-0" />
          {projectAtTail
            ? <span className="shrink-0 tabular-nums" title={formatDateTime(group.latestActivityAt)}>{relativeDate(group.latestActivityAt)}</span>
            : <span className="truncate" title={projectLabel}>{projectLabel}</span>}
        </span>
        <span className="flex min-w-0 items-center gap-2">
          {projectAtTail && <span className="truncate text-right" title={projectLabel}>{projectLabel}</span>}
          {!projectAtTail && <span className="shrink-0" title={formatDateTime(group.latestActivityAt)}>{relativeDate(group.latestActivityAt)}</span>}
        </span>
      </div>
      <h3 className={`truncate text-base font-semibold ${group.excerpt ? "mb-1.5 mt-3" : "my-3"}`} title={group.title}>{group.title}</h3>
      {/* 摘要：一眼看出「这条会话聊了什么」。没有可读消息时不占位（不渲染空行）。 */}
      {group.excerpt && <p className="mb-3 line-clamp-2 break-words text-xs leading-5 text-muted-foreground" title={group.excerpt}>{group.excerpt}</p>}
      <div className="flex flex-wrap items-center justify-between gap-2 text-sm text-muted-foreground"><span className="flex items-center gap-2" title={group.accountNames.join("、")}><MessageCircle className="size-4" />{group.memberCount} 个账号</span><span className="flex min-w-0 items-center gap-2 text-xs"><span className={`size-2 shrink-0 rounded-full ${group.summaryStatus === "latest" ? "bg-success" : group.summaryStatus === "behind" ? "bg-warning" : group.summaryStatus === "diverge" ? "bg-danger" : "bg-muted-foreground"}`} /><span className="truncate" title={group.summaryText}>{{ latest: "内容一致", behind: "待同步", diverge: "有分歧", missing: "内容缺失", unknown: "无法确认" }[group.summaryStatus]}</span></span></div>
    </button>
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button variant="ghost" size="icon" className="absolute right-2 top-2.5 size-6 text-muted-foreground hover:text-foreground" aria-label={`${group.title} 的操作`} title="更多操作"><Ellipsis className="size-3.5" /></Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="w-36">
        <DropdownMenuItem onSelect={onRequestDelete}><Trash2 />删除关联</DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  </div>;
}

function relativeDate(timestamp: number): string {
  if (!timestamp) return "—";
  const minutes = Math.floor((Date.now() - timestamp) / 60000);
  if (minutes < 1) return "刚刚";
  if (minutes < 60) return `${minutes}分钟前`;
  if (minutes < 1440) return `${Math.floor(minutes / 60)}小时前`;
  return formatDateTime(timestamp);
}

/**
 * 按项目分组的一节：`项目 X · N 个会话` 折叠节头 + 节内卡片。
 *
 * 默认展开（`useState(true)`）：分组的意义是「先看到项目」，默认全收起会变成多一次点击。
 * 折叠状态是**本节局部**的：换页、搜索、刷新都不该把它记到别处（不保存可推导状态）。
 */
export function ProjectSection({ section, columns, selectedGroupId, onSelectGroup, onRequestDelete }: {
  section: SessionGroupSection;
  /** 节内卡片网格的列数类名，与平铺模式共用同一套断点。 */
  columns: string;
  selectedGroupId: string | null;
  onSelectGroup: (groupId: string) => void;
  onRequestDelete: (group: SessionGroupSummary) => void;
}) {
  const [open, setOpen] = useState(true);
  const projectLabel = section.project || "未标记项目";
  return <Collapsible open={open} onOpenChange={setOpen} asChild>
    <section className="min-w-0" aria-label={`项目 ${projectLabel}`}>
      <CollapsibleTrigger asChild>
        <button type="button" className="flex w-full min-w-0 cursor-pointer items-center gap-2 rounded-lg px-1 py-1.5 text-left text-xs text-muted-foreground hover:bg-accent/60 hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring">
          <ChevronDown className={`size-4 shrink-0 transition-transform ${open ? "" : "-rotate-90"}`} aria-hidden="true" />
          <span className="truncate font-medium text-foreground" title={projectLabel}>{projectLabel}</span>
          <span className="shrink-0 tabular-nums" aria-label={`${section.groups.length} 个会话`}>· {section.groups.length} 个会话</span>
        </button>
      </CollapsibleTrigger>
      <CollapsibleContent className="min-w-0">
        <div className={`mt-2 grid min-w-0 gap-3 ${columns}`}>
          {section.groups.map((group) => <SessionGroupCard key={group.key} group={group} selected={group.groupId === selectedGroupId} onSelect={() => onSelectGroup(group.groupId)} onRequestDelete={() => onRequestDelete(group)} />)}
        </div>
      </CollapsibleContent>
    </section>
  </Collapsible>;
}
