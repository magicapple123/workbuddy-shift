/**
 * 限额台账拉取的节流判定（纯函数，供 hook 使用并在 node 环境单测）。
 *
 * 两个常量语义不同，不能混用：
 * - `RATE_LIMIT_REFRESH_INTERVAL_MS`：决定「台账多久算旧」的刷新间隔。后端命中缓存时
 *   该接口廉价（真正的日志扫描由后端自己按 5 分钟节流），所以前端取 1 分钟。
 * - `RATE_LIMIT_FETCH_FLOOR_MS`：只做防抖，拦下可见性抖动 / 重挂载 / 事件风暴造成的
 *   重复请求，**不**参与「要不要显示」的判断。
 */

/** 台账刷新间隔（1 分钟）。 */
export const RATE_LIMIT_REFRESH_INTERVAL_MS = 60 * 1000;

/** 两次拉取之间的硬下限（20 秒），只用于防抖。 */
export const RATE_LIMIT_FETCH_FLOOR_MS = 20 * 1000;

/**
 * 是否应该发起一次限额台账拉取。
 *
 * 节流基准必须是「上次**成功**拉取的时刻」（前端自己记的 `Date.now()`），不能用
 * payload 的 `scannedAt`：那是后端上次真正扫描日志的时刻，可能已经过去几分钟，
 * 用它做前端节流会让进入页面时拿不到台账（chip 要等下一轮才出现）。
 *
 * @param now 当前时刻（毫秒）。
 * @param lastFetchAt 上次成功拉取的时刻（毫秒）；`<= 0` 表示本进程还没成功拉过。
 * @param force 事件推送路径：忽略一切节流。
 */
export function shouldFetchRateLimits(
  now: number,
  lastFetchAt: number,
  force = false,
): boolean {
  if (force) return true;
  // 首次拉取：进页面就该拿到台账，不能等到下一个间隔。
  if (lastFetchAt <= 0) return true;
  const elapsed = now - lastFetchAt;
  // 防抖下限：短时间内重复触发（可见性抖动 / 重挂载 / 事件风暴）一律拦下。
  if (elapsed < RATE_LIMIT_FETCH_FLOOR_MS) return false;
  // 真正的刷新间隔：过了这个时长才重新拉取。
  return elapsed >= RATE_LIMIT_REFRESH_INTERVAL_MS;
}
