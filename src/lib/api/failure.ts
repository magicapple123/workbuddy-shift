/**
 * 结构化失败信息：把「哪一步失败、下一步该做什么」从后端带到界面。
 *
 * # 为什么需要它
 *
 * 后端（`crates/wb-shift-core/src/modules/config/failure.rs`）早就把**走 HTTP 且响应不是
 * JSON** 的失败分成了四档，并在失败对象里带上 `failureKind` / `httpStatus`。但这些字段在
 * 边界上会被压平成一句字符串（`Result<_, String>` / `json_err(String)`），前端再
 * `throw new Error(data.message || …)` 就彻底丢了结构——界面只知道「发生了什么」，
 * 不知道「下一步做什么」。
 *
 * 这个模块负责**零协议改动**地把结构还原回来：
 *
 * - webui：非 2xx 响应体里本来就有这些字段；
 * - 桌面端：`failure_response` 的序列化结果本身是一段 JSON，`invoke` 的 `Err(String)`
 *   直接 `JSON.parse` 就能还原。
 *
 * **「下一步动作」不进协议**：那是界面决策（同一档失败在登录页与统计页的落点不同），
 * 由 {@link ACTION_BY_KIND} 在前端映射。协议仍然只有 `code` / `message` / `failureKind` /
 * `httpStatus` 四个字段。
 *
 * # 覆盖范围
 *
 * 只有经过 `http_request` 且响应非 JSON 的调用点才会产生四档（积分、签到、旅行、限额、
 * 更新检查、刷新令牌……）。切号（纯本地文件操作）与 OAuth 的 `fail_oauth`（超时 / 域名不匹配 /
 * 保存账号失败）**不产生四档**，那些界面上的动作行只在真有 `failureKind` 时出现。
 *
 * # 为什么用 class 而不是普通对象
 *
 * `class ApiFailure extends Error` 让 `instanceof Error` 依旧成立，
 * 于是「把任意抛出值当 Error 用」的老代码不会因为这次改动而走岔路。
 */

import type { FailureKind, RecoveryAction } from "../types";

export type { FailureKind, RecoveryAction };

/** 结构化失败的原始字段（全部可为空，老后端 / 非 HTTP 失败不会有这些字段）。 */
export interface FailureDetails {
  code: number | null;
  message: string;
  failureKind: FailureKind | null;
  httpStatus: number | null;
}

const FAILURE_KINDS: ReadonlySet<string> = new Set<FailureKind>([
  "login_expired",
  "gateway_html",
  "upstream",
  "malformed",
]);

/**
 * 四档 → 动作（界面决策，唯一一处映射）。
 *
 * `upstream` 刻意是 `null`：上游已经给了一句人话，再叠一个可能指向错方向的按钮
 * 只会把用户往错的方向推。
 */
const ACTION_BY_KIND: Readonly<Record<FailureKind, RecoveryAction | null>> = {
  login_expired: "relogin",
  gateway_html: "check_proxy",
  malformed: "copy_diagnostic",
  upstream: null,
};

/** 档位 → 动作；未知 / 缺省档位返回 `null`。 */
export function actionForKind(kind: FailureKind | null): RecoveryAction | null {
  return kind === null ? null : ACTION_BY_KIND[kind];
}

/** 携带结构化失败信息的错误。`message` 与改造前的字符串逐字一致。 */
export class ApiFailure extends Error {
  /** 业务码（后端 `code`；HTTP 失败时缺省等于状态码）。 */
  readonly code: number | null;
  /** 失败档位；老后端 / 非 HTTP 失败可能没有。 */
  readonly failureKind: FailureKind | null;
  /** HTTP 状态码（排障用）。 */
  readonly httpStatus: number | null;
  /** 由 `failureKind` 映射出的下一步动作；`null` 表示「没有可执行的下一步」。 */
  readonly action: RecoveryAction | null;

  constructor(details: FailureDetails) {
    super(details.message);
    this.name = "ApiFailure";
    this.code = details.code;
    this.failureKind = details.failureKind;
    this.httpStatus = details.httpStatus;
    this.action = actionForKind(details.failureKind);
  }
}

/** 空字段集合（构造「只有文案」的失败时用）。 */
function emptyDetails(message: string): FailureDetails {
  return { code: null, message, failureKind: null, httpStatus: null };
}

/** 只接受有限数字；字符串数字、`NaN`、`Infinity` 一律视为缺失。 */
function asNumber(value: unknown): number | null {
  return typeof value === "number" && Number.isFinite(value) ? value : null;
}

function asKind(value: unknown): FailureKind | null {
  return typeof value === "string" && FAILURE_KINDS.has(value) ? (value as FailureKind) : null;
}

/** 安全解析 JSON：失败返回 `undefined`（调用方据此走「不是结构化错误」的分支）。 */
function parseJson(text: string): unknown {
  const trimmed = text.trim();
  if (!trimmed.startsWith("{") || !trimmed.endsWith("}")) return undefined;
  try {
    return JSON.parse(trimmed) as unknown;
  } catch {
    return undefined;
  }
}

/**
 * 从任意值里抽出结构化失败字段；**一个字段都没有时返回 `null`**。
 *
 * 只认对象（数组 / 基本类型一律 `null`）：Tauri 的 `Err(String)` 可能是任意字符串，
 * 无脑 `JSON.parse` 一个 `"404 Not Found"` 只会制造假的结构化错误。
 * 未知 `failureKind` 按「没有档位」处理，不猜、不炸。
 */
export function parseFailurePayload(value: unknown): FailureDetails | null {
  if (typeof value !== "object" || value === null || Array.isArray(value)) return null;
  const record = value as Record<string, unknown>;
  const failureKind = asKind(record.failureKind);
  const httpStatus = asNumber(record.httpStatus);
  const code = asNumber(record.code);
  if (failureKind === null && httpStatus === null && code === null) return null;
  const message =
    typeof record.message === "string"
      ? record.message
      : typeof record.error === "string"
        ? record.error
        : "";
  return { code, message, failureKind, httpStatus };
}

/**
 * HTTP 非 2xx 响应 → {@link ApiFailure}。
 *
 * `message` 的取值顺序与改造前**逐字一致**（`data.message || data.error || 请求失败 (status)`），
 * 因此既有文案函数看到的字符串不会变；多出来的只有结构化字段。
 */
export function failureFromResponse(status: number, data: unknown): ApiFailure {
  const record =
    typeof data === "object" && data !== null ? (data as Record<string, unknown>) : {};
  const fromMessage = typeof record.message === "string" ? record.message : "";
  const fromError = typeof record.error === "string" ? record.error : "";
  const fallback = fromMessage || fromError || `请求失败 (${status})`;
  const parsed = parseFailurePayload(record);
  return new ApiFailure({
    code: parsed?.code ?? status,
    message: fallback,
    failureKind: parsed?.failureKind ?? null,
    // 网页版恒可从响应本身拿到状态码（后端没在 body 里带也能补上）。
    httpStatus: parsed?.httpStatus ?? status,
  });
}

/**
 * 只在「确实带结构化字段」时把抛出值转成 {@link ApiFailure}，否则返回 `null`。
 *
 * 这是给 `call()` 用的：Tauri 的 `Err(String)` 如果是 JSON 就还原结构，
 * 如果是普通字符串就**原样放行**，保证既有 `catch` 分支拿到的值形状完全不变。
 */
export function structuredFailureOf(e: unknown): ApiFailure | null {
  if (e instanceof ApiFailure) return e;
  if (typeof e === "string") {
    const parsed = parseFailurePayload(parseJson(e));
    return parsed ? new ApiFailure(parsed) : null;
  }
  if (e instanceof Error) {
    const parsed = parseFailurePayload(parseJson(e.message));
    return parsed ? new ApiFailure(parsed) : null;
  }
  const parsed = parseFailurePayload(e);
  return parsed ? new ApiFailure(parsed) : null;
}

/**
 * 把任意抛出值统一成 {@link ApiFailure}（新代码用）。
 *
 * `message` 与既有文案函数（`core.ts` 里的 `asError`）口径逐字一致：
 * 字符串原样、`Error` 取 `message`、其余 `JSON.stringify(e ?? "未知错误")`。
 * **不要**用它替换 `asError()`——那是一百多处调用点的既有契约。
 */
export function asFailure(e: unknown): ApiFailure {
  const structured = structuredFailureOf(e);
  if (structured) return structured;
  if (typeof e === "string") return new ApiFailure(emptyDetails(e));
  if (e instanceof Error) return new ApiFailure(emptyDetails(e.message));
  return new ApiFailure(emptyDetails(JSON.stringify(e ?? "未知错误")));
}

/** 取失败的动作；对非 {@link ApiFailure} 的抛出值也成立（先归一化）。 */
export function failureActionOf(e: unknown): RecoveryAction | null {
  return asFailure(e).action;
}

/**
 * `copy_diagnostic` 动作复制出去的文本。
 *
 * 只包含「作者会问的那几项」，且都是脱敏口径：不含账号、令牌、路径、会话内容。
 */
export function failureDiagnostic(
  failure: ApiFailure,
  options: { source?: string; host?: string } = {},
): string {
  const lines = [
    "WorkBuddy Shift 失败诊断",
    `时间: ${new Date().toISOString()}`,
    `来源: ${options.source?.trim() || "未知界面"}`,
    `失败档位: ${failure.failureKind ?? "（未提供）"}`,
    `HTTP 状态: ${failure.httpStatus === null ? "（未提供）" : failure.httpStatus}`,
    `业务码: ${failure.code === null ? "（未提供）" : failure.code}`,
    `提示文案: ${failure.message}`,
  ];
  if (options.host) lines.push(`宿主: ${options.host}`);
  lines.push("完整排障信息：设置 → 诊断包 → 预览 / 导出");
  return lines.join("\n");
}
