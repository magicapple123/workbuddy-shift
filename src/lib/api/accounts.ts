import type {
  AccountMeta,
  AccountRecord,
  AppStatus,
  DisplayField,
  ImportPreviewAccount,
  ImportResult,
  OAuthPollResult,
  OAuthStartResult,
  SessionSyncSelection,
  SwitchResult,
  UpdateAccountMark,
  WbVariant,
} from "../types";
import { demoModeEnabled } from "../demo-mode";
import { screenshotDemoResponse } from "../screenshot-demo/index";
import { call, isWebui, variantArgs } from "./core";

// ---------------------------------------------------------------------------
// 状态 / 账号
// ---------------------------------------------------------------------------

/** 运行状态 / 当前账号 / 应用路径；`variant` 缺省为国内版。 */
export function getStatus(variant?: WbVariant): Promise<AppStatus> {
  return call("get_status", variantArgs(variant));
}

/** 返回全部档位的账号，由调用方按 `variant` 过滤。 */
export function getAccounts(): Promise<{ accounts: AccountMeta[] }> {
  return call("get_accounts");
}

/**
 * 开发环境信息：桌面版是否被 `WB_SHIFT_HOME` 重定向到沙箱数据目录。
 *
 * 只读、纯计算（后端不读文件）。`home` 是实际生效的家目录，`realHome` 是未被
 * 重定向的真实家目录，两者一起放进角标的 tooltip，便于确认当前窗口读到的是沙箱副本。
 *
 * webui / 演示模式没有这条命令（浏览器预览里也没有），降级为「非沙箱」而不是抛错：
 * 角标只是提示，不能因此打断页面渲染。
 */
export async function getDevEnvironment(): Promise<{
  sandboxed: boolean;
  home: string;
  realHome: string;
}> {
  if (demoModeEnabled || isWebui()) {
    return { sandboxed: false, home: "", realHome: "" };
  }
  try {
    return await call("dev_environment");
  } catch {
    // 老宿主 / 命令缺失时按非沙箱处理：宁可不显示角标，也不在首屏弹错误。
    return { sandboxed: false, home: "", realHome: "" };
  }
}

export function deleteAccount(accountId: string): Promise<{ ok: boolean }> {
  return call("delete_account", { accountId });
}

/**
 * 更新账号本地展示字段（备注 / 显示选择 / 标记）。
 * patch 只传需要改的项：`note`（字符串或 null 清空）、`displayField`、
 * `mark`（标记对象或 null 解除，见 R5-04）。
 */
export function updateAccountDisplay(
  accountId: string,
  patch: {
    note?: string | null;
    displayField?: DisplayField;
    mark?: UpdateAccountMark | null;
  },
): Promise<{ ok: boolean; account: AccountMeta }> {
  if (demoModeEnabled) {
    return Promise.resolve(
      screenshotDemoResponse("update_account_display", { accountId, patch }) as {
        ok: boolean;
        account: AccountMeta;
      },
    );
  }
  return call("update_account_display", { accountId, patch });
}

/** 发起登录：国内版为扫码授权，国际版为浏览器 Web 登录授权；`variant` 缺省为国内版（档位由后端记忆，轮询无需再传）。 */
export function oauthStart(variant?: WbVariant): Promise<OAuthStartResult> {
  return call("oauth_start", variantArgs(variant));
}

export function oauthStatus(loginId: string): Promise<OAuthPollResult> {
  return call("oauth_status", { loginId });
}

/** 导入本机当前登录态；`variant` 缺省为国内版（对应各自的登录态文件）。 */
export function importLocal(variant?: WbVariant): Promise<{ ok: boolean; account: AccountMeta }> {
  return call("import_local", variantArgs(variant));
}

export function exportAccounts(accountIds: string[]): Promise<{ ok: boolean; accounts: AccountRecord[] }> {
  return call("export_accounts", { accountIds });
}

/** 桌面端：把完整记录写入用户选择的路径（系统保存对话框产物）。 */
export function exportAccountsToPath(
  accountIds: string[],
  path: string,
): Promise<{ ok: boolean; path: string }> {
  return call("export_accounts_to_path", { accountIds, path });
}

export function previewImportAccounts(
  fileText: string,
): Promise<{ accounts: ImportPreviewAccount[]; total: number }> {
  return call("preview_import_accounts", { fileText });
}

export function importAccounts(fileText: string, indexes: number[]): Promise<ImportResult> {
  return call("import_accounts", { fileText, indexes });
}

export function switchAccount(args: {
  accountId: string;
  restart?: boolean;
  shareSessions?: boolean;
  copySessionIds?: string[];
  syncSelections?: SessionSyncSelection[];
}): Promise<SwitchResult> {
  return call("switch_account", args as unknown as Record<string, unknown>);
}

/** 切换进度（webui 轮询用；桌面端走事件，此函数无副作用）。 */
export function switchProgress(): Promise<{ running: boolean; progress: string | null }> {
  return call("switch_progress");
}
