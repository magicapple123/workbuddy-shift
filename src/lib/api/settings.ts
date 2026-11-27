import type { DataDirInfo, SettingsBundle, SettingsImportReport } from "../types";
import { DEMO_UNAVAILABLE_MESSAGE, demoModeEnabled } from "../demo-mode";
import { call, isDesktop } from "./core";

// ---------------------------------------------------------------------------
// 开机自启（仅桌面端；webui 不提供同名接口，卡片也不在 webui 渲染）
// ---------------------------------------------------------------------------

/** 查询系统当前的开机自启注册状态（桌面端）。 */
export function getLaunchAtLoginEnabled(): Promise<boolean> {
  if (demoModeEnabled) return call("get_launch_at_login_enabled");
  if (!isDesktop()) return Promise.resolve(false);
  return call("get_launch_at_login_enabled");
}

/** 注册 / 移除系统开机自启，返回回读后的权威状态（桌面端）。 */
export function setLaunchAtLoginEnabled(enabled: boolean): Promise<boolean> {
  if (demoModeEnabled) return Promise.reject(new Error(DEMO_UNAVAILABLE_MESSAGE));
  if (!isDesktop()) return Promise.resolve(false);
  return call("set_launch_at_login_enabled", { enabled });
}

// ---------------------------------------------------------------------------
// 工具数据目录 / 设置包导出导入（桌面端与 WebUI 都提供）
// ---------------------------------------------------------------------------

/**
 * 工具数据目录（`~/.workbuddy-shift`、开发沙箱或 `WB_SHIFT_HOME` 指向的目录）。
 *
 * 只读：回包只有路径字符串，不含凭据，也不做任何写操作。设置页「关于」用它展示
 * 数据位置，再用 `storageReveal(path, "open")` 在系统文件管理器里打开。
 */
export function getDataDir(): Promise<DataDirInfo> {
  return call("get_data_dir");
}

/**
 * 导出设置包（checkin / travel / rotate / rateLimit 四份）。
 *
 * 不含账号库、不含更新（GitHub）配置：前者带 token，后者可能带代理口令。
 * 回包是可直接落盘的 JSON 对象，落盘方式由调用方决定（桌面弹保存框 / 浏览器下载）。
 */
export function exportSettingsBundle(): Promise<SettingsBundle> {
  return call("export_settings_bundle");
}

/**
 * 导入设置包：白名单校验（未知键、半截对象、类型不符一律拒绝）+ 写前备份 + 失败回滚。
 *
 * 参数类型刻意是「JSON 对象」而不是 `SettingsBundle`：用户可能手改文件、也可能导入
 * 只含一两份配置的包，格式对不对由 core 判定（它是唯一权威，桌面端走同一个函数）。
 * 在前端先收敛成 `SettingsBundle` 会把「未知键」这类错误在客户端就抹平，
 * 让后端的整体拒绝失去意义。
 */
export function importSettingsBundle(
  bundle: Record<string, unknown>,
): Promise<SettingsImportReport> {
  return call("import_settings_bundle", { bundle });
}
