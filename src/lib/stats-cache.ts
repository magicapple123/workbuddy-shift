/**
 * 统计数据的进程内 single-flight + 成功结果缓存。
 *
 * 背景：Token 统计冷启动首次调用要全量扫 JSONL（本机约 839 MB），积分统计的官方用量
 * 在无磁盘缓存时也会走网络。启动预热与页面挂载、多标签页会几乎同时请求同一份数据，
 * 没有合并就会出现两次真实扫描 / 两次官方采集。
 *
 * 语义（与交付规格 4.3 的 T2.1 一致）：
 * - 同一 key 在途时，后来者 join **同一个** Promise，绝不重复请求（`force` 也一样）；
 * - 只缓存**成功**结果，并记录写入时刻 `at = Date.now()`；失败时清空在途、
 *   不写缓存、错误原样抛给调用方（页面据此展示重试入口）；
 * - 不改变后端响应形状，不新增接口。
 *
 * 依赖方向：`src/lib/*.ts` 纯逻辑层，只依赖 `@/lib/api` 与 `@/lib/types`，不引入 React。
 */

import * as api from "@/lib/api";
import type { CreditStatistics, TokenStatistics } from "@/lib/types";

/**
 * Token 统计的新鲜窗口：10 分钟内页面直接渲染、不再重扫（验收 4.1-2）。
 *
 * 判据是响应自带的 `generatedAt`（不是写入缓存的时刻），因此页面与启动预热对
 * 「这份数据算不算新鲜」永远只有一个答案。
 */
export const TOKEN_CACHE_TTL_MS = 10 * 60 * 1000;

/** 缓存项：`at` 是写入缓存的时刻（毫秒），便于页面判断数据新鲜度。 */
interface CacheEntry<T> {
  value: T;
  at: number;
}

/** Token 统计没有入参（只有一个 key），因此只保留一份在途请求。 */
let tokenEntry: CacheEntry<TokenStatistics> | null = null;
let tokenInflight: Promise<TokenStatistics> | null = null;

/**
 * 积分统计的 key 是 `refresh`：`refresh=true` 会重新采集官方用量，
 * 与 `refresh=false`（允许读服务端缓存）语义不同，两者**不得**互相 join，
 * 否则会把「没有重新采集」的结果当成重新采集的结果交回页面。
 */
let creditEntry: CacheEntry<CreditStatistics> | null = null;
const creditInflight = new Map<boolean, Promise<CreditStatistics>>();

/** 当前缓存里的 Token 统计（无缓存返回 null）；页面用它在首帧直接渲染。 */
export function readTokenStatisticsCache(): TokenStatistics | null {
  return tokenEntry === null ? null : tokenEntry.value;
}

/**
 * Token 缓存是否仍在新鲜窗口内（默认 10 分钟）。
 *
 * @param now 当前时刻（毫秒），单测注入；缺省 `Date.now()`。
 */
export function isTokenStatisticsCacheFresh(now = Date.now()): boolean {
  if (tokenEntry === null) return false;
  return now - tokenEntry.value.generatedAt < TOKEN_CACHE_TTL_MS;
}

/**
 * 读取 Token 统计，同一时刻只允许一次真实扫描。
 *
 * @param opts.force 忽略已缓存值重新请求；若已有在途请求则仍然 join 它。
 */
export function loadTokenStatistics(opts?: { force?: boolean }): Promise<TokenStatistics> {
  const force = opts?.force === true;
  // 有在途请求就 join：`force` 表示「不要拿旧缓存」，不代表「再扫一遍盘」。
  if (tokenInflight !== null) return tokenInflight;
  if (!force && tokenEntry !== null) return Promise.resolve(tokenEntry.value);

  const request = api.getTokenStatistics().then((value) => {
    tokenEntry = { value, at: Date.now() };
    return value;
  });
  tokenInflight = request;
  // 失败路径只做清理、不吞错误：调用方拿到的仍是原始 rejection，下次调用可以重试。
  const settle = (): void => {
    if (tokenInflight === request) tokenInflight = null;
  };
  request.then(settle, settle);
  return request;
}

/** 当前缓存里的积分统计（无缓存返回 null）。 */
export function readCreditStatisticsCache(): CreditStatistics | null {
  return creditEntry === null ? null : creditEntry.value;
}

/**
 * 读取积分统计。
 *
 * @param refresh `true` 要求重新采集官方用量（与页面的「刷新统计」一致）；
 *                缺省/`false` 时优先返回已缓存结果，其次走服务端允许的只读查询。
 */
export function loadCreditStatistics(refresh = false): Promise<CreditStatistics> {
  const inflight = creditInflight.get(refresh);
  if (inflight !== undefined) return inflight;
  if (!refresh && creditEntry !== null) return Promise.resolve(creditEntry.value);

  const request = api.getCreditStatistics(refresh).then((value) => {
    creditEntry = { value, at: Date.now() };
    return value;
  });
  creditInflight.set(refresh, request);
  const settle = (): void => {
    if (creditInflight.get(refresh) === request) creditInflight.delete(refresh);
  };
  request.then(settle, settle);
  return request;
}

/**
 * 丢弃本地缓存值（在途请求不受影响）。
 *
 * 预热里 `refreshOfficialUsage()` 成功后服务端已经写入新的官方用量，此时必须让
 * 下一次 `loadCreditStatistics(false)` 真正回读服务端结果，而不是把几秒前的旧值
 * 直接交回页面。只清缓存值、不触发请求，也不改 `refresh` 语义。
 */
export function invalidateCreditStatisticsCache(): void {
  creditEntry = null;
}

/**
 * 清空全部缓存与在途状态。**仅供单测**使用（命名带 `__` 前缀，页面不得调用）：
 * 否则用例之间会互相看到对方写入的缓存。
 */
export function __resetStatsCacheForTests(): void {
  tokenEntry = null;
  tokenInflight = null;
  creditEntry = null;
  creditInflight.clear();
}
