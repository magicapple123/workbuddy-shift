import { Ellipsis, Folder, MessageCircle, Trash2 } from "lucide-react";
import { CodeBuddyCnIdeMark, VscodeExtMark, WorkBuddyAiMark, WorkBuddyMark } from "@/components/product-marks";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { Skeleton } from "@/components/ui/skeleton";
import { formatDateTime } from "@/lib/format";
import type {
  SessionGroupClient,
  SessionGroupSummary,
  WbVariant,
} from "@/lib/types";

export function ClientMark({ client, variantScope }: { client: SessionGroupClient; variantScope?: WbVariant }) {
  if (client === "workbuddy") return variantScope === "ai" ? <WorkBuddyAiMark size={34} /> : <WorkBuddyMark size={34} />;
  if (client === "codebuddyIde") return <CodeBuddyCnIdeMark size={34} />;
  return <VscodeExtMark size={34} className="text-foreground" />;
}

export function GroupSkeleton() {
  return <div className="grid min-w-0 grid-cols-1 gap-3 sm:grid-cols-2" aria-label="正在加载会话组">
    {Array.from({ length: 4 }, (_, index) => <Card key={index} className="gap-3 rounded-xl py-3 shadow-none"><CardContent className="space-y-3 px-3"><Skeleton className="h-4 w-2/3" /><Skeleton className="h-3 w-1/2" /><Skeleton className="h-7 w-full" /></CardContent></Card>)}
  </div>;
}

export function SessionGroupCard({ group, selected, onSelect, onRequestDelete }: { group: SessionGroupSummary; selected: boolean; onSelect: () => void; onRequestDelete: () => void }) {
  // 「…」不能嵌在整卡按钮里（button 不能嵌套 button），所以绝对定位到卡片右上角。
  return <div className="relative min-w-0">
    <button type="button" onClick={onSelect} aria-pressed={selected} className={`w-full min-w-0 cursor-pointer rounded-lg border bg-card px-4 py-3.5 text-left transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring ${selected ? "border-brand shadow-sm" : "border-border hover:border-brand/50"}`}>
      <div className="flex min-w-0 items-center justify-between gap-2 pr-7 text-xs text-muted-foreground"><span className="flex min-w-0 items-center gap-2"><Folder className="size-4 shrink-0" /><span className="truncate" title={group.projectLabel}>{group.projectLabel || "未标记项目"}</span></span><span className="shrink-0" title={formatDateTime(group.latestActivityAt)}>{relativeDate(group.latestActivityAt)}</span></div>
      <h3 className="my-3 truncate text-base font-semibold" title={group.title}>{group.title}</h3>
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
