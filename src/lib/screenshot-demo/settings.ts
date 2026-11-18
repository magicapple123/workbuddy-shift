import type {
  AutoRotateConfig,
  CheckinConfig,
  CheckinLog,
  RateLimitHookStatus,
  RateLimitsPayload,
  RotateLog,
  TravelConfig,
  TravelStatus,
} from "../types";
import { accountVariant, variantSupportsCheckin } from "../variant";
import { accounts, atLocalTime, hydratedAccounts } from "./accounts";
import type { DemoContext } from "./accounts";

/**
 * 设置页演示数据：签到 / 旅行 / 限额 / 轮换配置与日志，以及更新与启动项状态。
 */

export const SETTINGS_DEMO_COMMANDS = [
  "get_checkin_status",
  "get_auto_checkin_config",
  "get_checkin_logs",
  "get_travel_status",
  "get_rate_limits",
  "get_rate_limit_hook_status",
  "get_rate_limit_config",
  "get_auto_travel_config",
  "get_auto_rotate_config",
  "rotate_status",
  "get_rotate_logs",
  "get_github_config",
  "check_update",
  "get_launch_at_login_enabled",
  "switch_progress",
] as const;

function checkinConfig(): CheckinConfig {
  return {
    enabled: true,
    excluded_account_ids: [],
    checkin_start: "",
    checkin_end: "",
    keepalive_days: 7,
    lazy_refresh_hours: 12,
  };
}

function travelConfig(): TravelConfig {
  return { enabled: true };
}

function travelStatus(accountId: string): TravelStatus {
  const index = Math.max(0, accounts.findIndex((account) => account.id === accountId));
  // 演示三种状态：旅行中 / 已结束 / 无 Buddy
  if (index % 3 === 0) return { label: "traveling", rewardCredit: 7, locationName: "咖啡馆", arriveAt: Math.floor(Date.now() / 1000) + 2 * 3600 + 40 * 60 };
  if (index % 3 === 1) return { label: "finished", rewardCredit: 20, locationName: "健身房" };
  return { label: "no-buddy", rewardCredit: null, locationName: null };
}

/**
 * 模型限额演示数据：A 单模型受限（图标无角标）、B 双模型受限（图标带数量角标，
 * 其中一条故意归因失败以展示「未知模型」）、C 无受限（图标不渲染）。
 *
 * 恢复时刻必须相对 `Date.now()` 生成：前端每秒按 `resetAt` 过滤，写死绝对时间会让
 * 演示页在某个时刻之后再也看不到图标。三条条目也刻意覆盖「小时级 / 分钟级」倒计时。
 */
function rateLimits(): RateLimitsPayload {
  const now = Date.now();
  return {
    scannedAt: now,
    windowDays: 2,
    accounts: [
      {
        accountId: accounts[0].id,
        limited: [
          { model: "deepseek-v4.1-flash", resetAt: now + 134 * 60_000, firstSeenAt: now - 26 * 60_000, hitCount: 7 },
        ],
      },
      {
        accountId: accounts[1].id,
        limited: [
          { model: "kimi-k3-1", resetAt: now + 47 * 60_000, firstSeenAt: now - 41 * 60_000, hitCount: 7 },
          { model: null, resetAt: now + 5 * 3_600_000, firstSeenAt: now - 12 * 60_000, hitCount: 2 },
        ],
      },
    ],
  };
}

export function rotateConfig(): AutoRotateConfig {
  return { enabled: true, check_interval_minutes: 15, cooldown_minutes: 120, min_gap_hours: 24, min_urgency_hours: 72, active_guard_minutes: 30, min_remaining_credits: 50 };
}

/** 限额 hook 演示状态：三处配置都显示为已安装。 */
function rateLimitHookStatus(): RateLimitHookStatus {
  const base = "/demo/.workbuddy-shift";
  return {
    scriptPath: `${base}/hook.sh`,
    scriptExists: true,
    eventsPath: `${base}/hook-events.jsonl`,
    installed: true,
    lastEventAt: Date.now() - 4 * 60_000,
    targets: ["codebuddy", "workbuddy", "workbuddy-ai"].map((label) => ({
      label,
      path: `/demo/.${label}/settings.json`,
      exists: true,
      installed: true,
    })),
  };
}

/** 签到日志只覆盖支持签到的档位；A/B/C 的索引与取值不变，企业版 D 追加在国际版之后。 */
function checkinLogs(): CheckinLog[] {
  return hydratedAccounts().flatMap((account, accountIndex) => variantSupportsCheckin(accountVariant(account))
    ? [0, 1, 2].map((daysAgo) => ({ ts: atLocalTime(daysAgo, 8, 6 + accountIndex * 9), accountId: account.id, email: account.nickname ?? account.email ?? account.id, result: accountIndex === 1 && daysAgo === 0 ? "already" : "success" }))
    : []);
}

function rotateLogs(): RotateLog[] {
  return [
    { ts: atLocalTime(0, 9, 30), action: "skipped", reason: "当前账号仍是积分到期最紧迫的可用账号", from: { id: accounts[0].id, name: accounts[0].nickname }, to: null },
    { ts: atLocalTime(1, 16, 20), action: "switched", reason: "目标账号积分将在 5 天内到期", from: { id: accounts[1].id, name: accounts[1].nickname }, to: { id: accounts[0].id, name: accounts[0].nickname } },
  ];
}

export function settingsDemoResponse(
  command: string,
  ctx: DemoContext,
  args?: Record<string, unknown>,
): unknown | undefined {
  const { config, rotateStatus, githubConfig } = ctx;
  switch (command) {
    case "get_checkin_status": return { ok: true, todayCheckedIn: true };
    case "get_auto_checkin_config": return checkinConfig();
    case "get_checkin_logs": return { logs: checkinLogs() };
    case "get_travel_status": return travelStatus(String(args?.accountId ?? ""));
    case "get_rate_limits": return rateLimits();
    case "get_rate_limit_hook_status": return rateLimitHookStatus();
    case "get_rate_limit_config": return { enabled: true };
    case "get_auto_travel_config": return travelConfig();
    case "get_auto_rotate_config": return config;
    case "rotate_status": return rotateStatus;
    case "get_rotate_logs": return { logs: rotateLogs() };
    case "get_github_config": return githubConfig;
    case "check_update": return { ok: true, current: "0.1.24", latest: "0.1.25", latestTag: "v0.1.25", hasUpdate: true, releaseName: "更新提示演示", releaseUrl: "https://github.com/magicapple123/workbuddy-shift/releases/tag/v0.1.25" };
    case "get_launch_at_login_enabled": return true;
    case "switch_progress": return { running: false, progress: null };
    default: return undefined;
  }
}