import { describe, expect, it } from "vitest";

import { restartDialogCopy, restartPrecheckClient } from "./restart";

/**
 * 「按客户端分流」是第 10 条最容易回退的地方：把 vsCodeExt 与 jetbrains 合并成一条分支会
 * 让 JetBrains 走错预检（漏关 IDE 或白关一次），且不会报错、只会「切了没变」。
 */
describe("restartPrecheckClient", () => {
  it("两个插件型客户端各自有预检，其余客户端没有", () => {
    expect(restartPrecheckClient("vscodeExt")).toBe("vscodeExt");
    expect(restartPrecheckClient("jetbrains")).toBe("jetbrains");
    expect(restartPrecheckClient("workbuddy")).toBeNull();
    expect(restartPrecheckClient("codebuddyIde")).toBeNull();
  });
});

describe("restartDialogCopy", () => {
  it("JetBrains 的文案写明会关闭 IDEA / PyCharm 并自动重开", () => {
    const copy = restartDialogCopy("jetbrains");
    expect(copy).not.toBeNull();
    expect(copy?.title).toContain("JetBrains");
    expect(copy?.description).toContain("IDEA / PyCharm");
    expect(copy?.description).toContain("重新打开");
    // 不得复述 VS Code 的说法，否则用户会去关错客户端。
    expect(copy?.description).not.toContain("VS Code");
    expect(copy?.busyText).toContain("JetBrains");
  });

  it("VS Code 的文案逐字保持既有说法", () => {
    const copy = restartDialogCopy("vscodeExt");
    expect(copy?.title).toBe("同步前需要重启 VS Code");
    expect(copy?.busyText).toBe("正在关闭 VS Code 并处理，请勿关闭本窗口…");
  });

  it("没有宿主进程的客户端不产生确认框文案", () => {
    expect(restartDialogCopy("workbuddy")).toBeNull();
    expect(restartDialogCopy("codebuddyIde")).toBeNull();
  });
});
