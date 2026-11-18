import type { AppNotification, ErrorLogKind } from "../types";
import { demoModeEnabled } from "../demo-mode";
import { call, isWebui } from "./core";

// ---------------------------------------------------------------------------
// 通知存档（toast 事后可查）
// ---------------------------------------------------------------------------

/** 记录一条应用内提示（由 `lib/notify.ts` 统一调用；失败不影响提示本身）。 */
export function recordNotification(
  level: AppNotification["level"],
  title: string,
  description?: string,
): Promise<{ recorded: boolean }> {
  if (demoModeEnabled) return Promise.resolve({ recorded: false });
  return call("record_notification", { level, title, description });
}

/** 读取最近的通知（新的在前，最多 100 条）。 */
export function listNotifications(): Promise<{ items: AppNotification[] }> {
  if (demoModeEnabled) return Promise.resolve({ items: [] });
  return call("list_notifications");
}

/** 清空通知存档。 */
export function clearNotifications(): Promise<{ cleared: boolean }> {
  if (demoModeEnabled) return Promise.resolve({ cleared: false });
  return call("clear_notifications");
}

// ---------------------------------------------------------------------------
// 错误日志（前端崩溃 / 未捕获错误落盘，见 lib/error-report.ts）
// ---------------------------------------------------------------------------

/**
 * 上报一条错误到本地错误日志（桌面端落盘 `~/.workbuddy-shift/error.log`）。
 *
 * webui / 演示模式没有落盘通道：静默忽略（调用方的本地提示不受影响）。
 */
export function logError(kind: ErrorLogKind, message: string, detail?: string): Promise<void> {
  if (demoModeEnabled || isWebui()) return Promise.resolve();
  return call<unknown>("log_error", { kind, message, detail: detail ?? null }).then(
    () => undefined,
  );
}

/** 错误日志文件路径（设置页展示）。 */
export function getErrorLogPath(): Promise<string> {
  // 演示模式给一条与其它演示路径同风格的值，保证演示页 / 截图里界面完整。
  if (demoModeEnabled) return Promise.resolve("/demo/.workbuddy-shift/error.log");
  // webui 没有落盘通道（不写服务端日志），设置页不展示路径。
  if (isWebui()) return Promise.resolve("");
  return call<string>("get_error_log_path");
}

/** 在文件管理器中定位错误日志（桌面端；日志尚未生成时由后端打开所在目录）。 */
export function revealErrorLog(): Promise<void> {
  if (demoModeEnabled || isWebui()) return Promise.resolve();
  return call<unknown>("reveal_error_log").then(() => undefined);
}
