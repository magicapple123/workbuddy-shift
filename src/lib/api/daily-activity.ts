import type { DailyActivityRunResult } from "../types";
import { call } from "./core";

/**
 * 手动跑一轮「国际版每日活跃会话」（issue #214）。
 *
 * # 唯一入口
 *
 * 只有账号页工具栏的「每日活跃」按钮会调用它，且必须先过二次确认。**没有任何自动
 * 触发路径**：不加定时器、不在启动/切号/签到后自动调用（core 模块也只在被显式调用时
 * 才发请求）。
 *
 * # 参数
 *
 * 不下发任何参数：账号由后端从账号库读取。允许调用方指定子集就能绕过「同一账号一天
 * 只做一次」的去重（后端 `daily_activity_state.json`），重复消耗额度。
 *
 * # 返回
 *
 * 逐账号结果：成功（含实际用的模型）/ 今日已跳过 / 不支持（国内版或缺凭据）/ 失败
 * （含脱敏原因）。单账号失败不影响其它账号，整体错误只在「连 HTTP 客户端都构造不出来」
 * 时抛出。
 */
export function runDailyActivity(): Promise<DailyActivityRunResult> {
  return call("run_daily_activity");
}
