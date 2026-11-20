import type { DisplayField } from "@/lib/types";

/**
 * 显示名计算所需字段（均可缺省）：
 * 本地账号记录、当前登录态（AppStatus.current）、备份预览记录都能直接传入。
 */
export interface DisplayableAccount {
  nickname?: string | null;
  uid?: string | null;
  id?: string | null;
  email?: string | null;
  /** 官方手机号（仅国内版账号可能有）。 */
  phoneNumber?: string | null;
  /** 本地备注。 */
  note?: string | null;
  /** 本地显示字段偏好。 */
  displayField?: DisplayField | null;
}

/** 取非空字符串（trim 后为空按缺失处理，与后端 `account::get_str` 同语义）。 */
function trimmed(value: string | null | undefined): string {
  return (value ?? "").trim();
}

/**
 * 账号显示名（**全局唯一口径**，见 R5-07）：备注优先。
 *
 * 取值顺序与后端 `crates/wb-shift-core/src/modules/account/display.rs` 的
 * `account_display_name()` **逐条一致**；改动必须两边同步，并在两边的单测里锁同一组用例：
 *
 * 1. `displayField === "phone"` 且手机号非空 → 手机号（唯一压过备注的例外）
 * 2. `note` 非空 → 备注
 * 3. `nickname` 非空 → 昵称
 * 4. `email` 非空 → 邮箱
 * 5. `uid` 非空 → uid
 * 6. `id` 非空 → id
 * 7. 兜底 → 「未命名账号」（后端为 `unknown`）
 *
 * 产品决策：「用户自己写的备注必须压过官方昵称 / 邮箱」。此前前端是
 * `displayField → 昵称 → uid → id`（默认忽略备注）、Token 统计下拉是「昵称 → 邮箱」，
 * 同一账号在不同页面会叫不同的名字；本轮统一收敛到本函数。
 *
 * 所有展示账号名的位置（账号卡片、切换弹窗、Token 统计下拉、积分统计、签到日志等）
 * 都必须走本函数，不要在调用处另写 `nickname || email` 的私有规则。
 */
export function displayName(account: DisplayableAccount): string {
  const phone = account.displayField === "phone" ? trimmed(account.phoneNumber) : "";
  return (
    phone ||
    trimmed(account.note) ||
    trimmed(account.nickname) ||
    trimmed(account.email) ||
    trimmed(account.uid) ||
    trimmed(account.id) ||
    "未命名账号"
  );
}

/** 账号身份行：邮箱脱敏展示（本地段只留首字符），无邮箱时回退 UID / ID。 */
export function accountIdentity(account: DisplayableAccount): string {
  if (account.email) {
    const [local, domain] = account.email.split("@");
    if (!domain) return account.email;
    return `${local.slice(0, 1)}${"*".repeat(Math.max(3, local.length - 1))}@${domain}`;
  }
  return account.uid ? `UID · ${account.uid}` : `ID · ${account.id}`;
}
