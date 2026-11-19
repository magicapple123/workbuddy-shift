

export function formatTime(ts: number): string {
  try {
    return new Date(ts).toLocaleString("zh-CN", {
      month: "2-digit",
      day: "2-digit",
      hour: "2-digit",
      minute: "2-digit",
      second: "2-digit",
    });
  } catch {
    return String(ts);
  }
}

export function logLabel(result: string): { text: string; tone: "success" | "warning" | "error" } {
  switch (result) {
    case "success":
      return { text: "签到成功", tone: "success" };
    case "already":
      return { text: "已签到", tone: "warning" };
    default:
      return { text: "失败", tone: "error" };
  }
}

/** "HH:MM" → 当日分钟数；非法返回 null。 */
function clockMinutes(value: string): number | null {
  const match = /^(\d{1,2}):(\d{2})$/.exec(value);
  if (!match) return null;
  const hour = Number(match[1]);
  const minute = Number(match[2]);
  if (hour > 23 || minute > 59) return null;
  return hour * 60 + minute;
}

/**
 * 签到时间段的非法组合说明；返回 null 表示可以落盘（两端都合法或两端都空）。
 *
 * 半填 / 非法组合只改本地显示、不写盘：后端把这种窗口当「不限制」处理，写盘会让下一轮
 * 对全部账号发起窗口外签到，所以文案要说明「未保存」。
 */
export function checkinWindowIssue(start: string, end: string): string | null {
  if (!start && !end) return null;
  if (!start || !end) return "开始与结束时间需同时选择，当前选择未保存";
  const startMinutes = clockMinutes(start);
  const endMinutes = clockMinutes(end);
  if (startMinutes === null || endMinutes === null) return "时间格式应为 HH:MM，当前选择未保存";
  if (startMinutes >= endMinutes) return "结束时间需晚于开始时间（不支持跨午夜），当前选择未保存";
  return null;
}

/** 数字参数：展示名 + 收敛边界（与输入框 min/max 同源）。 */
export interface NumberFieldSpec {
  label: string;
  min: number;
  max?: number;
}

/** 数字输入收敛：空 / 非数字返回 null（调用方回退到已落盘值），越界收敛到边界。 */
export function resolveNumberInput(raw: string, field: NumberFieldSpec): number | null {
  const text = raw.trim();
  if (!text) return null;
  const parsed = Number(text);
  if (!Number.isFinite(parsed)) return null;
  const rounded = Math.round(parsed);
  const max = field.max ?? Number.POSITIVE_INFINITY;
  return Math.min(Math.max(rounded, field.min), max);
}

/** 空值 / 非数字的回退提示。 */
export function numberInputHint(field: NumberFieldSpec): string {
  return field.max === undefined
    ? `${field.label}需为不小于 ${field.min} 的数字，已恢复为原值`
    : `${field.label}需为 ${field.min}–${field.max} 之间的数字，已恢复为原值`;
}

/**
 * 代理地址归一化：只去掉首尾空白。
 *
 * 用户从文档/聊天窗口粘贴时经常带上换行与前后空格；保留原样的首尾空白会让
 * 「改回原值」被判成脏（按钮不消失），也会让落盘的地址与展示值不一致。
 * 内部空白一律保留（`http:// user:pass@host` 这种写法不是我们要纠正的错）。
 */
export function normalizeProxy(raw: string | null | undefined): string {
  return (raw ?? "").trim();
}

/**
 * 代理地址校验：空串合法（表示关闭代理），非空必须是带 hostname 的 http(s) 地址。
 *
 * 返回 `null` 表示合法，否则返回可直接展示给用户的原因。只认 http/https：
 * 其他 scheme（ftp / socks5 / file）在 reqwest 侧会以更难懂的错误失败，
 * 在这里拦下来文案才说得清。
 */
export function validateProxyUrl(raw: string | null | undefined): string | null {
  const value = normalizeProxy(raw);
  if (!value) return null;
  let parsed: URL;
  try {
    parsed = new URL(value);
  } catch {
    return "代理地址格式不正确，请填写 HTTP/HTTPS 地址，例如 http://127.0.0.1:7897";
  }
  if (!["http:", "https:"].includes(parsed.protocol)) {
    return "代理地址只支持 http:// 或 https://，例如 http://127.0.0.1:7897";
  }
  if (!parsed.hostname) {
    return "代理地址缺少主机名，例如 http://127.0.0.1:7897";
  }
  return null;
}

/**
 * 代理输入框是否有未保存的修改（忽略首尾空白差异）。
 *
 * 未加载配置时把已保存值视为空串：此时输入框为空 → 不脏 → 不显示「保存代理」。
 * 只差空格也算「没改」——否则用户点进输入框再离开，按钮就会常驻。
 */
export function isProxyDirty(current: string | null | undefined, saved: string | null | undefined): boolean {
  return normalizeProxy(current) !== normalizeProxy(saved);
}

/** 待提交的一次落盘：编辑字段 + 成功后提示（连续编辑合并时提示取最新一次）。 */
export interface SaveCommit<T> {
  edits: Partial<T>;
  success?: { title: string; description?: string };
}

/** 签到数字参数（后端不做范围校验，前端按这里的边界收敛后提交）。 */
export const CHECKIN_NUMBER_FIELDS = {
  keepalive_days: { label: "保活阈值", min: 0, max: 90 },
  lazy_refresh_hours: { label: "惰性刷新", min: 1, max: 72 },
} as const satisfies Record<string, NumberFieldSpec>;

export type CheckinNumberKey = keyof typeof CHECKIN_NUMBER_FIELDS;

/** 轮换数字参数。 */
export const ROTATE_NUMBER_FIELDS = {
  check_interval_minutes: { label: "检查间隔", min: 1, max: 1440 },
  cooldown_minutes: { label: "切换冷却", min: 1, max: 1440 },
  min_gap_hours: { label: "到期差异阈值", min: 0, max: 720 },
  min_urgency_hours: { label: "到期紧迫阈值", min: 0, max: 720 },
  min_remaining_credits: { label: "最小剩余积分", min: 0 },
} as const satisfies Record<string, NumberFieldSpec>;

export type RotateNumberKey = keyof typeof ROTATE_NUMBER_FIELDS;
