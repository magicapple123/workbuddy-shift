import { useEffect, useRef, useState } from "react";
import { ArrowUpCircle, X } from "lucide-react";

import { DemoAction } from "@/components/demo-action";
import { Button } from "@/components/ui/button";
import { GITHUB_RELEASE_URL, openReleaseUrl } from "@/lib/update";
import { cn } from "@/lib/utils";

import { notesNeedToggle, publishedLabel, cleanNotes } from "./update-prompt";

interface UpdateAvailableCardProps {
  /** 目标版本号（无 `v` 前缀）。 */
  version: string;
  /** 发布说明（清单原文，可能是一整份 Release Markdown）；没有说明时为 null。 */
  notes?: string | null;
  /** 发布日期（ISO 8601 原文）。 */
  publishedAt?: string | null;
  /** 「查看完整说明」跳转的 Release 页。 */
  releaseUrl?: string;
  /** 主操作：打开更新弹窗并开始下载。 */
  onInstall: () => void;
  /** 「稍后」/ 关闭：本次会话不再提示这个版本。 */
  onDismiss: () => void;
}

/**
 * 右下角「发现新版本」非模态卡片。
 *
 * 四条硬约束：
 * 1. **非模态**：`role="status"` + 不铺遮罩、不抢焦点，主界面照常可操作；
 * 2. **层级低于 Dialog**（`z-40` < 弹窗的 `z-50`）：点「立即更新」后弹窗盖住卡片，
 *    卡片本身在下载阶段自动让位（只在 `available` 阶段出现）；
 * 3. **说明可折叠**：默认三行（`line-clamp-3`），溢出才给「展开/收起」。
 *    老 Release 没有说明 → 整块不渲染，只留版本号与日期。
 * 4. **不超过视口**（R5-02）：根容器 `max-h-[calc(100vh-2rem)]` + 三段式布局
 *    （header `shrink-0` / notes 区 `min-h-0` 独立滚动 / footer `shrink-0`）。
 *    展开 40+ 行时标题、关闭 ×、三个按钮都还在原位，说明区自己滚，
 *    而不是让卡片长到窗口外——那样被裁掉的头部是滚不回来的。
 */
export function UpdateAvailableCard({
  version,
  notes,
  publishedAt,
  releaseUrl = GITHUB_RELEASE_URL,
  onInstall,
  onDismiss,
}: UpdateAvailableCardProps) {
  const [expanded, setExpanded] = useState(false);
  /** 折叠状态下说明是否真的溢出（DOM 测量）；`null` = 还没量到。 */
  const [overflowing, setOverflowing] = useState<boolean | null>(null);
  const notesRef = useRef<HTMLParagraphElement | null>(null);

  // 渲染前兜底清洗：已发布的清单（0.2.2）里 `notes` 是整份 Release Markdown，
  // 生成侧改不了历史清单，只能在这里把 `##` / `**` / 表格挡住。
  const text = cleanNotes(notes);
  const dateLabel = publishedLabel(publishedAt);

  useEffect(() => {
    setExpanded(false);
    setOverflowing(null);
  }, [text]);

  useEffect(() => {
    // 展开态不测：clamp 已取消，量不出「三行时是否溢出」。
    if (expanded) return;
    const element = notesRef.current;
    if (!element || !text) {
      setOverflowing(null);
      return;
    }
    setOverflowing(element.scrollHeight > element.clientHeight + 1);
  }, [expanded, text]);

  // DOM 量不到时（首帧 / 无法布局的环境）回退到按内容估算，宁可多给一个开关。
  const toggleable = text ? (overflowing ?? notesNeedToggle(text)) : false;

  function openRelease() {
    void openReleaseUrl(releaseUrl).catch(() => {
      // 打不开浏览器不影响用户点「立即更新」：这里静默失败，卡片保持可用。
    });
  }

  return (
    <aside
      role="status"
      aria-live="polite"
      aria-label={`发现新版本 v${version}`}
      className="fixed bottom-4 right-4 z-40 flex max-h-[calc(100vh-2rem)] w-[360px] max-w-[calc(100vw-2rem)] flex-col gap-3 overflow-hidden rounded-xl border border-border bg-card p-4 text-left shadow-lg"
    >
      {/* header：`shrink-0`，说明再长也不许把标题与关闭 × 挤出去。 */}
      <div className="flex shrink-0 items-start gap-2">
        <ArrowUpCircle className="mt-0.5 size-4 shrink-0 text-primary" aria-hidden="true" />
        <div className="min-w-0 flex-1">
          <div className="text-[13px] font-medium leading-5">发现新版本 v{version}</div>
          {dateLabel ? (
            <div className="text-xs leading-4 text-muted-foreground">{dateLabel}</div>
          ) : null}
        </div>
        <Button
          variant="ghost"
          size="icon"
          className="size-6 shrink-0 text-muted-foreground"
          title="本次会话不再提示"
          aria-label="关闭更新提示（本次会话不再提示）"
          onClick={onDismiss}
        >
          <X className="size-3.5" aria-hidden="true" />
        </Button>
      </div>

      {text ? (
        <div className="flex min-h-0 flex-1 flex-col gap-1">
          {/* 说明区：`min-h-0` 让它在 flex 列里可以收缩，展开时自身滚动。
              `overscroll-contain` 滚到尽头不带动下面的页面；`tabIndex` + `aria-label`
              保证键盘能进入并滚动（验收项：Tab 可达）。 */}
          <div
            className={cn(
              "min-h-0 flex-1 overflow-y-auto overscroll-contain pr-1",
              expanded && "max-h-[min(45vh,360px)]",
            )}
            tabIndex={0}
            aria-label="更新说明"
            role="group"
          >
            <p
              ref={notesRef}
              className={cn(
                "whitespace-pre-line break-words text-xs leading-5 text-muted-foreground",
                !expanded && "line-clamp-3",
              )}
            >
              {text}
            </p>
          </div>
          {toggleable ? (
            <button
              type="button"
              className="shrink-0 self-start text-xs text-primary underline-offset-2 hover:underline"
              onClick={() => setExpanded((current) => !current)}
            >
              {expanded ? "收起" : "展开"}
            </button>
          ) : null}
        </div>
      ) : null}

      {/* footer：`shrink-0`，三个按钮永远留在可见范围内。 */}
      <div className="flex shrink-0 flex-wrap items-center gap-2">
        <DemoAction>
          <Button size="sm" onClick={onInstall}>
            <ArrowUpCircle />
            立即更新
          </Button>
        </DemoAction>
        <DemoAction>
          <Button size="sm" variant="outline" onClick={onDismiss}>
            稍后
          </Button>
        </DemoAction>
        <DemoAction>
          <Button
            variant="link"
            size="sm"
            className="h-auto p-0 text-xs text-muted-foreground"
            onClick={openRelease}
          >
            查看完整说明
          </Button>
        </DemoAction>
      </div>
    </aside>
  );
}
