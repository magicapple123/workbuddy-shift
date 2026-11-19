/**
 * 桌面端 UI 能力：事件订阅、打开外部链接、系统保存对话框、在文件管理器中定位。
 *
 * 页面与组件只从这里调用，不直接 import `@tauri-apps/*`：
 * 这样 WebUI 分支只需要判断一次环境，插件依赖也不会散落到 UI 层。
 * （`scripts/check-structure.mjs` 会拒绝 `src/pages` / `src/components` 的直接引用。）
 */
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { SaveDialogOptions } from "@tauri-apps/plugin-dialog";

/** 宿主平台。`unknown` 只出现在无法读取 UA 的环境（SSR / 测试）。 */
export type DesktopPlatform = "macos" | "windows" | "linux" | "unknown";

/**
 * UA 判定宿主平台。
 *
 * 与 `core.ts` 的 `isMobilePlatform`、`App.tsx` 的窗口判断沿用同一套浏览器约定：
 * macOS 的 WKWebView 带 `Macintosh`，Windows 的 WebView2 带 `Windows NT`，
 * Linux 的 WebKitGTK 带 `X11; Linux`。WebUI 在浏览器里同样可判定，因此这个函数
 * 不区分桌面 / WebUI —— 调用方再用 `isDesktop()` 决定要不要渲染桌面专属按钮。
 */
export function desktopPlatform(): DesktopPlatform {
  if (typeof navigator === "undefined") return "unknown";
  const ua = navigator.userAgent;
  if (/Windows/i.test(ua)) return "windows";
  if (/Macintosh|Mac OS X/i.test(ua)) return "macos";
  if (/Linux|X11/i.test(ua)) return "linux";
  return "unknown";
}

/** 当前宿主是否 macOS（三个权限按钮只在 macOS 存在）。 */
export function isMacos(): boolean {
  return desktopPlatform() === "macos";
}

/** 当前宿主是否 Windows。 */
export function isWindows(): boolean {
  return desktopPlatform() === "windows";
}

/** 当前宿主是否 Linux。 */
export function isLinux(): boolean {
  return desktopPlatform() === "linux";
}

/** 订阅后端事件；回调只接收 payload，返回值用于取消订阅。 */
export function listenDesktopEvent<T>(
  event: string,
  handler: (payload: T) => void,
): Promise<UnlistenFn> {
  return listen<T>(event, (received) => handler(received.payload));
}

/** 用系统默认浏览器打开链接。 */
export async function openExternalUrl(url: string): Promise<void> {
  const { openUrl } = await import("@tauri-apps/plugin-opener");
  await openUrl(url);
}

/** 在系统文件管理器中选中并定位文件（Windows 资源管理器 / macOS Finder）。 */
export async function revealPathInFileManager(path: string): Promise<void> {
  const { revealItemInDir } = await import("@tauri-apps/plugin-opener");
  await revealItemInDir(path);
}

/** 系统保存对话框；用户取消时返回 null。 */
export async function saveFileDialog(options: SaveDialogOptions): Promise<string | null> {
  const { save } = await import("@tauri-apps/plugin-dialog");
  return save(options);
}
