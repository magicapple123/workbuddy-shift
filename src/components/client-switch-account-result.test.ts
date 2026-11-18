import { beforeEach, describe, expect, it, vi } from "vitest";

import type { SessionLinkPreviewGroup, VscodeSessionCopyResult } from "@/lib/types";

import { reportClientSwitchResult } from "./client-switch-account-result";

const toastMock = vi.hoisted(() => ({
  success: vi.fn(),
  warning: vi.fn(),
  error: vi.fn(),
  info: vi.fn(),
}));

vi.mock("sonner", () => ({ toast: toastMock }));

/** 组标题回显只需 groupId / title，其余字段不参与提示逻辑。 */
function group(groupId: string, title: string): SessionLinkPreviewGroup {
  return { groupId, title } as SessionLinkPreviewGroup;
}

function copied(count: number): VscodeSessionCopyResult {
  return {
    sourceUid: null,
    targetUid: "target",
    copied: Array.from({ length: count }, (_, index) => ({
      workspaceHash: "w",
      oldId: `old-${index}`,
      newId: `new-${index}`,
      messages: 1,
    })),
  };
}

describe("reportClientSwitchResult", () => {
  beforeEach(() => {
    for (const fn of Object.values(toastMock)) fn.mockReset();
  });

  it("成功：带上复制数量与 VS Code 生效说明", () => {
    reportClientSwitchResult({
      client: "vscode",
      nickname: "账号甲",
      result: { restarted: true, sessionCopy: copied(2) },
      syncGroups: [],
      requestedSync: false,
    });
    expect(toastMock.success).toHaveBeenCalledTimes(1);
    const [title, options] = toastMock.success.mock.calls[0] as [string, { description: string }];
    expect(title).toBe("已切换至「账号甲」");
    expect(options.description).toContain("已复制 2 个会话");
    expect(options.description).toContain("已切换并重新打开 VS Code");
    expect(toastMock.warning).not.toHaveBeenCalled();
    expect(toastMock.error).not.toHaveBeenCalled();
  });

  it("VS Code 重开失败时用后端 message 兜底", () => {
    reportClientSwitchResult({
      client: "vscode",
      nickname: "账号甲",
      result: { restarted: false, closedByUs: true, message: "重开失败：xxx" },
      syncGroups: [],
      requestedSync: false,
    });
    const [, options] = toastMock.success.mock.calls[0] as [string, { description: string }];
    expect(options.description).toContain("重开失败：xxx");
  });

  it("CodeBuddy IDE：生效说明取后端 message，缺省为已重启 IDE", () => {
    reportClientSwitchResult({
      client: "codebuddy-ide",
      nickname: "账号甲",
      result: {},
      syncGroups: [],
      requestedSync: false,
    });
    const [, options] = toastMock.success.mock.calls[0] as [string, { description: string }];
    expect(options.description).toBe("已重启 CodeBuddy IDE");
  });

  it("复制失败：警告标题与失败原因逐项列出", () => {
    reportClientSwitchResult({
      client: "vscode",
      nickname: "账号甲",
      result: {
        restarted: true,
        sessionCopy: {
          ...copied(0),
          errors: [{ workspaceHash: "w", conversationId: "c1", error: "写入失败" }],
        },
      },
      syncGroups: [],
      requestedSync: false,
    });
    expect(toastMock.warning.mock.calls[0][0]).toBe("已切换至「账号甲」，但部分会话未复制");
    expect((toastMock.warning.mock.calls[0][1] as { description: string }).description).toContain("写入失败");
  });

  it("复制成功但未建立关联：单独警告，不静默当成完整成功", () => {
    reportClientSwitchResult({
      client: "vscode",
      nickname: "账号甲",
      result: {
        restarted: true,
        sessionCopy: { ...copied(1), linkErrors: [{ conversationId: "c1", error: "登记失败" }] },
      },
      syncGroups: [],
      requestedSync: false,
    });
    expect(toastMock.warning).toHaveBeenCalledWith("部分会话已复制但未建立关联", {
      description: expect.stringContaining("c1：登记失败"),
    });
  });

  it("同步跳过：按组标题回显原因，归类为警告", () => {
    reportClientSwitchResult({
      client: "vscode",
      nickname: "账号甲",
      result: {
        restarted: true,
        sessionSync: {
          synced: [],
          skipped: [{ groupId: "g1", status: "skipped", reasonCode: "previewStale", message: "预览已过期", verdict: null }],
          errors: [],
        },
      },
      syncGroups: [group("g1", "组一")],
      requestedSync: true,
    });
    expect(toastMock.success).toHaveBeenCalledTimes(1);
    expect(toastMock.warning).toHaveBeenCalledWith("有关联会话没有同步（已跳过）", {
      description: expect.stringContaining("组一：预览已过期"),
    });
  });

  it("同步失败：归类为错误", () => {
    reportClientSwitchResult({
      client: "vscode",
      nickname: "账号甲",
      result: {
        restarted: true,
        sessionSync: { synced: [], skipped: [], errors: [{ groupId: "g1", error: "写入失败" }] },
      },
      syncGroups: [group("g1", "组一")],
      requestedSync: true,
    });
    expect(toastMock.error).toHaveBeenCalledWith("部分关联会话没有同步（失败或已跳过）", {
      description: expect.stringContaining("组一：写入失败"),
    });
  });

  it("请求了同步但没有同步报告：提示可能不支持同步", () => {
    reportClientSwitchResult({
      client: "vscode",
      nickname: "账号甲",
      result: { restarted: true },
      syncGroups: [],
      requestedSync: true,
    });
    expect(toastMock.warning).toHaveBeenCalledWith("关联会话未同步", {
      description: expect.stringContaining("没有收到同步结果"),
    });
  });
});
