

import { WbVariant } from "./account";

export interface CheckinConfig {
  enabled: boolean;
  /** 关闭自动签到的账号 id；状态展示和刷新附带签到也跳过，主动手动签到不受影响。 */
  excluded_account_ids?: string[];
  /** 签到时间段（"HH:MM"，本地时区）；空串 = 不限制。两端都合法且 start < end 才生效。 */
  checkin_start: string;
  checkin_end: string;
  /** Legacy persisted fields; accepted by the backend but ignored by scheduling. */
  start_hour?: number;
  end_hour?: number;
  keepalive_days: number;
  lazy_refresh_hours: number;
}

export interface CheckinLog {
  ts: number;
  accountId: string | null;
  email: string;
  result: string;
  error?: string;
  /** 该行所属档位；历史日志缺省按国内版处理。 */
  variant?: WbVariant;
}

export interface CheckinResult {
  result: string;
  error?: string;
  /** 国际版签到活动未开放时的业务判定；不写成功日志、不计入失败重试。 */
  inactive?: boolean;
}

export interface TravelConfig {
  enabled: boolean;
}

export type TravelStatusLabel = "untraveled" | "no-buddy" | "traveling" | "finished";

export interface TravelStatus {
  label: TravelStatusLabel;
  rewardCredit: number | null;
  locationName?: string | null;
  arriveAt?: number | null;
}

/** 单个受限模型；`model` 为 null 表示日志里归因不到模型（显示「未知模型」，不猜测）。 */
export interface RateLimitEntry {
  model: string | null;
  /** 官方日志原文给出的恢复时刻（毫秒）。 */
  resetAt: number;
  /** 该事件首次出现的时刻（毫秒）。 */
  firstSeenAt: number;
  /** 去重前的原始命中行数（调试/排查用）。 */
  hitCount: number;
}

/** 一个账号当前受限的全部模型（按 `resetAt` 升序）。 */
export interface AccountRateLimits {
  accountId: string;
  limited: RateLimitEntry[];
}

/** 模型限额台账：一次返回全部账号的当前受限状态（数据来自本机日志）。 */
export interface RateLimitsPayload {
  scannedAt: number;
  /** 固定 2 天，回显便于调试。 */
  windowDays: number;
  /** 只包含至少有一个受限模型的账号。 */
  accounts: AccountRateLimits[];
}

/** 一处客户端 hook 配置的安装状态。 */
export interface RateLimitHookTarget {
  /** 备份标签（codebuddy / workbuddy / workbuddy-ai）。 */
  label: string;
  /** `settings.json` 路径。 */
  path: string;
  /** 该客户端数据根目录是否存在（唯一的存在性判据；不存在则不参与安装）。 */
  exists: boolean;
  /** 该配置里是否已注册本工具的 Stop / FinalStop。 */
  installed: boolean;
}

/**
 * 限额 hook 安装状态：脚本 + 三处客户端配置逐项结果。
 *
 * `installed` = 脚本存在且至少一处配置注册成功；`lastEventAt` 是最近一次由后端
 * 入账的 hook 限额事件时刻（null = 从未收到）。
 */
export interface RateLimitHookStatus {
  scriptPath: string;
  scriptExists: boolean;
  eventsPath: string;
  installed: boolean;
  lastEventAt?: number | null;
  targets: RateLimitHookTarget[];
}

/** 限额监听开关（`~/.workbuddy-shift/rate_limit_config.json`）。 */
export interface RateLimitConfig {
  enabled: boolean;
  /** 用户点过「卸载 hook」→ 启动时不再自动接入；重新点「接入 hook」清除。 */
  hookOptOut: boolean;
  /**
   * 是否扫描两个 CodeBuddy IDE 的日志（默认 true）。
   * IDE 的 429 不触发任何 hook 事件，日志是它唯一的数据源；关闭只影响 IDE 两源，
   * CLI / WorkBuddy 的 hook 实时上报与未接 hook 时的日志兜底不变。
   */
  scanIdeLogs: boolean;
}

export interface AutoRotateConfig {
  enabled: boolean;
  check_interval_minutes: number;
  cooldown_minutes: number;
  min_gap_hours: number;
  min_urgency_hours: number;
  /** 配置键兼容保留：轮换已改用「会话存活门控」，该值不再参与决策，设置页也不再展示。 */
  active_guard_minutes: number;
  min_remaining_credits: number;
}

export interface RotateLog {
  ts: number;
  action: string;
  reason?: string | null;
  from?: { id: string; name?: string | null } | null;
  to?: { id: string; name?: string | null } | null;
}

export interface RotateStatus {
  config: AutoRotateConfig;
  cliConfigured: boolean;
  activeAccountId: string | null;
  activeAccountName: string | null;
  lastCheckAt: number | null;
  lastSwitchAt: number | null;
}

export interface CreditResource {
  packageCode: string | null;
  packageName: string | null;
  total: number;
  remaining: number;
  used: number;
  status: number | null;
  expireAt: number | null;
  expired: boolean;
  expiringSoon: boolean;
}

export interface CreditExpiry {
  ok: boolean;
  accountId?: string | null;
  accountName?: string;
  updatedAt?: number;
  /** 企业版不限量（limitNum=-1）时为 null，展示按 `unlimited` 分支。 */
  totalCapacity?: number | null;
  /** 企业版不限量（limitNum=-1）时为 null，展示按 `unlimited` 分支。 */
  totalRemaining?: number | null;
  expiringSoonRemaining?: number;
  expiredRemaining?: number;
  soonestExpireAt?: number | null;
  expiringSoon?: boolean;
  expired?: boolean;
  resources?: CreditResource[];
  /** 企业配额不限量：数字字段为 null、资源列表为空，不当作 0 展示。 */
  unlimited?: boolean;
  error?: string;
}

export interface CreditStatsSummary {
  currentRemaining: number;
  currentCapacity: number;
  usageToday: number;
  usage7Days: number;
  usageThisMonth: number;
  todayCheckedInAccounts: number;
  todaySuccess: number;
  todayAlready: number;
  todayFailed: number;
}

export interface CreditStatsDailyPoint {
  date: string;
  usage: number;
  /** 官方用量按模型聚合（全量，不受请求明细条数限制）；本地观察口径下为空 */
  models?: { model: string; requestCount: number; credit: number }[];
}

export interface CreditStatsAccount {
  accountId: string;
  accountName: string;
  isCurrent: boolean;
  currentRemaining: number | null;
  totalCapacity: number | null;
  lastSnapshotAt: number | null;
  usageToday: number;
  usage7Days: number;
  usageThisMonth: number;
  checkedInToday: boolean | null;
  checkinStatusToday: string | null;
  lastCheckinAt: number | null;
  lastCheckinResult: string | null;
  /** 按账号的逐日观察消耗（缺省兼容旧后端）；官方可用时趋势图优先使用官方 daily */
  daily?: CreditStatsDailyPoint[];
  /** 档位标记。后端当前不下发，前端容忍性读取；缺省时回退到按 accountId 的映射表 */
  variant?: WbVariant;
}

export interface CreditStatsUsageEvent {
  kind: "usage";
  ts: number;
  date: string;
  accountId: string;
  accountName: string;
  amount: number;
  /** 档位标记。后端当前不下发，前端容忍性读取；缺省时回退到按 accountId 的映射表 */
  variant?: WbVariant;
}

export interface CreditStatsCheckinEvent {
  kind: "checkin";
  ts: number;
  date: string;
  accountId: string | null;
  accountName: string;
  result: string;
  error?: string | null;
  /** 档位标记。后端当前不下发，前端容忍性读取；缺省时回退到按 accountId 的映射表 */
  variant?: WbVariant;
}

export type CreditStatsEvent = CreditStatsUsageEvent | CreditStatsCheckinEvent;

export type CreditOfficialUsageStatus = "complete" | "partial" | "unavailable";

export interface CreditOfficialUsageSummary {
  usageToday: number;
  usage7Days: number;
  usageThisMonth: number;
}

export interface CreditOfficialUsageModel {
  model: string;
  requestCount: number;
  credit: number;
}

export interface CreditOfficialUsageAccount {
  accountId: string;
  accountName: string;
  ok: boolean;
  requestCount: number;
  detailTruncated: boolean;
  usageToday: number | null;
  usage7Days: number | null;
  usageThisMonth: number | null;
  error?: string | null;
  reportedTotal?: number | null;
  fetchedCount?: number;
  /** 缺省兼容旧后端响应。 */
  models?: CreditOfficialUsageModel[];
  /** 按账号的逐日官方消耗（全量聚合，不受 requests 明细上限影响；缺省兼容旧后端） */
  daily?: CreditStatsDailyPoint[];
}

export interface CreditOfficialUsageRequest {
  accountId: string;
  accountName: string;
  requestId: string;
  credit: number;
  model: string;
  client: string;
  requestTime: string;
}

export interface CreditOfficialUsageError {
  accountId: string;
  accountName: string;
  error: string;
}

export interface CreditOfficialUsage {
  status: CreditOfficialUsageStatus;
  rangeStart: string;
  rangeEnd: string;
  /** 官方用量最近一次采集时间；缓存命中时保持采集当时的时间。 */
  collectedAt?: number;
  summary: CreditOfficialUsageSummary;
  daily: CreditStatsDailyPoint[];
  accounts: CreditOfficialUsageAccount[];
  requests: CreditOfficialUsageRequest[];
  /** 官方全部有效请求按模型汇总；不受 requests 明细上限影响。 */
  models?: CreditOfficialUsageModel[];
  detailLimitPerAccount: number;
  errors: CreditOfficialUsageError[];
}

export interface CreditStatistics {
  /** 后端本次统计的总耗时（毫秒）；旧后端可能缺字段。 */
  elapsedMs?: number;
  generatedAt: number;
  retentionDays: number;
  coverageStartAt: number | null;
  /** 本地观察快照的终点；缺省兼容旧后端（此时按 `generatedAt` 解释）。 */
  coverageEndAt?: number | null;
  /**
   * 本地观察口径的覆盖面；缺省兼容旧后端。
   *
   * `covers7Days` / `coversThisMonth` 为 false 时，对应的本地窗口值只是「覆盖范围内的
   * 观察值」，页面必须显示「—」而不是把今日值当成 7 天/本月（issue #225）。
   */
  coverage?: CreditLocalCoverage;
  summary: CreditStatsSummary;
  daily: CreditStatsDailyPoint[];
  accounts: CreditStatsAccount[];
  events: CreditStatsEvent[];
  /** 官方接口不可用时仍使用上述本地观察字段；缺省兼容旧后端。 */
  officialUsage?: CreditOfficialUsage;
}

/** 本地观察的覆盖区间与窗口覆盖判定（后端 `coverage` 字段）。 */
export interface CreditLocalCoverage {
  /** 最早一条本地快照的时刻；无快照为 null。 */
  startAt: number | null;
  /** 最近一条本地快照的时刻；无快照为 null。 */
  endAt: number | null;
  /** 覆盖天数（含首尾）；无快照为 0。 */
  days: number;
  /** 本地快照是否从近 7 天窗口的第一天起就存在。 */
  covers7Days: boolean;
  /** 本地快照是否从本月 1 日起就存在。 */
  coversThisMonth: boolean;
}


export interface TokenStatsTotals { total: number; input: number; output: number; cacheRead: number; cacheWrite: number; uncachedInput: number; records: number; cacheHitRate: number | null; }

export interface TokenStatsGroup extends TokenStatsTotals { key: string; title?: string | null; project?: string; sessionId?: string; /** 归属账号 uid；空串 = 未归属（旧后端缺省）。 */ uid?: string; }

/** 一次模型调用的明细行；`total = input + output + cacheWrite`，`uncachedInput = max(0, input - cacheRead)`，`thinking` 是 `output` 中思考过程的 token 数（回复内容 = max(0, output - thinking)），均与聚合口径一致。 */
export interface TokenStatsRequestRow { timestamp: number; model: string; project: string; sessionId: string; title?: string | null; /** 归属账号 uid；空串 = 未归属（旧后端缺省）。 */ uid?: string; input: number; output: number; cacheRead: number; cacheWrite: number; uncachedInput: number; thinking: number; total: number; }

/**
 * 单个账号的 Token 聚合（一次扫描产出，前端纯客户端过滤）。
 *
 * key 是账号 uid；空串代表「未归属」（DB 缺行、孤儿文件、CLI 日志不记录账号）。
 * 只包含有用量的账号：零用量账号由前端按账号库补 0 显示，避免 payload 膨胀。
 */
export interface TokenStatsAccount {
  uid: string;
  summary: TokenStatsTotals;
  models: TokenStatsGroup[];
  projects: TokenStatsGroup[];
  daily: TokenStatsGroup[];
  dailyByModel?: Record<string, TokenStatsGroup[]>;
  hours: TokenStatsGroup[];
}

/** `workbuddy-ai` 为国际版本地数据源，与国内版分开统计，数据源缺失时为空集。 */
export interface TokenStatsSource { source: "workbuddy" | "workbuddy-ai" | "codebuddy-cli" | "codebuddy-ide"; summary: TokenStatsTotals; models: TokenStatsGroup[]; projects: TokenStatsGroup[]; sessions: TokenStatsGroup[]; daily: TokenStatsGroup[]; /** Optional model-specific daily series for trend filtering. */ dailyByModel?: Record<string, TokenStatsGroup[]>; /** 仅 CodeBuddy CLI 来源返回的最近请求明细；旧后端或缺失时按空数组处理。 */ requests?: TokenStatsRequestRow[]; hours: TokenStatsGroup[]; /** 按账号聚合；旧后端缺省，此时不显示账号下拉。 */ accounts?: TokenStatsAccount[]; filesScanned: number; parseErrors: number; coverageStartAt?: number | null; coverageEndAt?: number | null; }


export interface TokenStatistics { generatedAt: number; rangeDays?: number | null; sources: TokenStatsSource[]; }
