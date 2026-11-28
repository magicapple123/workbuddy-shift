import { useEffect, useRef, useState } from "react";

import { cn } from "@/lib/utils";
import type { SettingsGrouped } from "./sections";

interface Props {
  /** 已按搜索过滤、且已按当前环境过滤的分组节。 */
  groups: SettingsGrouped[];
  /** 宽屏下的固定宽度栏：由页面用 grid 定义列，这里只负责 sticky 与内容。 */
  className?: string;
}

/**
 * 判定线：视口顶部往下多少像素算「这一节的标题已经越过」。
 *
 * 与节标题上的 `scroll-mt-24`（96px，见 `rows.tsx` 的 `SettingsGroup`）对齐并留一点余量：
 * 跳转落地后标题停在 96px，`96 <= 120` 才判定为「就是这一节」。若判定线比 96 还小，
 * 刚滚过去的这一节会被判成「还没到」，active 会掉回上一节。
 */
export const TOC_ACTIVE_OFFSET = 120;

/**
 * 落点目标：节标题距滚动容器可视顶多少像素算「跳到了」。
 *
 * 与节标题上的 `scroll-mt-24`（96px，见 `rows.tsx` 的 `SettingsGroup`）一致——
 * `scrollIntoView({ block: "start" })` 就是把标题停在这里。`TOC_ACTIVE_OFFSET`（120px）
 * 是判定容差线，**不是**落点线：按 120px 校正会把每次正常跳转整体往下推 24px。
 */
export const TOC_LANDING_TOP = 96;

/** 落点校正的容差（px）：偏差在 ±2px 内视为命中，避免平滑收尾的像素抖动来回拉锯。 */
export const LANDING_TOLERANCE = 2;

/** 平滑滚动是否停下的判断：最后一次 `scroll` 之后这么久没有新滚动就算停了（`scrollend` 兜底）。 */
const SCROLL_SETTLE_MS = 250;
/** 总兜底：不管有没有滚动事件，最多等这么久就量落点。 */
const LANDING_TIMEOUT_MS = 1200;
/** 点击后一直没有 `scroll`（本来就停在目标上）时的提前检查。 */
const LANDING_FIRST_CHECK_MS = 400;
/** 校正后观察布局再变的窗口：窗口内尺寸变化又把标题推走，允许再校正一次。 */
const LANDING_RELAYOUT_MS = 300;
/** 落点校正次数上限：一次校正 + 一次布局补偿。 */
const MAX_LANDING_CORRECTIONS = 2;

/** 一节标题在视口里的位置（只需要 `id` 与 `top`；`top` 由 `getBoundingClientRect()` 给出）。 */
export interface TocSectionBox {
  id: string;
  top: number;
}

/**
 * 按**节标题相对视口顶部的位置**选当前节（纯函数，便于单测）。
 *
 * 规则，按优先级：
 * 1. 已经滚到底（`atBottom`）→ 最后一节。最后一节的卡片通常不够高，标题可能永远
 *    越不过判定线，若不做这一条，「滚到底」时高亮会停在倒数第二节；
 * 2. 取**最后一个** `top <= viewportTop + offset` 的节——即使它是张很短的卡，
 *    只要标题已经越线就归它，不会被紧接着的下一节抢走（旧实现按「最大交叉比」选，
 *    短卡在视口里占比小，正中这个坑）；
 * 3. 一个都没越线（还在顶部）→ 第一节。
 */
export function pickActiveSection(
  sections: readonly TocSectionBox[],
  viewportTop: number,
  viewportHeight: number,
  offset: number = TOC_ACTIVE_OFFSET,
  atBottom = false,
): string {
  if (sections.length === 0) return "";
  if (atBottom) return sections[sections.length - 1].id;

  // 判定线不能越过视口下沿（窗口被压得很矮时 offset 会失真）。
  const line = viewportTop + Math.min(offset, viewportHeight);
  let active = sections[0].id;
  for (const section of sections) {
    if (section.top > line) break;
    active = section.id;
  }
  return active;
}

/**
 * 落点校正量（纯函数，便于单测）：目标标题当前在 `targetTop`，期望停在
 * `viewportTop + offset`，返回应追加到 `scrollTop` 的像素数。
 *
 * 正数 = 还差一截，继续往下滚；负数 = 滚过头了，往回退；`±LANDING_TOLERANCE` 内返回 0。
 */
export function landingDelta(targetTop: number, viewportTop: number, offset: number): number {
  const delta = targetTop - (viewportTop + offset);
  return Math.abs(delta) <= LANDING_TOLERANCE ? 0 : delta;
}

interface LandingCorrectionOptions {
  /** 滚动容器（设置页的 `main`）。 */
  scroller: HTMLElement;
  /** 这次跳转的目标节标题。 */
  target: HTMLElement;
  /** 这次跳转钉住的节 id；`pinnedRef.current` 不再是它就说明已被新跳转/用户滚动接管。 */
  id: string;
  pinnedRef: { current: string | null };
  setActiveId: (id: string) => void;
}

/**
 * 平滑滚动结束后的**一次性落点校正**（T05 K1）。
 *
 * 为什么要它：`scrollIntoView({ behavior: "smooth" })` 只在调用瞬间算一次目标偏移；
 * 跳转期间上方卡片还在加载/状态切换，长高 89px 后标题从 96px 漂到 185px，越过判定线
 * 后高亮就落到上一节。钉住只保住「平滑期间别回写 active」，落点本身得等停稳后再量。
 *
 * 时序：
 * - 优先等滚动容器的 `scrollend`；不支持时用「最后一次 scroll 后 250ms」兜底；
 * - 另设 1200ms 总兜底；点击后 400ms 内一次 scroll 都没有（本来就停在目标上）也直接检查；
 * - 量出的差值超过容差 → 在 rAF 里 `scrollTop += delta` **非平滑**校正一次；
 * - 校正后观察 300ms：若布局又变、把标题推离落点，允许再校正一次（总次数 ≤ 2）；
 * - wheel / touchmove / 滚动翻页键 / 新跳转 → 立刻取消并解除钉住，用户的滚动永远赢。
 *
 * 返回取消函数（新跳转、卸载时调用；已收尾后调用是空操作）。
 */
function armLandingCorrection({
  scroller,
  target,
  id,
  pinnedRef,
  setActiveId,
}: LandingCorrectionOptions): () => void {
  if (typeof window === "undefined") return () => {};

  const isDocument = scroller === document.scrollingElement || scroller === document.documentElement;
  const contentBox = target.parentElement?.parentElement ?? null;
  const timers = new Set<number>();
  let observer: ResizeObserver | null = null;
  let raf = 0;
  let idleTimer = 0;
  let firstTimer = 0;
  let watchTimer = 0;
  let capTimer = 0;
  let corrections = 0;
  let finished = false;

  const later = (run: () => void, ms: number) => {
    const handle = window.setTimeout(() => {
      timers.delete(handle);
      run();
    }, ms);
    timers.add(handle);
    return handle;
  };

  const cancelTimer = (handle: number) => {
    if (!handle) return;
    window.clearTimeout(handle);
    timers.delete(handle);
  };

  const stillMine = () => pinnedRef.current === id;

  const measure = () => {
    const viewportTop = isDocument ? 0 : scroller.getBoundingClientRect().top + scroller.clientTop;
    return landingDelta(target.getBoundingClientRect().top, viewportTop, TOC_LANDING_TOP);
  };

  /** 拆掉监听器与定时器。不动钉住状态（可能是被新跳转接管）。 */
  function stop() {
    if (finished) return false;
    finished = true;
    scroller.removeEventListener("scroll", onScroll);
    scroller.removeEventListener("scrollend", onSettled);
    window.removeEventListener("wheel", onUserInput);
    window.removeEventListener("touchmove", onUserInput);
    window.removeEventListener("keydown", onKeyDown);
    observer?.disconnect();
    observer = null;
    if (raf) window.cancelAnimationFrame(raf);
    raf = 0;
    for (const handle of timers) window.clearTimeout(handle);
    timers.clear();
    return true;
  }

  /** 收尾并解除钉住，回到正常跟随。 */
  function finish() {
    if (!stop()) return;
    if (stillMine()) pinnedRef.current = null;
  }

  /** 用户真的动手滚了：待执行的校正作废，钉住也解除。 */
  function onUserInput() {
    finish();
  }

  function onKeyDown(event: Event) {
    if (SCROLL_KEYS.has((event as KeyboardEvent).key)) finish();
  }

  function onSettled() {
    correct();
  }

  function onScroll() {
    cancelTimer(firstTimer);
    firstTimer = 0;
    cancelTimer(idleTimer);
    idleTimer = later(correct, SCROLL_SETTLE_MS);
  }

  /** 落点已经量过并校正：不再需要等「滚动停稳」，也别让校正自己产生的 scroll 事件再触发一次。 */
  function detachSettleWatchers() {
    scroller.removeEventListener("scroll", onScroll);
    scroller.removeEventListener("scrollend", onSettled);
    cancelTimer(firstTimer);
    firstTimer = 0;
    cancelTimer(idleTimer);
    idleTimer = 0;
    cancelTimer(capTimer);
    capTimer = 0;
  }

  function correct() {
    if (finished) return;
    if (!stillMine()) {
      // 已被新跳转接管：拆掉自己，别碰对方钉住的那一节。
      stop();
      return;
    }
    const delta = measure();
    if (delta === 0 || corrections >= MAX_LANDING_CORRECTIONS) {
      finish();
      return;
    }
    // 目标就在页面最顶上（如「外观」）：标题本来就够不到落点线（滚不动负距离），
    // 往下补 100+px 会把页头滚出视野，那不是校正而是新的错误落点。
    if (delta > 0 && scroller.scrollTop <= 1) {
      finish();
      return;
    }
    corrections += 1;
    cancelTimer(watchTimer);
    watchTimer = 0;
    detachSettleWatchers();
    raf = window.requestAnimationFrame(() => {
      raf = 0;
      if (finished) return;
      if (!stillMine()) {
        stop();
        return;
      }
      // 帧内再量一次：settle 到这一帧之间若又长了一截，按最新差值一次到位。
      const apply = measure();
      if (apply !== 0) {
        scroller.scrollTop += apply;
        setActiveId(id);
      }
      watchRelayout();
    });
  }

  /** 校正只做一次；但 300ms 内布局又长高把标题推走时，允许再补一次。 */
  function watchRelayout() {
    if (corrections >= MAX_LANDING_CORRECTIONS || !contentBox || typeof ResizeObserver === "undefined") {
      finish();
      return;
    }
    observer = new ResizeObserver(() => {
      if (finished || !stillMine()) return;
      // 尺寸变了但没把标题推离落点：继续等窗口结束。
      if (measure() === 0) return;
      observer?.disconnect();
      observer = null;
      correct();
    });
    observer.observe(contentBox);
    watchTimer = later(finish, LANDING_RELAYOUT_MS);
  }

  scroller.addEventListener("scroll", onScroll, { passive: true });
  scroller.addEventListener("scrollend", onSettled);
  window.addEventListener("wheel", onUserInput, { passive: true });
  window.addEventListener("touchmove", onUserInput, { passive: true });
  window.addEventListener("keydown", onKeyDown, { passive: true });
  // 点击时已经停在目标位置（没有可滚的）：400ms 无 scroll 事件就直接检查收尾。
  firstTimer = later(correct, LANDING_FIRST_CHECK_MS);
  capTimer = later(correct, LANDING_TIMEOUT_MS);

  return stop;
}

/**
 * 设置页目录（宽屏专用，`xl` 以上显示）。
 *
 * 三件事：
 * 1. **跳转**：点条目滚动到对应节。用 `scrollIntoView` 而不是 `<a href="#id">`，
 *    避免默认跳转把锚点顶到视口最上方（被 sticky 顶栏盖住）；URL 用 `replaceState`
 *    同步，不产生历史记录，也不会触发 HashRouter 的 hashchange（演示站点用的是 HashRouter，
 *    那里的 hash 是路由本身：`#/settings`，改成 `#settings-startup` 会把页面甩回首页）。
 *    跳转后钉住目标节，等平滑滚动停稳再量一次落点、必要时非平滑校正一次
 *    （见 `armLandingCorrection`：上方卡片跳转期间长高会把标题推离落点，T05 K1）。
 * 2. **高亮跟随**：只有一个事实来源——滚动时用 rAF 节流重算一次「标题越线的最后一节」
 *    （`pickActiveSection`）。不用 IntersectionObserver：观察者按「回调批次里可见的条目」
 *    回写 active，平滑滚动结束时最后一批往往只有下一节，于是高亮漂到下一节（T05 缺陷 A1）。
 *    位置判定与「交叉比大小」无关，短卡不会被抢。
 * 3. **深链**：挂载时读一次 hash（`/settings#settings-startup`），命中已知节就滚过去，
 *    并把 active 钉在该节上，直到用户真的用手滚（wheel / touchmove / 滚动翻页键）——
 *    卡片里的数据加载完成后行高会变，标题可能从 96px 漂到 130px 之外，若立刻重算，
 *    高亮会掉到上一节。用户一动手就解除钉住，回到正常跟随。深链的内容是异步加载的，
 *    钉住本身就是正确策略，不做落点校正。
 */
export function SettingsToc({ groups, className }: Props) {
  const [activeId, setActiveId] = useState<string>(groups[0]?.sections[0]?.id ?? "");
  /** 跳转/深链钉住的节 id；非 null 时重算不回写 active（见上方注释 2、3）。 */
  const pinnedRef = useRef<string | null>(null);
  /** 待执行/进行中的落点校正的取消函数（见 `armLandingCorrection`）。 */
  const landingCancelRef = useRef<(() => void) | null>(null);

  // 目录项的 id 串：用它当 effect 依赖。groups 每次渲染都是新数组，
  // 直接当依赖会让监听器每帧重建（滚动时反复 add/remove 事件）。
  const idsKey = groups.flatMap((group) => group.sections.map((section) => section.id)).join(",");

  useEffect(() => {
    if (typeof window === "undefined") return;
    const ids = idsKey ? idsKey.split(",") : [];
    const targets = ids
      .map((id) => document.getElementById(id))
      .filter((element): element is HTMLElement => element !== null);
    if (targets.length === 0) return;

    // 滚动发生在 `<main class="overflow-y-auto">` 里，不是文档本身：
    // 判定要拿滚动容器的视口顶（`rect.top + clientTop`）与 `atBottom`。
    const scroller = findScrollContainer(targets[0]);
    const isDocument = scroller === document.scrollingElement || scroller === document.documentElement;

    const recompute = () => {
      if (pinnedRef.current) return;
      const viewportTop = isDocument ? 0 : scroller.getBoundingClientRect().top + scroller.clientTop;
      const viewportHeight = isDocument ? window.innerHeight : scroller.clientHeight;
      // 内容比容器还短时 `scrollHeight === clientHeight`，那不是「滚到底」而是「没得滚」。
      const atBottom =
        scroller.scrollHeight > scroller.clientHeight + 2 &&
        scroller.scrollTop + scroller.clientHeight >= scroller.scrollHeight - 2;
      const boxes: TocSectionBox[] = targets.map((element) => ({
        id: element.id,
        top: element.getBoundingClientRect().top,
      }));
      const next = pickActiveSection(boxes, viewportTop, viewportHeight, TOC_ACTIVE_OFFSET, atBottom);
      if (next) setActiveId(next);
    };

    let frame = 0;
    const schedule = () => {
      if (frame) return;
      frame = window.requestAnimationFrame(() => {
        frame = 0;
        recompute();
      });
    };

    // 折叠面板展开/收起会改行高但不产生 scroll 事件，用尺寸变化兜底重算。
    const observer =
      typeof ResizeObserver === "undefined" ? null : new ResizeObserver(() => schedule());
    const contentBox = targets[0].parentElement?.parentElement;
    if (observer && contentBox) observer.observe(contentBox);

    scroller.addEventListener("scroll", schedule, { passive: true });
    window.addEventListener("resize", schedule);
    schedule();

    return () => {
      if (frame) window.cancelAnimationFrame(frame);
      observer?.disconnect();
      scroller.removeEventListener("scroll", schedule);
      window.removeEventListener("resize", schedule);
    };
  }, [idsKey]);

  // 深链只在挂载时读一次：之后用户点目录不再改地址语义，避免与滚动高亮互相打架。
  useEffect(() => {
    if (typeof window === "undefined") return;
    const hash = window.location.hash.replace(/^#/, "");
    if (!hash || hash.startsWith("/")) return;
    const frame = window.requestAnimationFrame(() => {
      const element = document.getElementById(hash);
      if (!element) return;
      pinnedRef.current = hash;
      setActiveId(hash);
      element.scrollIntoView({ block: "start" });
    });

    // 用户真的动手滚了才解除钉住：程序滚动（上面的 scrollIntoView）与数据加载后的
    // 行高变化都不该解除，否则深链的 active 会被重算覆盖成上一节。
    const listeners: Array<[string, EventListener]> = [];
    const unpin = () => {
      pinnedRef.current = null;
      for (const [type, listener] of listeners) window.removeEventListener(type, listener);
    };
    const onKeyDown: EventListener = (event) => {
      if (SCROLL_KEYS.has((event as KeyboardEvent).key)) unpin();
    };
    listeners.push(["wheel", unpin], ["touchmove", unpin], ["keydown", onKeyDown]);
    for (const [type, listener] of listeners) {
      window.addEventListener(type, listener, { passive: true });
    }

    return () => {
      window.cancelAnimationFrame(frame);
      for (const [type, listener] of listeners) window.removeEventListener(type, listener);
    };
  }, []);

  function jump(id: string) {
    const element = document.getElementById(id);
    if (!element) return;
    const scroller = findScrollContainer(element);
    // 上一次跳转的落点校正还没走完就再点：只保留最后一次（连点两个目录项时不互相拉扯）。
    landingCancelRef.current?.();
    landingCancelRef.current = null;
    // 平滑滚动期间钉住目标节：上方卡片加载/状态切换改行高时，重算不许把 active 写回上一节。
    pinnedRef.current = id;
    setActiveId(id);
    element.scrollIntoView({ block: "start", behavior: "smooth" });
    if (typeof window !== "undefined" && !window.location.hash.startsWith("#/")) {
      window.history.replaceState(null, "", `#${id}`);
    }
    landingCancelRef.current = armLandingCorrection({
      scroller,
      target: element,
      id,
      pinnedRef,
      setActiveId,
    });
  }

  // 卸载时取消还在等 `scrollend`/定时器的落点校正，避免对已卸载的组件写 state。
  useEffect(
    () => () => {
      landingCancelRef.current?.();
      landingCancelRef.current = null;
    },
    [],
  );

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

/** 触发「解除深链钉住」的按键：滚动类按键，普通输入不算。 */
const SCROLL_KEYS = new Set([
  "ArrowUp",
  "ArrowDown",
  "PageUp",
  "PageDown",
  "Home",
  "End",
  " ",
  "Spacebar",
]);

/**
 * 找最近的可滚动祖先；都没有就退回文档。
 *
 * 设置页的 `main` 是 `overflow-y-auto`，判定要用它的视口顶与 scrollTop/scrollHeight，
 * 用 `window` 的滚动量会永远是 0（文档不滚，见 T05 报告第 1 节）。
 */
function findScrollContainer(element: HTMLElement): HTMLElement {
  let node = element.parentElement;
  while (node && node !== document.documentElement) {
    const overflowY = window.getComputedStyle(node).overflowY;
    if (overflowY === "auto" || overflowY === "scroll" || overflowY === "overlay") return node;
    node = node.parentElement;
  }
  return (document.scrollingElement as HTMLElement | null) ?? document.documentElement;
}
