import { useCallback, useState } from "react";

import { runDailyActivity } from "@/lib/api";

import {
  EMPTY_DAILY_ACTIVITY_STATE,
  runDailyActivityAttempt,
  type DailyActivityState,
} from "../daily-activity";

/** 每日活跃面板的控制器：确认框开关 + 结果状态 + 四个动作。 */
export interface DailyActivityController {
  /** 进行中 / 最近结果 / 最近失败原因。 */
  state: DailyActivityState;
  /** 二次确认框是否打开。 */
  confirmOpen: boolean;
  /** 工具栏按钮：只打开确认框，**不发任何请求**。 */
  request: () => void;
  /** 确认框「取消」：关闭，不发请求。 */
  cancel: () => void;
  /** 确认框「开始」：唯一会发请求的动作。 */
  confirm: () => void;
  /** 关闭结果面板（进行中不允许关闭，否则用户看不到也不会拿到结果）。 */
  dismiss: () => void;
}

/**
 * 账号页「每日活跃」的装配层。
 *
 * 只有 `confirm()` 会发请求，而它只挂在确认框的「开始」按钮上：`request()`（工具栏
 * 按钮）永远只开确认框。请求本身走 `runDailyActivityAttempt` 的纯状态机，因此
 * 「未确认不发请求」「失败不清空已有结果」都有单测锁着（`../daily-activity.test.ts`）。
 */
export function useDailyActivity(): DailyActivityController {
  const [state, setState] = useState<DailyActivityState>(EMPTY_DAILY_ACTIVITY_STATE);
  const [confirmOpen, setConfirmOpen] = useState(false);

  const request = useCallback(() => setConfirmOpen(true), []);
  const cancel = useCallback(() => setConfirmOpen(false), []);

  const confirm = useCallback(() => {
    setConfirmOpen(false);
    void runDailyActivityAttempt({
      state,
      confirmed: true,
      runner: runDailyActivity,
      publish: setState,
    });
  }, [state]);

  const dismiss = useCallback(() => {
    setState((prev) => (prev.running ? prev : EMPTY_DAILY_ACTIVITY_STATE));
  }, []);

  return { state, confirmOpen, request, cancel, confirm, dismiss };
}
