import type { AutomationTasksSummary } from "../types";
import { call } from "./core";

/**
 * 当前登录账号启用中的 WorkBuddy「自动化」定时任务摘要（只读）。
 *
 * 切号后原账号的任务不再执行，而客户端界面不给提示——切号弹窗用这个数量提醒用户。
 * 客户端连不上库（未安装 / 老版本 / 无该账号）时后端返回 `count = 0`，前端不渲染提示块。
 */
export function automationTasksSummary(uid: string): Promise<AutomationTasksSummary> {
  return call("automation_tasks_summary", { uid });
}

/** 切号同步自动化任务的开关状态（默认关闭）。 */
export function automationSyncEnabled(): Promise<{ enabled: boolean }> {
  return call("automation_sync_enabled");
}

/** 写开关；关闭时只剩切号弹窗里的提示，库零写入。 */
export function automationSyncSetEnabled(enabled: boolean): Promise<{ enabled: boolean }> {
  return call("automation_sync_set_enabled", { enabled });
}
