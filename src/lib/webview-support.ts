/**
 * 运行时 WebView 能力检测与降级判定（R5-17，上游 issue #196）。
 *
 * 背景：样式走 Tailwind v4（`@import "tailwindcss"` + `@layer`），要求的 WebKit 能力是
 * 「支持 `@layer`」「支持 `CSS.registerProperty`」「支持 `color-mix()`」。
 * 老 WebKit 不认识 `@layer` 会**丢弃整块**，theme/base/utilities 全没了，只剩未分层
 * 自定义 CSS —— 用户看到的是裸 HTML。因此运行时要判一次，分三档处理：
 *
 * | 能力 | 档位 | 处理 |
 * | --- | --- | --- |
 * | 缺 `@layer`（WebKit < 15.4） | `blocked` | `index.html` 内联全页提示 + `main.tsx` 不挂载 React |
 * | 缺 `color-mix()`（< 16.2）或缺 `CSS.registerProperty`（< 16.4） | `degraded` | 应用顶部一条可关闭提醒条 |
 * | 三项齐备 | `ok` | 静默，行为与改动前完全一致 |
 *
 * 两份实现的分工（改一处必须同时改另一处，`webview-support.test.ts` 会断言
 * `index.html` 里确实存在三项能力探测字符串）：
 * - `index.html` 里的**内联脚本**：未分层、自包含，负责首帧就挡住裸界面（先于 React）；
 * - 本文件的 TS：负责 React 侧守卫与降级提醒条。
 *
 * 本文件只做**能力**判定，不做系统版本判定：同一 Safari 版本号的 WebKitGTK / WKWebView
 * 能力并不一致，版本号只用于文案与「关闭提醒」的存储键（版本变化后重新提示一次）。
 */

/** WebView 能力档位。 */
export type WebviewSupport = "ok" | "degraded" | "blocked";

/** 三项能力探测结果；与 `index.html` 内联脚本里的三个变量一一对应。 */
export interface WebviewCapabilities {
  /** `CSSLayerBlockRule` 存在 → 认识 `@layer`（WebKit 15.4+）。 */
  hasLayer: boolean;
  /** `CSS.registerProperty` 存在（WebKit 16.4+）。 */
  hasProperty: boolean;
  /** `CSS.supports("color", "color-mix(...)")` 为真（WebKit 16.2+）。 */
  hasColorMix: boolean;
}

/**
 * 档位对应的 WebKit / Safari 版本（只用于文案，不参与判定）。
 *
 * `minimum` 是**能正常显示**的最低版本（认识 `@layer`）；`recommended` 是三项能力齐备的
 * 版本。清单明确否决「最低系统设成 13.3」：10.14 及以下最高只有 Safari 14.1，必然
 * < 15.4；而 11.7.5 / 12.6.4 若更新过 Safari 已经是 16.4，设 13.3 会误伤。
 */
export const WEBVIEW_SUPPORT_VERSIONS = {
  minimum: "15.4",
  recommended: "16.4",
} as const;

/** 缺 `@layer` 时注入的阻断页在 DOM 上的标记（便于 DevTools 取证）。 */
export const WEBVIEW_BLOCKED_ATTRIBUTE = "data-webview-blocked";

/** 降级提醒条在 DOM 上的标记（便于 DevTools 取证）。 */
export const WEBVIEW_DEGRADED_ATTRIBUTE = "data-webview-degraded";

/** localStorage 键前缀；后缀是检测到的 Safari / WebKit 版本。 */
const DISMISS_KEY_PREFIX = "wb-shift.webview-notice.dismissed";

/** 版本比较/存储只用到的最小存储接口（便于单测注入假实现）。 */
export interface WebviewStorage {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
}

declare global {
  interface Window {
    /**
     * `index.html` 内联脚本写入的能力档位。缺失 / 非法时由本文件重新探测
     * （两条路径的判定规则必须一致）。
     */
    __WB_SUPPORT__?: string;
    /** `index.html` 内联脚本写入的三项探测原始结果，便于 DevTools 取证。 */
    __WB_CAPABILITIES__?: Partial<WebviewCapabilities>;
  }
}

/**
 * 由三项能力得出档位。
 *
 * 判定顺序固定：先看 `@layer`（缺了就是裸界面，直接阻断），再看两个「能用但会花」的能力。
 */
export function evaluateWebviewSupport(capabilities: WebviewCapabilities): WebviewSupport {
  if (!capabilities.hasLayer) return "blocked";
  if (!capabilities.hasProperty || !capabilities.hasColorMix) return "degraded";
  return "ok";
}

/** `blocked` 时**不得**挂载 React：第一次渲染会清空 `#root`，把内联提示页顶掉。 */
export function shouldBlockRender(support: WebviewSupport): boolean {
  return support === "blocked";
}

/** `degraded` 时渲染可关闭的提醒条；`ok` / `blocked` 都不渲染。 */
export function shouldShowDegradedNotice(support: WebviewSupport): boolean {
  return support === "degraded";
}

/**
 * 比较 `major.minor[.patch]` 形式的版本号：`left < right` 返回负数，相等 0，`left > right` 正数。
 *
 * 缺失的段按 0 处理（`"16"` 等价 `"16.0"`）；非数字段按 0 处理，绝不抛错——
 * UA 是不可信输入，解析失败只会影响文案，不能影响启动。
 */
export function compareWebviewVersions(left: string, right: string): number {
  const parse = (value: string): number[] =>
    value
      .split(".")
      .slice(0, 3)
      .map((part) => {
        const parsed = Number.parseInt(part, 10);
        return Number.isFinite(parsed) && parsed > 0 ? parsed : 0;
      });
  const leftParts = parse(left);
  const rightParts = parse(right);
  const length = Math.max(leftParts.length, rightParts.length);
  for (let index = 0; index < length; index += 1) {
    const diff = (leftParts[index] ?? 0) - (rightParts[index] ?? 0);
    if (diff !== 0) return diff;
  }
  return 0;
}

/** `version` 是否低于 `threshold`；`version` 为空（取不到）时返回 false。 */
export function isWebviewVersionBelow(version: string | null, threshold: string): boolean {
  if (!version) return false;
  return compareWebviewVersions(version, threshold) < 0;
}

/** UA 里解析出来的版本：Safari 的 `Version/x.y` 优先，WKWebView 只有 `AppleWebKit/x.y`。 */
export interface DetectedWebviewVersion {
  /** Safari 版本（`Version/16.3` 里的 `16.3`）；WKWebView / WebView2 上通常为 null。 */
  safari: string | null;
  /** `AppleWebKit/605.1.15` 里的 `605.1.15`；只在没有 Safari 版本时用于文案与存储键。 */
  appleWebKit: string | null;
}

/** 从 UA 取 Safari / WebKit 版本；取不到对应项就是 null。 */
export function parseWebviewVersion(userAgent: string): DetectedWebviewVersion {
  const safari = /Version\/(\d+(?:\.\d+)*)/.exec(userAgent);
  const appleWebKit = /AppleWebKit\/(\d+(?:\.\d+)*)/.exec(userAgent);
  return {
    safari: safari ? safari[1] : null,
    appleWebKit: appleWebKit ? appleWebKit[1] : null,
  };
}

/**
 * 「关闭过提醒」的存储键后缀：Safari 版本优先，否则 WebKit 版本，都没有用 `unknown`。
 *
 * 用版本而不是固定键：升级 Safari 后版本变了，提醒会重新出现一次（清单 §2.2 的默认取舍）。
 */
export function webviewVersionToken(userAgent: string): string {
  const detected = parseWebviewVersion(userAgent);
  return detected.safari ?? detected.appleWebKit ?? "unknown";
}

/** 按 UA 里的 Safari 版本给出**预期**档位（`unknown` = 取不到版本）。 */
export type WebviewSupportExpectation = WebviewSupport | "unknown";

/**
 * 版本 → 预期档位。**只用于文案措辞**，不参与阻断判断（阻断只看能力探测）。
 *
 * 边界：`< 15.4` → `blocked`；`15.4 – 16.3` → `degraded`；`>= 16.4` → `ok`。
 */
export function webviewSupportFromVersion(version: string | null): WebviewSupportExpectation {
  if (!version) return "unknown";
  if (isWebviewVersionBelow(version, WEBVIEW_SUPPORT_VERSIONS.minimum)) return "blocked";
  if (isWebviewVersionBelow(version, WEBVIEW_SUPPORT_VERSIONS.recommended)) return "degraded";
  return "ok";
}

/** 降级提醒条的文案：带检测到的版本、给出升级目标，版本未达推荐值时再点明一句。 */
export function degradedNoticeMessage(userAgent: string): string {
  const detected = parseWebviewVersion(userAgent);
  const label = detected.safari
    ? `Safari ${detected.safari}`
    : detected.appleWebKit
      ? `WebKit ${detected.appleWebKit}`
      : "未知版本";
  const belowRecommended = detected.safari
    ? isWebviewVersionBelow(detected.safari, WEBVIEW_SUPPORT_VERSIONS.recommended)
    : true;
  const suffix = belowRecommended
    ? `你的 ${label} 低于推荐的 ${WEBVIEW_SUPPORT_VERSIONS.recommended}。`
    : `检测到的 ${label} 已够新，若仍有异常请反馈。`;
  return `当前 WebView 版本较旧（${label}），部分颜色与边框可能显示异常，建议升级到 Safari / WebKit ${WEBVIEW_SUPPORT_VERSIONS.recommended} 及以上。${suffix}`;
}

/** 探测窗口对象的三项能力；传 `window` 即可，传假对象便于单测。 */
export function probeWebviewCapabilities(target: unknown): WebviewCapabilities {
  const scope = (target ?? {}) as {
    CSSLayerBlockRule?: unknown;
    CSS?: { registerProperty?: unknown; supports?: unknown };
  };
  const css = scope.CSS;
  const hasLayer = typeof scope.CSSLayerBlockRule !== "undefined";
  const hasProperty = typeof css?.registerProperty === "function";
  const hasColorMix =
    typeof css?.supports === "function" &&
    (css.supports as (property: string, value: string) => boolean)("color", "color-mix(in srgb, red, blue)");
  return { hasLayer, hasProperty, hasColorMix };
}

/** 内联脚本给出的档位是否合法。 */
function isWebviewSupport(value: unknown): value is WebviewSupport {
  return value === "ok" || value === "degraded" || value === "blocked";
}

/**
 * 读取当前窗口的能力档位。
 *
 * 优先用 `index.html` 内联脚本写下的 `window.__WB_SUPPORT__`；缺失或非法时**重新探测**
 * （与服务端渲染 / 单测 / 内联脚本未生效的宿主保持可用）。无 `window` 时按 `ok` 处理：
 * 不能因为拿不到环境就把正常环境误判成阻断。
 */
export function readWebviewSupport(): WebviewSupport {
  if (typeof window === "undefined") return "ok";
  const declared = window.__WB_SUPPORT__;
  if (isWebviewSupport(declared)) return declared;
  return evaluateWebviewSupport(probeWebviewCapabilities(window));
}

/** 「关闭过提醒」的完整存储键（含版本后缀）。 */
export function degradedNoticeStorageKey(versionToken: string): string {
  return `${DISMISS_KEY_PREFIX}:${versionToken}`;
}

/** 取默认存储；隐私模式 / 受限 WebView 下访问 `localStorage` 会抛错，这里吞掉返回 null。 */
function defaultStorage(): WebviewStorage | null {
  try {
    return typeof window === "undefined" ? null : window.localStorage;
  } catch {
    return null;
  }
}

/** 该版本的提醒是否已被关闭。存储不可用一律按「未关闭」处理（宁可多提示一次）。 */
export function isDegradedNoticeDismissed(
  versionToken: string,
  storage: WebviewStorage | null = defaultStorage(),
): boolean {
  if (!storage) return false;
  try {
    return storage.getItem(degradedNoticeStorageKey(versionToken)) === "1";
  } catch {
    return false;
  }
}

/** 记下「该版本的提醒已关闭」。写失败（配额 / 禁用）静默忽略。 */
export function dismissDegradedNotice(
  versionToken: string,
  storage: WebviewStorage | null = defaultStorage(),
): void {
  if (!storage) return;
  try {
    storage.setItem(degradedNoticeStorageKey(versionToken), "1");
  } catch {
    /* 存储不可用：本次会话内由组件 state 记住，刷新后会再提示一次，可接受 */
  }
}
