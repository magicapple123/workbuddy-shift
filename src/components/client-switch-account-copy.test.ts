import { describe, expect, it } from "vitest";

import {
  busyCopy,
  emptyStateHint,
  notLoggedInHint,
  statusNoticeFor,
} from "./client-switch-account-copy";

/**
 * 客户端切号弹窗抽出的纯文案逻辑：状态三态、空态、忙碌遮罩。
 *
 * 这些文案是用户能直接看到的差异点（VS Code / CodeBuddy IDE 各自不同），
 * 抽取时逐字保留，这里锁定分支与文案，防止后续重构漂移。
 */

describe("statusNoticeFor", () => {
  it("VS Code：运行中且开启自动重开时提示将自动关闭并重开", () => {
    const notice = statusNoticeFor("vscode", true, true);
    expect(notice.title).toBe("将自动关闭并重开 VS Code");
    expect(notice.warning).toBe(false);
  });

  it("VS Code：运行中但关闭自动重开时给出警告", () => {
    const notice = statusNoticeFor("vscode", true, false);
    expect(notice.title).toBe("请先完全退出 VS Code");
    expect(notice.warning).toBe(true);
  });

  it("VS Code：未运行时可直接切换", () => {
    expect(statusNoticeFor("vscode", false, true).title).toBe("VS Code 未运行，可直接切换");
  });

  it("CodeBuddy IDE：运行中/未运行两态，均非警告", () => {
    expect(statusNoticeFor("codebuddy-ide", true, true).title).toBe("将自动关闭并重开 CodeBuddy IDE");
    const idle = statusNoticeFor("codebuddy-ide", false, true);
    expect(idle.title).toBe("CodeBuddy IDE 未运行，可直接切换");
    expect(idle.warning).toBe(false);
  });
});

describe("notLoggedInHint", () => {
  it("按客户端给出对应的未登录说明", () => {
    expect(notLoggedInHint("vscode")).toContain("VS Code");
    expect(notLoggedInHint("codebuddy-ide")).toContain("CodeBuddy IDE");
  });
});

describe("emptyStateHint", () => {
  const vscodeReady = { installed: true, extensionInstalled: true };

  it("加载中优先于其它状态", () => {
    expect(emptyStateHint("vscode", undefined, true, null, undefined, false)).toBe("正在加载会话…");
  });

  it("VS Code：依次区分未安装 / 未装扩展 / 无数据目录 / 未登录 / 无会话 / 可复制", () => {
    expect(emptyStateHint("vscode", { installed: false }, false, null, undefined, false)).toContain("未检测到 VS Code");
    expect(
      emptyStateHint("vscode", { installed: true, extensionInstalled: false }, false, null, undefined, false),
    ).toContain("未安装 VS Code CodeBuddy 插件");
    expect(emptyStateHint("vscode", vscodeReady, false, "u1", null, true)).toContain("数据目录");
    expect(emptyStateHint("vscode", vscodeReady, false, null, "/data", true)).toContain("当前登录账号");
    expect(emptyStateHint("vscode", vscodeReady, false, "u1", "/data", false)).toContain("暂无可复制的会话");
    expect(emptyStateHint("vscode", vscodeReady, false, "u1", "/data", true)).toContain("以新 id 复制");
  });

  it("CodeBuddy IDE：没有扩展概念，未安装文案与 VS Code 不同", () => {
    expect(emptyStateHint("codebuddy-ide", { installed: false }, false, null, undefined, false)).toBe(
      "未检测到 CodeBuddy IDE，请先安装并登录",
    );
    expect(emptyStateHint("codebuddy-ide", { installed: true }, false, "u1", null, true)).toContain(
      "未找到 CodeBuddy IDE 数据目录",
    );
    expect(emptyStateHint("codebuddy-ide", { installed: true }, false, "u1", "/data", true)).toContain(
      "沿用会话 id",
    );
  });
});

describe("busyCopy", () => {
  it("VS Code：区分自动重开 / 复制 / 同步 / 纯切换", () => {
    expect(busyCopy("vscode", { autoClose: true, copyCount: 2, syncCount: 3 }).title).toBe(
      "正在关闭 VS Code 并写入凭证…",
    );
    expect(busyCopy("vscode", { autoClose: false, copyCount: 2, syncCount: 3 }).title).toBe(
      "正在切换并复制会话…",
    );
    expect(busyCopy("vscode", { autoClose: false, copyCount: 0, syncCount: 3 }).title).toBe(
      "正在切换并同步会话…",
    );
    const plain = busyCopy("vscode", { autoClose: false, copyCount: 0, syncCount: 0 });
    expect(plain.title).toBe("正在切换账号…");
    expect(plain.hint).toBe("正在处理中，请勿关闭窗口");
  });

  it("CodeBuddy IDE：固定关闭并重开，按动作给出主文案", () => {
    expect(busyCopy("codebuddy-ide", { autoClose: false, copyCount: 2, syncCount: 0 }).title).toBe(
      "正在关闭 CodeBuddy IDE 并复制会话…",
    );
    expect(busyCopy("codebuddy-ide", { autoClose: false, copyCount: 0, syncCount: 3 }).title).toBe(
      "正在关闭 CodeBuddy IDE 并同步会话…",
    );
    expect(busyCopy("codebuddy-ide", { autoClose: false, copyCount: 0, syncCount: 0 })).toEqual({
      title: "正在关闭 CodeBuddy IDE 并写入凭证…",
      hint: "若 IDE 弹出保存提示请先处理（最长等待 60 秒）",
    });
  });
});
