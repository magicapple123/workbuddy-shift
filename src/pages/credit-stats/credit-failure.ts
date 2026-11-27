/**
 * 积分页的「失败 → 动作」派生逻辑（纯函数，便于单测）。
 *
 * # 为什么是 `errors[]` 而不是 catch
 *
 * 桌面端 `get_credit_expiry` / `get_credit_statistics` 都是 `Result` 之外的形状
 * （`-> Value` 或 `Ok(Value)`），**失败时也是正常 resolve**：Value 里是
 * `{ok:false, error, …}`。所以 `call()` 不会 reject，`catch` 里的 `asFailure()` 在桌面端
 * 永远不执行——把动作挂在 catch 上等于「按钮恒不渲染」。
 *
 * 用户真正看到的那句失败文案来自数据通道：`officialUsage.errors[]`
 * （`index.tsx` 的 `officialFailureText`）。档位在 core 侧由
 * `official_usage::UsageFetchError` 一路带到这里，前端只需从 `errors[]` 里挑出第一条
 * 带档位的项，用 `failureFromResponse()` 还原成 `ApiFailure`。
 *
 * # 「重新登录」的落点
 *
 * 需要知道是哪个账号失败了（用来选档位）：`errors[]` 项带 `accountId`，但账号可能已被
 * 删除。拿不到可用账号时调用方**不传 handler**（`FailureActions` 在无 handler 时整块
 * 返回 `null`，不会留一个点了没用的按钮）。
 */
import type { AccountMeta, CreditOfficialUsageError, WbVariant } from "@/lib/types";
import { accountVariant } from "@/lib/variant";
import { failureFromResponse, type ApiFailure } from "@/lib/api";

/** 从 `errors[]` 里挑出来的一条可行动失败。 */
export interface CreditFailureHit {
  /** 结构化失败（`failureKind` 一定非空，否则不会成为 hit）。 */
  failure: ApiFailure;
  /** 失败所属账号 id（用于「重新登录」选档位）。 */
  accountId: string | null;
  accountName: string | null;
}

/**
 * 在官方用量的 `errors[]` 里挑第一条**带档位**的失败。
 *
 * 只挑带 `failureKind` 的：没有档位就没有动作，而那条文案本来已经在页面上显示了，
 * 拿它去生成一个空动作行只会多出一块不可点击的留白。多个账号同时失败时取第一条，
 * 与页面「一条汇总文案」的既有形态一致。
 */
export function findCreditFailure(
  errors: CreditOfficialUsageError[] | undefined,
): CreditFailureHit | null {
  for (const entry of errors ?? []) {
    if (!entry.failureKind) continue;
    const failure = failureFromResponse(entry.httpStatus ?? 0, entry);
    if (!failure.failureKind) continue;
    return {
      failure,
      accountId: typeof entry.accountId === "string" ? entry.accountId : null,
      accountName: typeof entry.accountName === "string" ? entry.accountName : null,
    };
  }
  return null;
}

/** `relogin` 的目标账号；拿不到可用上下文（无 accountId / 账号已被删）时返回 `null`。 */
export function reloginAccountOf(
  hit: CreditFailureHit | null,
  accounts: AccountMeta[],
): AccountMeta | null {
  const id = hit?.accountId;
  if (!id) return null;
  return accounts.find((account) => account.id === id) ?? null;
}

/** 目标账号的档位（给 OAuth 登录弹窗用）；没有目标账号时用默认档位。 */
export function reloginVariantOf(account: AccountMeta | null): WbVariant {
  return accountVariant(account);
}

/**
 * `FailureActions` 需要的那组 props。
 *
 * 单独抽出来是为了让「按钮到底会不会出现、点了走哪条流程」可被单测直接断言
 * （本仓测试环境是 node，没有 jsdom / @testing-library，无法模拟真实 click）。
 */
export interface FailureActionsProps {
  failure: ApiFailure | null;
  source: string;
  onRelogin?: () => void;
}

/** 组装动作行 props：只有拿到目标账号时才给 `onRelogin`。 */
export function creditFailureActionsProps(
  hit: CreditFailureHit | null,
  accounts: AccountMeta[],
  openOauth: () => void,
): FailureActionsProps {
  const account = reloginAccountOf(hit, accounts);
  return {
    failure: hit?.failure ?? null,
    source: "credit-stats",
    ...(account ? { onRelogin: openOauth } : {}),
  };
}
