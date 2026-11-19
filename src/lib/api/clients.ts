import type {
  CodeBuddyCliInstallResult,
  CodeBuddyCliStatus,
  CodeBuddyCliSwitchResult,
  CodeBuddyCnIdeStatus,
  CodeBuddyCnIdeSwitchResult,
  SessionLinksPreview,
  SessionSyncSelection,
  VscodeExtStatus,
  VscodeExtSwitchResult,
  JetbrainsStatus,
  JetbrainsSwitchResult,
  VscodeSessionList,
  VscodeSessionRef,
  WbVariant,
} from "../types";
import { DEMO_UNAVAILABLE_MESSAGE, demoModeEnabled } from "../demo-mode";
import { screenshotDemoResponse } from "../screenshot-demo/index";
import { call, isWebui, requireCompanionDesktop, variantArgs } from "./core";

export function getCodebuddyCliStatus(): Promise<CodeBuddyCliStatus> {
  return call("get_codebuddy_cli_status");
}

export function installCodebuddyCliHelper(): Promise<CodeBuddyCliInstallResult> {
  return call("install_codebuddy_cli_helper");
}

/**
 * 切换 CodeBuddy CLI 默认账号。
 *
 * @param closeRunningCli 已废弃：后端一律先关闭正在运行的 CLI 再写状态，该入参被忽略。
 *   仅为兼容既有调用方保留（HTTP 路径仍会原样发送）。
 */
export function switchCodebuddyCliAccount(
  accountId: string,
  closeRunningCli = false,
): Promise<CodeBuddyCliSwitchResult> {
  if (demoModeEnabled) {
    return new Promise((resolve, reject) => {
      window.setTimeout(() => {
        try {
          resolve(
            screenshotDemoResponse("switch_codebuddy_cli_account", {
              accountId,
              closeRunningCli,
            }) as CodeBuddyCliSwitchResult,
          );
        } catch (error) {
          reject(error);
        }
      }, 1200);
    });
  }
  return call("switch_codebuddy_cli_account", { accountId, closeRunningCli });
}

export function getCodebuddyCnIdeStatus(): Promise<CodeBuddyCnIdeStatus> {
  return call("get_codebuddy_cn_ide_status");
}

/**
 * 切换 CodeBuddy IDE 账号（可同时复制 / 同步会话）。
 *
 * `restart` 默认 true：IDE 运行时由后端先关闭、写入后再重新打开。
 * `copySessions` 非空时切换前把勾选会话复制到目标账号（默认沿用会话 id，冲突才重随机）；
 * `syncSelections` 与 VS Code 侧同形；两者都不传时行为与纯切换逐字一致。
 */
export function switchCodebuddyCnIdeAccount(
  accountId: string,
  restart = true,
  copySessions?: VscodeSessionRef[],
  syncSelections?: SessionSyncSelection[],
): Promise<CodeBuddyCnIdeSwitchResult> {
  return call("switch_codebuddy_cn_ide_account", { accountId, restart, copySessions, syncSelections });
}

/** 列出当前 CodeBuddy IDE 账号可复制的会话（未登录/未安装时返回空列表）。 */
export function listCodebuddyIdeSessions(): Promise<VscodeSessionList> {
  return call("list_codebuddy_ide_sessions");
}

/**
 * 预览「当前 CodeBuddy IDE 账号 → 目标账号」可同步的关联会话。
 *
 * 只读：`defaultChecked` 与 `availableModes` 是勾选权限的唯一来源，前端不得自行扩大。
 */
export function codebuddyIdeSessionLinksPreview(
  targetAccountId: string,
): Promise<SessionLinksPreview> {
  return call("codebuddy_ide_session_links_preview", { targetAccountId });
}

export function detectCodebuddyCnIdeAccount(): Promise<{
  ok: boolean;
  found: boolean;
  matched?: boolean;
  accountId?: string;
  message?: string;
}> {
  return call("detect_codebuddy_cn_ide_account");
}

export function getVscodeExtStatus(): Promise<VscodeExtStatus> {
  return call("get_vscode_ext_status");
}

/** 列出当前 VS Code 扩展账号可复制的会话（未安装/未登录时返回空列表）。 */
export function listVscodeSessions(): Promise<VscodeSessionList> {
  return call("list_vscode_sessions");
}

/**
 * 预览「当前 VS Code CodeBuddy 插件账号 → 目标账号」可同步的关联会话。
 *
 * 只读：`defaultChecked` 与 `availableModes` 是勾选权限的唯一来源，前端不得自行扩大。
 */
export function vscodeSessionLinksPreview(targetAccountId: string): Promise<SessionLinksPreview> {
  return call("vscode_session_links_preview", { targetAccountId });
}

/**
 * 切换 VS Code CodeBuddy 扩展账号。
 *
 * `restart` 默认 true：VS Code 运行时由后端先优雅退出、写入后再重新打开；
 * 传 false 退回「请先完全退出 VS Code」的手动模式（不在编辑器中自动操作）。
 * `syncSelections` 与 WorkBuddy 侧同形；只传它（不传 `copySessions`）也能执行同步。
 */
export function switchVscodeExtAccount(
  accountId: string,
  restart = true,
  copySessions?: VscodeSessionRef[],
  syncSelections?: SessionSyncSelection[],
): Promise<VscodeExtSwitchResult> {
  return call("switch_vscode_ext_account", { accountId, restart, copySessions, syncSelections });
}

export function detectVscodeExtAccount(): Promise<{
  ok: boolean;
  found: boolean;
  matched?: boolean;
  accountId?: string;
  message?: string;
}> {
  return call("detect_vscode_ext_account");
}

export function getJetbrainsStatus(): Promise<JetbrainsStatus> {
  return call("get_jetbrains_status");
}

/**
 * 切换 JetBrains IDE（IDEA / PyCharm）CodeBuddy 插件账号。
 *
 * `restart` 默认 true：IDE 运行时由后端先优雅退出、写入后再重新打开；
 * 传 false 退回「请先完全退出 IDE」的手动模式（不在 IDE 中自动操作）。
 * `configDirs` 可选：目标配置目录名列表（如 ["PyCharm2026.2"]），缺省 / 空
 * = 全部装了插件的 IDE；非空时只写所选目录、只关闭/重开这些目录的运行实例。
 */
export function switchJetbrainsAccount(
  accountId: string,
  restart = true,
  configDirs?: string[],
): Promise<JetbrainsSwitchResult> {
  return call("switch_jetbrains_account", { accountId, restart, configDirs });
}

export function detectJetbrainsAccount(): Promise<{
  ok: boolean;
  found: boolean;
  matched?: boolean;
  accountId?: string;
  message?: string;
}> {
  return call("detect_jetbrains_account");
}

export function getCodebuddyIdeStatus(): Promise<CodeBuddyCnIdeStatus> {
  return call("get_codebuddy_ide_status");
}

/**
 * 切换 CodeBuddy IDE（国际版）账号（可同时复制 / 同步会话）。
 *
 * `restart` 默认 true：IDE 运行时由后端先关闭、写入后再重新打开。
 * `copySessions` 非空时切换前把勾选会话复制到目标账号（默认沿用会话 id，冲突才重随机）；
 * `syncSelections` 与国内版同形；两者都不传时行为与纯切换逐字一致。
 */
export function switchCodebuddyIdeAccount(
  accountId: string,
  restart = true,
  copySessions?: VscodeSessionRef[],
  syncSelections?: SessionSyncSelection[],
): Promise<CodeBuddyCnIdeSwitchResult> {
  return call("switch_codebuddy_ide_account", { accountId, restart, copySessions, syncSelections });
}

/** 列出当前国际版 CodeBuddy IDE 账号可复制的会话（未登录/未安装时返回空列表）。 */
export function listCodebuddyIntlIdeSessions(): Promise<VscodeSessionList> {
  return call("list_codebuddy_intl_ide_sessions");
}

/**
 * 预览「当前国际版 CodeBuddy IDE 账号 → 目标账号」可同步的关联会话。
 *
 * 只读：`defaultChecked` 与 `availableModes` 是勾选权限的唯一来源，前端不得自行扩大。
 */
export function codebuddyIntlIdeSessionLinksPreview(
  targetAccountId: string,
): Promise<SessionLinksPreview> {
  return call("codebuddy_intl_ide_session_links_preview", { targetAccountId });
}

export function detectCodebuddyIdeAccount(): Promise<{
  ok: boolean;
  found: boolean;
  matched?: boolean;
  accountId?: string;
  message?: string;
}> {
  return call("detect_codebuddy_ide_account");
}

/** 预检：当前是否需要关闭 VS Code（运行中 且 目标账号含当前登录账号）；供前端决定是否先弹确认框。 */
export function vscodeRestartPrecheck(targetAccountIds: string[]): Promise<{ required: boolean; running: boolean }> {
  return call("vscode_restart_precheck", { targetAccountIds });
}

/**
 * 预检：当前是否需要关闭 JetBrains IDE（IDEA / PyCharm）。
 *
 * 判定**与目标账号无关**：只要还有装了插件且在运行的实例就得先关闭（多个产品共用同一份
 * 插件数据目录，只关一个仍会被另一个覆盖）。`ides` 是确认框里要列出的实例名。
 */
export function jetbrainsRestartPrecheck(
  targetAccountIds: string[],
): Promise<{ required: boolean; running: boolean; ides?: string[] }> {
  return call("jetbrains_restart_precheck", { targetAccountIds });
}

/**
 * 打开系统设置授权面板（**仅 macOS**：「完全磁盘访问 / App 管理」）。
 *
 * Windows / Linux 没有对应开关，后端返回明确错误；WebUI 没有系统面板，
 * 这里同样 reject 而不是静默 resolve —— 静默成功正是第三轮第 11 条「点了没反应」
 * 的一半原因。调用方必须 try/catch 并给出可见提示。
 */
export function openPermissionSettings(
  target?: "app_management" | "all_files",
): Promise<void> {
  if (demoModeEnabled) return Promise.reject(new Error(DEMO_UNAVAILABLE_MESSAGE));
  if (isWebui()) {
    return Promise.reject(
      new Error("WebUI 模式没有「完全磁盘访问 / App 管理」系统面板：写入权限由服务进程决定"),
    );
  }
  return call("open_permission_settings", { target: target ?? "app_management" });
}

/** 权限自检：桌面端写探针（按档位写在对应登录态文件旁）；webui 模式由服务进程权限决定。 */
export function checkAuthPermission(variant?: WbVariant): Promise<{
  ok: boolean;
  message?: string;
  error?: string;
  dir?: string;
  hint?: string;
}> {
  if (demoModeEnabled) return Promise.reject(new Error(DEMO_UNAVAILABLE_MESSAGE));
  if (isWebui()) {
    return Promise.resolve({
      ok: true,
      message: "webui 模式由服务进程（终端启动）的权限决定，无需额外授权",
      hint: "",
    });
  }
  return call("check_auth_permission", variantArgs(variant));
}

/**
 * 在文件管理器中选中并定位本 App 的可执行文件。
 *
 * macOS 的「完全磁盘访问」要靠拖拽 .app 授权，所以那里用它；Windows / Linux
 * 没有这个授权动作，诊断目标是**认证文件**能否被写，因此那些平台改用
 * `revealPathInFileManager(authFile)` 定位认证文件。后端三个平台都已实现，
 * 失败会 reject（不再 `let _` 吞掉）。
 */
export function revealAppInFinder(): Promise<void> {
  if (demoModeEnabled) return Promise.reject(new Error(DEMO_UNAVAILABLE_MESSAGE));
  if (isWebui()) {
    return Promise.reject(new Error("WebUI 模式无法在系统文件管理器中定位文件"));
  }
  return call("reveal_app_in_finder");
}

/** 后端持久化的悬浮栏启用状态；默认值由后端决定。 */
export function getCompanionEnabled(): Promise<boolean> {
  try {
    requireCompanionDesktop();
    return call("get_companion_enabled");
  } catch (error) {
    return Promise.reject(error);
  }
}

/** 返回后端确认的最终状态，不在前端单独持久化。 */
export function setCompanionEnabled(enabled: boolean): Promise<boolean> {
  try {
    requireCompanionDesktop();
    return call("set_companion_enabled", { enabled });
  } catch (error) {
    return Promise.reject(error);
  }
}

export function openCompanionSettings(): Promise<void> {
  try {
    requireCompanionDesktop();
    return call("open_companion_settings");
  } catch (error) {
    return Promise.reject(error);
  }
}
