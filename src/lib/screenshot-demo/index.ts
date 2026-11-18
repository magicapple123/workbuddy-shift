import type {
  AppStatus,
  AutoRotateConfig,
  CodeBuddyCliStatus,
  GithubConfig,
  RotateStatus,
  SessionGroupClient,
  SessionGroupDetail,
} from "../types";
import { accountVariant } from "../variant";

import {
  accountsDemoResponse,
  ACCOUNT_DEMO_COMMANDS,
  atLocalTime,
  getDemoActiveCliAccountId,
  hydratedAccounts,
} from "./accounts";
import type { DemoContext } from "./accounts";
import { creditsDemoResponse, CREDIT_DEMO_COMMANDS } from "./credits";
import { buildDemoSessionGroups, sessionsDemoResponse, SESSION_DEMO_COMMANDS } from "./sessions";
import { storageDemoResponse, STORAGE_DEMO_COMMANDS } from "./storage";
import { rotateConfig, settingsDemoResponse, SETTINGS_DEMO_COMMANDS } from "./settings";

/** 演示模式支持的全部只读命令（各域清单合并，供一致性测试与白名单核对）。 */
export const SCREENSHOT_DEMO_COMMANDS: readonly string[] = [
  ...ACCOUNT_DEMO_COMMANDS,
  ...SESSION_DEMO_COMMANDS,
  ...CREDIT_DEMO_COMMANDS,
  ...SETTINGS_DEMO_COMMANDS,
  ...STORAGE_DEMO_COMMANDS,
];

/** 组装一次请求的演示上下文：账号快照、各客户端状态与按客户端生成的会话组。 */
function buildDemoContext(args?: Record<string, unknown>): DemoContext {
  const demoAccounts = hydratedAccounts();
  const appStatus: AppStatus = { running: true, authFile: "/demo/workbuddy/auth.json", current: { uid: demoAccounts[0].uid, nickname: demoAccounts[0].nickname, email: demoAccounts[0].email }, appPath: "/demo/WorkBuddy.app", version: "0.1.24" };
  const activeIndex = Math.max(0, demoAccounts.findIndex((account) => account.id === getDemoActiveCliAccountId()));
  const activeAccount = demoAccounts[activeIndex] ?? demoAccounts[0];
  const cliStatus: CodeBuddyCliStatus = { configured: true, settingsPresent: true, helperPresent: true, helperSupportsAccountIds: true, activeIndex, activeAccountId: activeAccount.id, activeAccountName: activeAccount.nickname, activeAccountVariant: accountVariant(activeAccount), accountCount: demoAccounts.length, statePath: "/demo/codebuddy-cli-state.json" };
  const config: AutoRotateConfig = rotateConfig();
  const rotateStatus: RotateStatus = { config, cliConfigured: true, activeAccountId: demoAccounts[0].id, activeAccountName: demoAccounts[0].nickname, lastCheckAt: atLocalTime(0, 9, 30), lastSwitchAt: atLocalTime(1, 16, 20) };
  const githubConfig: GithubConfig = { owner: "magicapple123", repo: "workbuddy-shift", proxy: "" };
  const groupClient = args?.client as SessionGroupClient | undefined;
  const groupScope = args?.variantScope === "ai" ? "ai" : args?.variantScope === "cn" ? "cn" : null;
  const demoGroups: SessionGroupDetail[] = groupClient ? buildDemoSessionGroups(groupClient, groupScope, demoAccounts) : [];
  return { demoAccounts, appStatus, activeIndex, cliStatus, config, rotateStatus, githubConfig, groupClient, groupScope, demoGroups };
}

/**
 * Read-only demo response provider. It never reads or mutates real user data.
 *
 * 按域拆分为 accounts / sessions / credits / settings 四个 dispatcher；未命中任何域时保持原来的
 * 明确报错，避免演示页因为缺少分支静默退化。
 */
export function screenshotDemoResponse(command: string, args?: Record<string, unknown>): unknown {
  const ctx = buildDemoContext(args);
  const handlers = [accountsDemoResponse, sessionsDemoResponse, creditsDemoResponse, settingsDemoResponse, storageDemoResponse];
  for (const handler of handlers) {
    const result = handler(command, ctx, args);
    if (result !== undefined) return result;
  }
  throw new Error(`演示模式缺少只读数据: ${command}`);
}
