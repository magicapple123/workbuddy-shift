/**
 * 桌面端 UI 能力：事件订阅、打开外部链接、系统保存对话框、在文件管理器中定位。
 *
 * 页面与组件只从这里调用，不直接 import `@tauri-apps/*`：
 * 这样 WebUI 分支只需要判断一次环境，插件依赖也不会散落到 UI 层。
 * （`scripts/check-structure.mjs` 会拒绝 `src/pages` / `src/components` 的直接引用。）
 */
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { SaveDialogOptions } from "@tauri-apps/plugin-dialog";

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
