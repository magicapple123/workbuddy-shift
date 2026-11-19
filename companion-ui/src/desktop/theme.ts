/**
 * 悬浮窗主题引导与同步。
 *
 * 主应用（`src/lib/theme.ts`）把主题偏好写在 `localStorage` 的
 * `workbuddy-shift.theme`，三态：`light` / `dark` / `system`，并把结果落到
 * `document.documentElement` 的 `dark` class。悬浮窗与主窗口**同源**
 * （`WebviewUrl::App`），因此能读同一份键、写同一个 class，不需要新增 Tauri
 * 命令，也不需要 IPC。
 *
 * 这个模块必须能在**没有 React** 的环境里跑完：settings/rail 两个 HTML 在
 * head 里把它当第一个模块加载，先上色再渲染，避免先闪一帧浅色骨架。
 * 因此这里不 import 任何 React / Tauri API。
 *
 * 为什么不用 `@media (prefers-color-scheme)`：颜色只允许在 tokens.css 定义一次，
 * 一个 class 决定全部 token；媒体查询只用来判断 `system` 偏好解析成什么，
 * 不做第二套覆盖层。
 */

/** 与主应用一致的存储键（运行时标识，不能改名）。 */
export const THEME_STORAGE_KEY = 'workbuddy-shift.theme';
/** 主应用同页广播的事件名；悬浮窗是独立窗口，只作为附带的同步来源。 */
export const THEME_EVENT_NAME = 'workbuddy-shift:theme-changed';

/** 三态偏好；缺省与损坏数据回退 `system`，与主应用一致。 */
export type ThemePreference = 'system' | 'light' | 'dark';

const DARK_QUERY = '(prefers-color-scheme: dark)';
const DARK_CLASS = 'dark';

function isThemePreference(value: string | null): value is ThemePreference {
  return value === 'system' || value === 'light' || value === 'dark';
}

/** 读取偏好：存储不可用、缺省或值损坏时回退 `system`，不抛错。 */
export function readThemePreference(storage: Storage | null = safeStorage()): ThemePreference {
  if (!storage) return 'system';
  try {
    const raw = storage.getItem(THEME_STORAGE_KEY);
    return isThemePreference(raw) ? raw : 'system';
  } catch {
    return 'system';
  }
}

function safeStorage(): Storage | null {
  if (typeof window === 'undefined') return null;
  try {
    return window.localStorage;
  } catch {
    /* 受限 WebView / 隐私模式下访问 localStorage 会抛错 */
    return null;
  }
}

/** 系统当前是否深色；没有 matchMedia 的环境按浅色处理。 */
export function systemPrefersDark(): boolean {
  if (typeof window === 'undefined' || typeof window.matchMedia !== 'function') return false;
  try {
    return window.matchMedia(DARK_QUERY).matches;
  } catch {
    return false;
  }
}

/** 偏好 → 是否深色。`system` 交给系统查询解析，结果仍落到同一个 class 上。 */
export function resolveDark(preference: ThemePreference): boolean {
  if (preference === 'dark') return true;
  if (preference === 'light') return false;
  return systemPrefersDark();
}

/**
 * 把当前偏好写到 `<html>` 上。返回是否深色。
 *
 * 只切 class，不写内联样式：颜色全部来自 tokens.css 的 token，切换不产生
 * 第二套样式表。
 */
export function applyTheme(preference: ThemePreference = readThemePreference()): boolean {
  if (typeof document === 'undefined') return false;
  const dark = resolveDark(preference);
  document.documentElement.classList.toggle(DARK_CLASS, dark);
  return dark;
}

/**
 * 订阅主题变化：
 * - 同源的另一个窗口（主应用设置页）改偏好会触发 `storage` 事件
 *   （部分 WebView 不跨窗口派发，因此再补一个窗口获得焦点时的重读）；
 * - `system` 偏好下系统外观变化由 matchMedia 通知。
 *
 * 返回取消订阅函数。
 */
export function watchTheme(onChange: (dark: boolean) => void = () => {}): () => void {
  if (typeof window === 'undefined') return () => {};
  const sync = () => onChange(applyTheme());
  window.addEventListener('storage', sync);
  window.addEventListener(THEME_EVENT_NAME, sync);
  window.addEventListener('focus', sync);
  document.addEventListener('visibilitychange', sync);
  let media: MediaQueryList | null = null;
  try {
    media = window.matchMedia(DARK_QUERY);
    media.addEventListener('change', sync);
  } catch {
    media = null;
  }
  return () => {
    window.removeEventListener('storage', sync);
    window.removeEventListener(THEME_EVENT_NAME, sync);
    window.removeEventListener('focus', sync);
    document.removeEventListener('visibilitychange', sync);
    media?.removeEventListener('change', sync);
  };
}

// 被 HTML 当第一个模块加载时立刻上色：本模块只做这一件事，副作用是幂等的，
// 入口脚本再调用一次也不会产生额外开销。
applyTheme();
