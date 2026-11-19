import type { GithubConfig, UpdateInfo, UpdateSnapshot } from "../types";
import { DEMO_UNAVAILABLE_MESSAGE, demoModeEnabled } from "../demo-mode";
import { screenshotDemoResponse } from "../screenshot-demo/index";
import { call, isWebui } from "./core";

// ---------------------------------------------------------------------------
// 阶段 4：自动更新
// ---------------------------------------------------------------------------

export function getGithubConfig(): Promise<GithubConfig> {
  return call("get_github_config");
}

export function saveGithubConfig(config: GithubConfig): Promise<GithubConfig> {
  return call("save_github_config", {
    config: config as unknown as Record<string, unknown>,
  });
}

export function checkUpdate(proxy?: string, force?: boolean): Promise<UpdateInfo> {
  return call("check_update", { proxy: proxy?.trim() || null, force: force ?? false });
}

// ---------------------------------------------------------------------------
// 统一更新服务（桌面端；`update-state` 事件是阶段与进度的唯一来源）
// ---------------------------------------------------------------------------

/** 浏览器 / 演示模式没有更新服务：与弹窗既有文案逐字一致。 */
const UPDATE_UNSUPPORTED_MESSAGE = "浏览器 webui 模式不能直接安装桌面更新包";

/**
 * 更新状态快照（前端首屏初始化；之后由 `update-state` 事件推送）。
 *
 * webui 没有更新服务、演示模式禁止真实下载，两者都回落到静态快照：
 * 演示模式给「有新版」态，保证演示页 / 截图里的升级入口与外链完整。
 */
export function updateState(): Promise<UpdateSnapshot> {
  if (demoModeEnabled) {
    // 复用只读演示数据的版本号，避免版本号在两处硬编码。
    const demo = screenshotDemoResponse("check_update") as UpdateInfo;
    return Promise.resolve({
      phase: "available",
      latest: demo.latest ?? null,
      percent: null,
      message: null,
      checkedAt: null,
      busyKind: null,
      busySince: null,
      manualInstallRequired: false,
    });
  }
  if (isWebui()) {
    return Promise.resolve({
      phase: "idle",
      latest: null,
      percent: null,
      message: null,
      checkedAt: null,
      busyKind: null,
      busySince: null,
      manualInstallRequired: false,
    });
  }
  return call("update_state");
}

/** 启动更新包下载（异步，立即返回；进度走 `update-state` 事件与托盘）。
 *
 * `proxy` 传设置页输入框当前值：下载与检查共用同一代理源，避免「只填未保存」时
 * 检查通过、下载却读不到代理（第三轮第 1 条）。
 */
export function updateDownload(proxy?: string): Promise<void> {
  if (demoModeEnabled) return Promise.reject(new Error(DEMO_UNAVAILABLE_MESSAGE));
  if (isWebui()) return Promise.reject(new Error(UPDATE_UNSUPPORTED_MESSAGE));
  return call<unknown>("update_download", { proxy: proxy?.trim() || null }).then(() => undefined);
}

/** 安装已下载的更新包并重启（用户点「重启以完成升级」时调用）。 */
export function updateRestart(): Promise<void> {
  if (demoModeEnabled) return Promise.reject(new Error(DEMO_UNAVAILABLE_MESSAGE));
  if (isWebui()) return Promise.reject(new Error(UPDATE_UNSUPPORTED_MESSAGE));
  return call<unknown>("update_restart").then(() => undefined);
}
