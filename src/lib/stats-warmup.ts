/**
 * 统计启动预热的纯编排逻辑。
 *
 * 本文件**零副作用**：不发请求、不装监听、不读 DOM、不写 localStorage。所有外部能力
 * 都从 `WarmupDeps` 注入，因此可以在 node 环境里用 fake deps 精确断言调用顺序与门控。
 * 副作用集中在 `@/hooks/use-stats-warmup`（空闲调度、可见性订阅、真实 api 注入）。
 *
 * 顺序（交付规格 4.3 的 T2.2）：
 * 1. 演示模式 / 主窗口不可见 → 全部 skipped，一个请求都不发；
 * 2. 等账号 store 就绪（复用注入的 `fetchAccounts`，见 hook 里的在途 join）；
 * 3. 本地积分投影 `loadCredit(false)`（页面先有数字）；
 * 4. 第 3 步完成后启动 `loadToken()`，与官方用量门控**并行**；
 * 5. 官方用量：账号数 > 0 且 `officialUsage.collectedAt` 缺失或超过 30 分钟才补采，
 *    补采后回读一次积分（回填官方用量）；
 * 6. 失败静默：任何一步抛错都不 toast、不写页面错误态、不置完成标记。
 */

import {
  invalidateCreditStatisticsCache,
  isTokenStatisticsCacheFresh,
  readCreditStatisticsCache,
} from "@/lib/stats-cache";

/** 官方用量补采门控：与积分页的自动补采间隔保持一致（30 分钟）。 */
export const OFFICIAL_USAGE_TTL_MS = 30 * 60 * 1000;

/** 积分侧结果：`hit` 表示直接用了缓存值，`refreshed` 表示这次真的取了服务端。 */
export type WarmupCreditState = "hit" | "refreshed" | "skipped";

/**
 * Token 侧结果：`hit` 复用新鲜缓存、`loaded` 本次加载成功、`failed` 加载失败（静默）、
 * `skipped` 未执行（演示模式 / 窗口不可见）。
 *
 * 规格 4.3 只列了前三项；`skipped` 是为了满足验收 4.1-5/6（演示模式、不可见时必须
 * 明确「没做」）与 T2.4 的断言要求而补充的**超集**成员，旧的 `=== "loaded"` 判断不受影响。
 */
export type WarmupTokenState = "hit" | "loaded" | "failed" | "skipped";

/** 官方用量侧结果：`fresh` 缓存新鲜未补采、`refreshed` 本次补采、`skipped` 无账号或失败。 */
export type WarmupOfficialState = "fresh" | "refreshed" | "skipped";

/** 预热结果：供页面日志与 QA 断言（不影响页面渲染）。 */
export interface WarmupResult {
  credit: WarmupCreditState;
  token: WarmupTokenState;
  official: WarmupOfficialState;
}

/** 预热所需的全部外部能力；由 hook 注入真实实现，单测注入 fake。 */
export interface WarmupDeps {
  /** 演示模式（`api.isDemoMode()`）：不预热、不发请求。 */
  isDemo: () => boolean;
  /** 主窗口是否可见（托盘静默启动时为 false）。 */
  isVisible: () => boolean;
  /** 账号 store 的 fetchAll；调用方保证「已有在途则 join」，本文件不自己发请求。 */
  fetchAccounts: () => Promise<void>;
  /** 账号 store 是否已有在途请求（仅用于日志与调用方语义说明）。 */
  hasAccountsInflight?: () => boolean;
  /** 当前账号列表（只读 `length`）。 */
  getAccounts: () => { length: number };
  /** 从积分缓存里读官方用量采集时刻（毫秒，缺失为 null）。 */
  getOfficialCollectedAt: () => number | null;
  /** 读积分统计；`refresh=true` 要求重新采集官方用量。 */
  loadCredit: (refresh?: boolean) => Promise<unknown>;
  /** 读 Token 统计（内部有 single-flight，重复调用不会重复扫盘）。 */
  loadToken: () => Promise<unknown>;
  /** 只采集官方用量（服务端单飞）。 */
  refreshOfficialUsage: () => Promise<unknown>;
  /** 时间源，便于单测；缺省 `Date.now`。 */
  now?: () => number;
  /** T2.5：预取两个统计页的 chunk（不放请求）；演示模式不调用。 */
  prefetchRoutes?: () => void;
}

/** 只用于 `console.info` 的错误文案，不参与 UI，因此不复用 `api.asError` 的严格格式。 */
function describeError(cause: unknown): string {
  if (typeof cause === "string") return cause;
  if (cause instanceof Error) return cause.message;
  try {
    return JSON.stringify(cause);
  } catch {
    return String(cause);
  }
}

/**
 * 执行一次统计预热。**永不 reject**：所有失败都收敛成对应的 `skipped` / `failed`。
 */
export async function runStatsWarmup(deps: WarmupDeps): Promise<WarmupResult> {
  const result: WarmupResult = { credit: "skipped", token: "skipped", official: "skipped" };

  if (deps.isDemo() || !deps.isVisible()) {
    // 演示模式没有真实后端；不可见（托盘静默启动）时由 hook 等恢复可见后重新调度。
    console.info("[stats-warmup]", result);
    return result;
  }

  try {
    await warmUp(deps, result);
  } catch (cause) {
    // 兜底：任何未预期的抛出都不影响页面，页面上仍有自己的加载与重试路径。
    console.info("[stats-warmup] aborted", describeError(cause));
  }

  console.info("[stats-warmup]", result);
  // T2.5：最后预取 chunk，只下载 JS、不发请求；失败也不影响上面的结果。
  deps.prefetchRoutes?.();
  return result;
}

/** 预热主体：按规格顺序推进，并把每一步的结果写回 `result`。 */
async function warmUp(deps: WarmupDeps, result: WarmupResult): Promise<void> {
  const now = deps.now ?? (() => Date.now());

  // 1. 账号 store 就绪。`fetchAccounts` 由调用方注入并负责复用 store 的在途请求，
  //    这里只 await 一次，绝不重复发起账号请求。
  const accountsInflight = deps.hasAccountsInflight?.() === true;
  try {
    await deps.fetchAccounts();
  } catch (cause) {
    // 账号拉取失败不阻塞预热：先用 store 里已有的账号继续（可能为空）。
    console.info("[stats-warmup] accounts failed", describeError(cause), { accountsInflight });
  }
  const accountCount = deps.getAccounts().length;

  // 2. 本地积分投影：进入积分页时先有数字，官方用量稍后回填。
  const creditCached = readCreditStatisticsCache() !== null;
  try {
    await deps.loadCredit(false);
    result.credit = creditCached ? "hit" : "refreshed";
  } catch (cause) {
    console.info("[stats-warmup] credit failed", describeError(cause));
    result.credit = "skipped";
  }

  // 3. 第 2 步完成后启动 Token（不 await）：与官方用量门控并行，避免串行拉长预热时间。
  const tokenPromise = startTokenWarmup(deps, isTokenStatisticsCacheFresh(now()));

  // 4. 官方用量门控 + 回填。
  result.official = await warmOfficialUsage(deps, accountCount, now);

  // 5. 收尾 Token：官方流程结束后才等它，两个请求确实是并行发出的。
  result.token = await tokenPromise;
}

/** 启动 Token 预热（返回结果 Promise，从不 reject）。 */
function startTokenWarmup(deps: WarmupDeps, fresh: boolean): Promise<WarmupTokenState> {
  if (fresh) return Promise.resolve<WarmupTokenState>("hit");
  return deps.loadToken().then(
    (): WarmupTokenState => "loaded",
    (cause: unknown): WarmupTokenState => {
      // 失败静默：缓存不会被写入，页面下次进入会自己重试。
      console.info("[stats-warmup] token failed", describeError(cause));
      return "failed";
    },
  );
}

/**
 * 官方用量门控：只在「有账号且采集时刻缺失/过期」时补采一次，补采后回读积分做回填。
 *
 * @param accountCount 账号数（0 → 跳过，本地投影与 Token 预热照常）
 * @param now 时间源
 */
async function warmOfficialUsage(
  deps: WarmupDeps,
  accountCount: number,
  now: () => number,
): Promise<WarmupOfficialState> {
  if (accountCount === 0) return "skipped";

  const collectedAt = deps.getOfficialCollectedAt();
  const stale = collectedAt === null ? true : now() - collectedAt >= OFFICIAL_USAGE_TTL_MS;
  if (!stale) return "fresh";

  try {
    await deps.refreshOfficialUsage();
    // 服务端已写入新采集结果；丢掉客户端缓存值再回读一次，避免把旧 officialUsage 交给页面。
    invalidateCreditStatisticsCache();
    await deps.loadCredit(false);
    return "refreshed";
  } catch (cause) {
    console.info("[stats-warmup] official usage failed", describeError(cause));
    return "skipped";
  }
}
