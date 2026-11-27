/**
 * 桌面端 UI 能力：事件订阅、打开外部链接、系统保存对话框、在文件管理器中定位。
 *
 * 页面与组件只从这里调用，不直接 import `@tauri-apps/*`：
 * 这样 WebUI 分支只需要判断一次环境，插件依赖也不会散落到 UI 层。
 * （`scripts/check-structure.mjs` 会拒绝 `src/pages` / `src/components` 的直接引用。）
 */
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { OpenDialogOptions, SaveDialogOptions } from "@tauri-apps/plugin-dialog";

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

/**
 * 当前宿主是否 Windows。
 *
 * 平台判断统一走 `desktopPlatform()` 的比较（调用方拿完整平台值再决定渲染哪些按钮），
 * 单独的 `isXxx()` 便捷函数没有人调用，已删除，避免留下第二套判断入口。
 */
export function isWindows(): boolean {
  return desktopPlatform() === "windows";
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

/**
 * 系统打开对话框（单选文件）；用户取消时返回 null。
 *
 * 前端拿到的只是路径本身：加密备份的解密与合并都在 Rust 侧完成（口令不外传字节码、
 * 大文件也不用先读进 JS 内存）。多选形态 `open()` 会返回数组，这里只接受单选字符串，
 * 形状不符一律按"没选"处理。
 */
export async function openFileDialog(options: OpenDialogOptions): Promise<string | null> {
  const { open } = await import("@tauri-apps/plugin-dialog");
  const picked = await open({ ...options, multiple: false, directory: false });
  return typeof picked === "string" ? picked : null;
}
