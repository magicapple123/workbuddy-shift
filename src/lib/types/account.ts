import { SessionCopyReport, SessionRecoveryReport, SessionSyncReport } from "./session";

/**
 * WorkBuddy 客户端档位：国内版（cn）/ 国际版（ai）。
 * 后端以字符串返回，历史数据与旧响应可能缺省该字段，读取时统一按国内版处理。
 */
export type WbVariant = "cn" | "ai";

/**
 * 账号卡片显示名取哪个字段（每账号独立的本地偏好）。
 * 缺省/异常按 `nickname` 处理；选定字段为空时回退 `nickname → uid`。
 */
export type DisplayField = "nickname" | "phone" | "note";

export interface AccountMeta {
  id: string;
  uid: string | null;
  email: string | null;
  nickname: string | null;
  /** 官方手机号（实时读 profile_raw，仅国内版账号可能有）。 */
  phoneNumber?: string | null;
  /** 本地备注（用户自填，不来自官方数据）。 */
  note?: string | null;
  /** 本地显示字段偏好。 */
  displayField?: DisplayField | null;
  enterpriseName: string | null;
  expiresAt: number | null;
  refreshExpiresAt: number | null;
  refreshedAt: number | null;
  createdAt: number | null;
  needsRelogin: boolean;
  needsReloginReason: string | null;
  /** 账号所属档位；缺省（旧后端/历史账号）按国内版处理。 */
  variant?: WbVariant;
  /**
   * 企业版账号（有 enterpriseId，或 type 为 ultimate/exclusive）；
   * 缺省（旧后端/历史账号）按非企业处理。
   */
  isEnterprise?: boolean;
}

export interface AppStatus {
  running: boolean;
  authFile: string;
  current: {
    uid: string | null;
    nickname: string | null;
    email: string | null;
  } | null;
  appPath: string;
  version: string;
  /** 上述字段所属档位；缺省按国内版处理。 */
  variant?: WbVariant;
}

export interface OAuthStartResult {
  loginId: string;
  verificationUri: string;
  expiresIn: number;
}

export interface OAuthPollResult {
  done: boolean;
  result?: AccountMeta;
  error?: string;
}

/** 导出文件中的完整账号记录（含 token，仅导出命令返回；字段与账号库原始记录一致）。 */
export interface AccountRecord {
  id?: string;
  uid?: string | null;
  nickname?: string | null;
  email?: string | null;
  access_token?: string | null;
  refresh_token?: string | null;
  token_type?: string | null;
  domain?: string | null;
  expiresAt?: number | null;
  refreshExpiresAt?: number | null;
  auth_raw?: unknown;
  profile_raw?: unknown;
  createdAt?: number | null;
  [key: string]: unknown;
}

/** 导入文件账号的脱敏预览（不含 token）。 */
export interface ImportPreviewAccount {
  index: number;
  uid: string | null;
  nickname: string | null;
  email: string | null;
  hasToken: boolean;
  /** access_token 为 WorkBuddy 加密信封：可导入，但仅切换可用（签到/积分不可用）。 */
  encrypted: boolean;
}

/** 导入结果计数。 */
export interface ImportResult {
  ok: boolean;
  imported: number;
  skipped: number;
  overwritten: number;
}

export interface SwitchResult {
  ok: boolean;
  account: string;
  /** 目标账号自身档位；缺省按国内版处理。 */
  variant?: WbVariant;
  backup: string | null;
  sessionCopy?: SessionCopyReport;
  /** 本次的会话同步报告（未勾选同步时不返回）；含跳过与失败原因，不只是成功数。 */
  sessionSync?: SessionSyncReport;
  sessionRecovery?: SessionRecoveryReport;
}
