import { describe, expect, it } from "vitest";

import { DEMO_READ_COMMANDS } from "./api/core";
import { SCREENSHOT_DEMO_COMMANDS } from "./screenshot-demo/index";

/**
 * 演示模式契约：只读白名单（api 层）与 screenshotDemoResponse 的分发分支必须一致。
 *
 * 通过两份导出清单对比而不是逐个调用命令：调用需要构造参数、缺 fixture 时会抛错，
 * 清单对比能在不触发运行时异常的前提下发现「白名单有、分支缺」或「分支有、白名单缺」。
 */
describe("demo 命令清单一致性", () => {
  const demoCommands = [...SCREENSHOT_DEMO_COMMANDS];
  const readCommands = [...DEMO_READ_COMMANDS];

  it("演示分支清单无重复", () => {
    expect(new Set(demoCommands).size).toBe(demoCommands.length);
  });

  it("只读白名单里的每条命令都有对应演示分支", () => {
    const missing = readCommands.filter((command) => !demoCommands.includes(command));
    expect(missing).toEqual([]);
  });

  it("演示分支里超出只读白名单的只能是演示写操作", () => {
    const extras = demoCommands.filter((command) => !DEMO_READ_COMMANDS.has(command)).sort();
    expect(extras).toEqual(["switch_codebuddy_cli_account", "update_account_display"]);
  });

  it("白名单覆盖账号 / 会话 / 积分 / 设置四个域", () => {
    for (const command of [
      "get_accounts",
      "list_session_groups",
      "get_credit_statistics",
      "get_token_statistics",
      "get_auto_rotate_config",
      "check_update",
    ]) {
      expect(DEMO_READ_COMMANDS.has(command)).toBe(true);
      expect(demoCommands).toContain(command);
    }
  });
});
