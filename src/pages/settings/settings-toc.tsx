import { useEffect, useRef, useState } from "react";

import { cn } from "@/lib/utils";
import type { SettingsGrouped } from "./sections";

interface Props {
  /** 已按搜索过滤、且已按当前环境过滤的分组节。 */
  groups: SettingsGrouped[];
  /** 宽屏下的固定宽度栏：由页面用 grid 定义列，这里只负责 sticky 与内容。 */
  className?: string;
}

/** 顶部留白：标题栏是 sticky 的，高亮判定要把这段让出去。 */
const OBSERVER_ROOT_MARGIN = "-72px 0px -55% 0px";

/**
 * 设置页目录（宽屏专用，`xl` 以上显示）。
 *
 * 三件事：
 * 1. **跳转**：点条目滚动到对应节。用 `scrollIntoView` 而不是 `<a href="#id">`，
 *    避免默认跳转把锚点顶到视口最上方（被 sticky 顶栏盖住）；URL 用 `replaceState`
 *    同步，不产生历史记录，也不会触发 HashRouter 的 hashchange（演示站点用的是 HashRouter，
 *    那里的 hash 是路由本身：`#/settings`，改成 `#settings-startup` 会把页面甩回首页）。
 * 2. **高亮跟随**：IntersectionObserver 监听所有节，取「当前可见且最靠上」的那个。
 *    用观察者而不是 scroll 事件：设置页有折叠面板，行高会变，观察者会自己重新计算。
 * 3. **深链**：挂载时读一次 hash（`/settings#settings-startup`），命中已知节就滚过去。
 */
export function SettingsToc({ groups, className }: Props) {
  const [activeId, setActiveId] = useState<string>(groups[0]?.sections[0]?.id ?? "");
  const touchedRef = useRef(false);

  // 目录项的 id 串：用它当 effect 依赖。groups 每次渲染都是新数组，
  // 直接当依赖会让观察者每帧重建（滚动时反复 observe/disconnect）。
  const idsKey = groups.flatMap((group) => group.sections.map((section) => section.id)).join(",");

  useEffect(() => {
    if (typeof window === "undefined") return;
    const ids = idsKey ? idsKey.split(",") : [];
    const targets = ids
      .map((id) => document.getElementById(id))
      .filter((element): element is HTMLElement => element !== null);
    if (targets.length === 0) return;
    if (typeof IntersectionObserver === "undefined") return;

    const observer = new IntersectionObserver(
      (entries) => {
        // 用户点击刚滚过去的这一小段时间里，观察者会连续回调；以点击结果为准。
        if (touchedRef.current) return;
        const visible = entries
          .filter((entry) => entry.isIntersecting)
          .sort((a, b) => a.boundingClientRect.top - b.boundingClientRect.top);
        if (visible.length > 0) setActiveId(visible[0].target.id);
      },
      { rootMargin: OBSERVER_ROOT_MARGIN },
    );
    for (const target of targets) observer.observe(target);
    return () => observer.disconnect();
  }, [idsKey]);

  // 深链只在挂载时读一次：之后用户点目录不再改地址语义，避免与滚动高亮互相打架。
  useEffect(() => {
    if (typeof window === "undefined") return;
    const hash = window.location.hash.replace(/^#/, "");
    if (!hash) return;
    const frame = window.requestAnimationFrame(() => {
      const element = document.getElementById(hash);
      if (!element) return;
      touchedRef.current = true;
      setActiveId(hash);
      element.scrollIntoView({ block: "start" });
    });
    return () => window.cancelAnimationFrame(frame);
  }, []);

  function jump(id: string) {
    const element = document.getElementById(id);
    if (!element) return;
    touchedRef.current = true;
    setActiveId(id);
    element.scrollIntoView({ block: "start", behavior: "smooth" });
    if (typeof window !== "undefined" && !window.location.hash.startsWith("#/")) {
      window.history.replaceState(null, "", `#${id}`);
    }
    // 平滑滚动结束后重新交还给观察者，否则高亮会一直停在点击项上。
    window.setTimeout(() => {
      touchedRef.current = false;
    }, 600);
  }

  // `top-10`（40px）而不是贴顶：macOS 桌面壳在窗口顶部固定了一条 32px 高的
  // `data-tauri-drag-region`（`src/App.tsx` 的 hasUnifiedTitleBar），贴顶的目录
  // 最上面几行会落进那条拖拽带里点不动。
  return (
    <nav aria-label="设置目录" className={cn("sticky top-10 self-start", className)}>
      <div className="space-y-5">
        {groups.map((group) => (
          <div key={group.group.id}>
            <div className="px-2 text-[11px] font-medium uppercase tracking-wide text-muted-foreground/70">
              {group.group.title}
            </div>
            <ul className="mt-1.5 space-y-0.5">
              {group.sections.map((section) => {
                const active = section.id === activeId;
                return (
                  <li key={section.id}>
                    <button
                      type="button"
                      onClick={() => jump(section.id)}
                      aria-current={active ? "true" : undefined}
                      className={cn(
                        "w-full cursor-pointer truncate rounded-md px-2 py-1.5 text-left text-[13px] leading-5 outline-none transition-colors",
                        "focus-visible:ring-2 focus-visible:ring-ring/50",
                        active
                          ? "bg-foreground/[0.06] font-medium text-foreground"
                          : "text-muted-foreground hover:bg-foreground/[0.04] hover:text-foreground",
                      )}
                    >
                      {section.title}
                    </button>
                  </li>
                );
              })}
            </ul>
          </div>
        ))}
      </div>
    </nav>
  );
}
