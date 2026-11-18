import type {
  AccountMeta,
  AutoRotateConfig,
  CheckinConfig,
  CheckinLog,
  CheckinResult,
  CreditExpiry,
  CreditStatistics,
  TokenStatistics,
  RateLimitConfig,
  RateLimitHookStatus,
  RateLimitsPayload,
  RotateLog,
  RotateStatus,
  TravelConfig,
  TravelStatus,
  WbVariant,
} from "../types";
import { demoModeEnabled } from "../demo-mode";
import { screenshotDemoResponse } from "../screenshot-demo/index";
import { call, httpCall, isWebui } from "./core";

// ---------------------------------------------------------------------------
// 阶段 3：签到 + token 刷新
// ---------------------------------------------------------------------------

export async function getCheckinStatus(accountId: string): Promise<{
  ok: boolean;
  todayCheckedIn?: boolean;
  result?: string;
  reason?: string;
  error?: string;
  raw?: unknown;
  /** 该行所属档位（档位取账号自身）；缺省按国内版处理。 */
  variant?: WbVariant;
}> {
  // 两个宿主都只查询目标账号；Web 端不再为每个账号重复请求整份列表。
  return call("get_checkin_status", { accountId });
}

export function getCreditExpiry(accountId: string): Promise<CreditExpiry> {
  return call("get_credit_expiry", { accountId });
}

export function getCreditStatistics(refresh = false): Promise<CreditStatistics> {
  return call("get_credit_statistics", refresh ? { refresh: true } : undefined);
}

/**
 * 只采集官方用量（渐进式刷新用）。
 *
 * 服务端单飞：并发点击只有第一个真正发请求，其余复用刚采到的结果（`reused=true`）。
 */
export function refreshOfficialUsage(): Promise<{
  officialUsage: CreditStatistics["officialUsage"];
  elapsedMs: number;
  reused: boolean;
}> {
  return call("refresh_official_usage", {});
}

export function getTokenStatistics(days?: number): Promise<TokenStatistics> { return call("get_token_statistics", days ? { days } : undefined); }

/**
 * 模型限额台账：一次返回**全部账号**当前受限的模型与官方恢复时刻。
 *
 * 不传档位：扫描本身就是全局的（两档位各扫一遍）。后端把 hook 信号与日志扫描
 * 合并后返回，日志扫描按 5 分钟节流（`scannedAt` 是最近一次真实扫描时刻）。
 */
export function getRateLimits(): Promise<RateLimitsPayload> {
  return call("get_rate_limits");
}

/** 限额 hook 安装状态（脚本 + 三处客户端配置）。 */
export function getRateLimitHookStatus(): Promise<RateLimitHookStatus> {
  return call("get_rate_limit_hook_status");
}

/** 安装限额 hook（幂等、写前备份；返回安装后的状态）。 */
export function installRateLimitHook(): Promise<RateLimitHookStatus> {
  return call("install_rate_limit_hook");
}

/** 卸载限额 hook（移除注册条目并尽量逐字节还原配置）。 */
export function uninstallRateLimitHook(): Promise<RateLimitHookStatus> {
  return call("uninstall_rate_limit_hook");
}

/** 限额监听开关（关闭后不扫日志、不渲染限额 chip）。 */
export function getRateLimitConfig(): Promise<RateLimitConfig> {
  return call("get_rate_limit_config");
}

export function saveRateLimitConfig(config: RateLimitConfig): Promise<RateLimitConfig> {
  return call("save_rate_limit_config", {
    config: config as unknown as Record<string, unknown>,
  });
}

export function checkin(accountId: string): Promise<CheckinResult> {
  return call("checkin", { accountId });
}

/**
 * 批量签到：不传档位时覆盖全部档位；显式传入时只处理该档位。
 * 关闭自动签到的账号会被跳过，并逐账号返回 skipped 原因（设置页与托盘同样遵守）。
 * `respectWindow=true` 时遵守签到时间段：窗口外整轮返回 skipped /
 * `outside_checkin_window`，不发起任何签到请求（账号页「刷新并签到」专用）；
 * 缺省不下发该字段，保持设置页 / 托盘 / 旧客户端的立即签到语义。
 *
 * 这里**不能**用 `variantArgs`：`checkin_all` 的缺省语义是「全部档位」，国内版若
 * 缺省不传参，账号页在国内版 Tab 触发的批量签到会打到国际版账号。显式下发 `cn`
 * 与改造前等价（改造前账号库里只有国内版账号）。
 */
export function checkinAll(variant?: WbVariant, respectWindow?: boolean): Promise<{
  accounts: { accountId: string; email: string; result: string; error?: string; inactive?: boolean; reason?: string }[];
  status?: string;
  reason?: string;
}> {
  const args: Record<string, unknown> = variant ? { variant } : {};
  if (respectWindow === true) args.respectWindow = true;
  return call("checkin_all", args);
}

export function getAutoCheckinConfig(): Promise<CheckinConfig> {
  return call("get_auto_checkin_config");
}

export function saveAutoCheckinConfig(config: CheckinConfig): Promise<CheckinConfig> {
  return call("save_auto_checkin_config", {
    config: config as unknown as Record<string, unknown>,
  });
}

export function getCheckinLogs(): Promise<{ logs: CheckinLog[] }> {
  return call("get_checkin_logs");
}

export async function getTravelStatus(accountId: string): Promise<TravelStatus> {
  if (demoModeEnabled) {
    return screenshotDemoResponse("get_travel_status", { accountId }) as TravelStatus;
  }
  if (isWebui()) {
    // 与桌面端同形：按 accountId 查询单账号状态（GET query）。
    return httpCall<TravelStatus>("get_travel_status", { accountId });
  }
  return call("get_travel_status", { accountId });
}

export function getAutoTravelConfig(): Promise<TravelConfig> {
  return call("get_auto_travel_config");
}

export function saveAutoTravelConfig(config: TravelConfig): Promise<TravelConfig> {
  return call("save_auto_travel_config", {
    config: config as unknown as Record<string, unknown>,
  });
}

export function getAutoRotateConfig(): Promise<AutoRotateConfig> {
  return call("get_auto_rotate_config");
}

export function saveAutoRotateConfig(config: AutoRotateConfig): Promise<AutoRotateConfig> {
  return call("save_auto_rotate_config", {
    config: config as unknown as Record<string, unknown>,
  });
}

export function getRotateStatus(): Promise<RotateStatus> {
  return call("rotate_status");
}

/**
 * 手动触发一次轮换检查。
 *
 * `notify`（可选）：因存活门控被推迟、但除门控外本来会切换时由 core 组装好的提示内容。
 * 桌面端由宿主（Rust）直接投递系统通知，这里只保留字段以描述完整返回契约；
 * 无头 server 不投递，调用方按需自行处理。
 */
export function runRotate(): Promise<{
  status: string;
  reason?: string;
  error?: string;
  to?: string;
  notify?: { title: string; body: string };
}> {
  return call("run_rotate");
}

export function getRotateLogs(): Promise<{ logs: RotateLog[] }> {
  return call("get_rotate_logs");
}

export function refreshAccountToken(accountId: string): Promise<AccountMeta> {
  return call("refresh_account_token", { accountId });
}

/** 保存文本到指定路径（桌面：先弹保存对话框；只允许 .csv、≤16 MiB）。 */
export function saveTextFile(path: string, content: string): Promise<{
  ok: boolean;
  path: string;
  bytes: number;
  savedAt: number;
}> {
  return call("save_text_file", { path, content });
}
