import { useVisibleInterval } from "@/hooks/use-visible-interval";
import { useAccountsStore } from "@/stores/accounts";

/** Keep credits fresh while the main window is visible; pause when it is hidden. */
export const CREDIT_REFRESH_INTERVAL_MS = 30 * 60 * 1000;

function currentAccountIds(): string[] {
  return useAccountsStore.getState().accounts.map((account) => account.id);
}

/**
 * 恢复可见 / 挂载时的刷新计划：从未拉取过则 ensureCredits（只补缺失），
 * 距上次刷新已满一个间隔才静默刷新，避免切回页面就重复请求。
 */
export function resolveCreditRefreshAction(
  lastRefreshAt: number,
  now: number,
): "ensure" | "refresh" | "none" {
  if (lastRefreshAt === 0) return "ensure";
  return now - lastRefreshAt >= CREDIT_REFRESH_INTERVAL_MS ? "refresh" : "none";
}

function refreshOnShow() {
  const ids = currentAccountIds();
  if (ids.length === 0) return;
  const last = useAccountsStore.getState().lastCreditRefreshAt;
  const action = resolveCreditRefreshAction(last, Date.now());
  if (action === "ensure") void useAccountsStore.getState().ensureCredits(ids);
  else if (action === "refresh") void useAccountsStore.getState().refreshCredits(ids, { silent: true });
}

/** 每个间隔的强制刷新：只要求有账号，不做「是否刚刷新过」的二次判断（与原实现一致）。 */
function refreshCreditsOnTick() {
  const ids = currentAccountIds();
  if (ids.length === 0) return;
  void useAccountsStore.getState().refreshCredits(ids, { silent: true });
}

/**
 * 账号页积分自动刷新：可见性、30 分钟间隔与「恢复可见时按需补拉」全部复用
 * `useVisibleInterval`；这里只声明两种动作的差异（进入可见按需 / 间隔强制）。
 */
export function useCreditAutoRefresh() {
  useVisibleInterval(refreshCreditsOnTick, CREDIT_REFRESH_INTERVAL_MS, true, {
    onShow: refreshOnShow,
    onTick: refreshCreditsOnTick,
  });
}
