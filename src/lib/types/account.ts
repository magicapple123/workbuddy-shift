import { SessionCopyReport, SessionRecoveryReport, SessionSyncReport } from "./session";

/**
 * WorkBuddy 客户端档位：国内版（cn）/ 国际版（ai）。
 * 后端以字符串返回，历史数据与旧响应可能缺省该字段，读取时统一按国内版处理。
 */
export type WbVariant = "cn" | "ai";

/**
 * 账号卡片显示名取哪个字段（每账号独立的本地偏好）。
 *
 * R5-07 起显示名统一为「备注优先」（见 `src/lib/account-display.ts` 的 `displayName`）：
 * - `nickname`（缺省/异常）：账号名 = 无备注时的回退，有备注时仍显示备注；
 * - `phone`：手机号**始终压过备注**（唯一例外）；
 * - `note`：与新默认等价，仅为兼容存量数据保留，弹窗里映射到「账号名」。
 */
export type DisplayField = "nickname" | "phone" | "note";

/**
 * 账号标记的语义色（R5-04）。与后端 `MARK_COLORS` 一一对应，顺序也保持一致。
 *
 * - `warning`：需要留意（如「即将限额」）；
 * - `danger`：明确不可用 / 已废弃（浅底 + 同色文字，不做实心 destructive）；
 * - `info`：中性提示；
 * - `success`：正向状态；
 * - `neutral`：纯备注性质。
 */
export type AccountMarkColor = "warning" | "danger" | "info" | "success" | "neutral";

/**
 * 账号自定义标记（R5-04）：单账号最多一条，重复保存即覆盖。
 *
 * 只保存在本机账号库（`accounts.json` 的同级字段 `mark`），随账号备份导出/导入，
 * 不同步官方；到期后卡片上不再显示图标，但**记录保留**，编辑时提示「已过期」可续期。
 */
export interface AccountMark {
  /** 标记文本（1–16 字，纯文本，不含换行）。 */
  text: string;
  color: AccountMarkColor;
  /** 创建时刻（绝对毫秒），由后端写入，客户端传值会被忽略。 */
  createdAt: number;
  /** 到期时刻（绝对毫秒）；`null` = 不自动过期（只能手动解除）。 */
  expiresAt: number | null;
}

/**
 * 写入用的标记载荷：`createdAt` 由后端生成，客户端不传（传了也会被忽略）。
 *
 * `expiresAt` 必须是 `null`（不自动过期）或**晚于当前时刻**的毫秒数，
 * 过去时间会被后端拒绝（「到期时间必须晚于当前时间」）。
 */
export type UpdateAccountMark = Pick<AccountMark, "text" | "color" | "expiresAt">;

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
  /**
   * 本地自定义标记（R5-04）：`null` / 缺省（旧账号库）= 无标记。
   * 后端下发的 `mark` 已做过类型校验，任何一项不合法都会整体折叠成 `null`。
   */
  mark?: AccountMark | null;
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

/**
 * 启用中的「自动化」定时任务摘要（切号提示用）。
 *
 * 任务按 `owner_user_id` 绑账号：切号后原账号的任务不再执行，客户端界面却不给任何提示，
 * 因此切号弹窗要用 `count` 主动提醒；`names` 最多 5 条，超出由前端显示「等 N 个」。
 */
export interface AutomationTasksSummary {
  count: number;
  names: string[];
  /** 同步开关（默认关闭）：开启后切号会把简单任务复制到目标账号并暂停源任务。 */
  enabled: boolean;
}

/** 切号时自动化任务的同步报告；未开启同步或未重启时不出现在结果里。 */
export interface AutomationSyncReport {
  ok: boolean;
  backup?: string;
  copied: { id: string; name: string }[];
  paused: { id: string; name: string }[];
  skipped: { name: string; reason: string }[];
  error?: string;
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
  /** 自动化定时任务的同步报告（开关关闭或未勾选重启时不返回）。 */
  automationSync?: AutomationSyncReport;
}
