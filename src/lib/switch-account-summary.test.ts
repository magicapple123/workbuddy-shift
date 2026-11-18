import { describe, expect, it } from "vitest";

import { switchAccountSummary } from "./switch-account-summary";

/**
 * 切号弹窗底部摘要：三个弹窗共用同一份文案规则，这里锁定「复制 / 同步 / 覆盖」
 * 与「未选择说明」的组合行为，避免抽取重构时文案漂移。
 */

describe("switchAccountSummary", () => {
  it("同时勾选复制与同步", () => {
    expect(switchAccountSummary({ copyCount: 2, syncCount: 3, overwriteCount: 0 })).toEqual({
      main: "将复制 2 个、同步 3 个关联会话",
      sub: null,
    });
  });

  it("只勾选复制或只勾选同步", () => {
    expect(switchAccountSummary({ copyCount: 2, syncCount: 0, overwriteCount: 0 })).toEqual({
      main: "将复制 2 个会话",
      sub: "未选择同步会话",
    });
    expect(switchAccountSummary({ copyCount: 0, syncCount: 3, overwriteCount: 0 })).toEqual({
      main: "将同步 3 个关联会话",
      sub: "未选择复制会话",
    });
  });

  it("都没勾选时给出「仅切换账号」与未选择说明", () => {
    expect(switchAccountSummary({ copyCount: 0, syncCount: 0, overwriteCount: 0 })).toEqual({
      main: "本次仅切换账号",
      sub: "未选择复制或同步会话",
    });
  });

  it("覆盖项的次摘要优先于未选择说明", () => {
    expect(switchAccountSummary({ copyCount: 1, syncCount: 2, overwriteCount: 1 })).toEqual({
      main: "将复制 1 个、同步 2 个关联会话",
      sub: "其中 1 个会替换目标账号的完整内容",
    });
  });

  it("idleHint 只替换「未选择复制或同步会话」一句，不影响其它分支", () => {
    const hint = "未选择复制或同步会话，确认后将重启 IDE";
    expect(switchAccountSummary({ copyCount: 0, syncCount: 0, overwriteCount: 0, idleHint: hint })).toEqual({
      main: "本次仅切换账号",
      sub: hint,
    });
    expect(switchAccountSummary({ copyCount: 1, syncCount: 0, overwriteCount: 0, idleHint: hint }).sub).toBe(
      "未选择同步会话",
    );
    expect(switchAccountSummary({ copyCount: 0, syncCount: 1, overwriteCount: 1, idleHint: hint }).sub).toBe(
      "其中 1 个会替换目标账号的完整内容",
    );
  });
});
