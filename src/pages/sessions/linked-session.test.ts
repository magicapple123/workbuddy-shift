import { describe, expect, it } from "vitest";
import type { AccountMeta, SessionGroupClient } from "@/lib/types";
import {
  linkedSessionAccountLabel,
  linkedSessionCopyFailureToast,
  linkedSessionCopyKind,
  linkedSessionEditorLabel,
  linkedSessionEmptySessionsHint,
  linkedSessionNeedsRestartPrecheck,
  linkedSessionTargetHint,
  linkedSessionTargetOptions,
  supportsAddLinkedSession,
} from "./linked-session";

const ALL_CLIENTS: SessionGroupClient[] = ["workbuddy", "codebuddyIde", "vscodeExt", "jetbrains"];

/** 只要 id / variant 参与判定的最小账号（其余字段与本次逻辑无关）。 */
function account(id: string, variant?: "cn" | "ai"): AccountMeta {
  return { id, uid: `uid-${id}`, variant } as AccountMeta;
}

describe("客户端分派", () => {
  it("四个客户端都支持新增关联会话（#195：CodeBuddy IDE 不得再被白名单排除）", () => {
    for (const client of ALL_CLIENTS) {
      expect(supportsAddLinkedSession(client), client).toBe(true);
      expect(linkedSessionCopyKind(client), client).not.toBeNull();
    }
  });

  it("按客户端分派到三个后端入口", () => {
    expect(linkedSessionCopyKind("workbuddy")).toBe("workbuddy");
    expect(linkedSessionCopyKind("codebuddyIde")).toBe("codebuddyIde");
    expect(linkedSessionCopyKind("vscodeExt")).toBe("editorPlugin");
    expect(linkedSessionCopyKind("jetbrains")).toBe("editorPlugin");
  });

  it("只有两个插件型客户端需要「关闭并重开宿主」预检", () => {
    expect(linkedSessionNeedsRestartPrecheck("vscodeExt")).toBe(true);
    expect(linkedSessionNeedsRestartPrecheck("jetbrains")).toBe(true);
    // WorkBuddy 是自己重开的应用；CodeBuddy IDE 走「先退出再复制」，都不弹确认框。
    expect(linkedSessionNeedsRestartPrecheck("workbuddy")).toBe(false);
    expect(linkedSessionNeedsRestartPrecheck("codebuddyIde")).toBe(false);
  });

  it("宿主名与账号称呼按客户端取词，不串用", () => {
    expect(linkedSessionEditorLabel("workbuddy")).toBe("WorkBuddy");
    expect(linkedSessionEditorLabel("codebuddyIde")).toBe("CodeBuddy IDE");
    expect(linkedSessionEditorLabel("vscodeExt")).toBe("VS Code");
    expect(linkedSessionEditorLabel("jetbrains")).toBe("JetBrains IDE");

    expect(linkedSessionAccountLabel("workbuddy")).toBe("WorkBuddy 账号");
    expect(linkedSessionAccountLabel("codebuddyIde")).toBe("CodeBuddy IDE 账号");
    expect(linkedSessionAccountLabel("vscodeExt")).toBe("CodeBuddy 插件账号");
    // 插件版与 IDE 版的称呼必须区分：同一个人在两处看到「CodeBuddy 插件账号」会以为走错了客户端。
    expect(linkedSessionAccountLabel("codebuddyIde")).not.toBe(linkedSessionAccountLabel("vscodeExt"));
  });
});

describe("目标账号候选", () => {
  it("排除来源账号", () => {
    const accounts = [account("a"), account("b")];
    expect(
      linkedSessionTargetOptions(accounts, { client: "workbuddy", sourceAccountId: "a" }).map((item) => item.id),
    ).toEqual(["b"]);
  });

  it("WorkBuddy 跨档复制是能力：不按档位收窄", () => {
    const accounts = [account("cn-1", "cn"), account("ai-1", "ai")];
    expect(
      linkedSessionTargetOptions(accounts, { client: "workbuddy", sourceAccountId: "cn-1", variantScope: "cn" }).map(
        (item) => item.id,
      ),
    ).toEqual(["ai-1"]);
    // 插件侧存储与地区无关，同样不按档位收窄。
    expect(
      linkedSessionTargetOptions(accounts, { client: "vscodeExt", sourceAccountId: "cn-1", variantScope: "cn" }).map(
        (item) => item.id,
      ),
    ).toEqual(["ai-1"]);
  });

  it("CodeBuddy IDE 只列同档位账号（跨档登记会建到另一个档位的组里）", () => {
    const accounts = [account("src", "cn"), account("cn-2", "cn"), account("ai-2", "ai")];
    expect(
      linkedSessionTargetOptions(accounts, { client: "codebuddyIde", sourceAccountId: "src", variantScope: "cn" }).map(
        (item) => item.id,
      ),
    ).toEqual(["cn-2"]);
    // 缺省档位字段的账号按国内版处理（与 `accountVariant` 同一口径）。
    const legacy = [account("src", "cn"), account("legacy")];
    expect(
      linkedSessionTargetOptions(legacy, { client: "codebuddyIde", sourceAccountId: "src", variantScope: "cn" }).map(
        (item) => item.id,
      ),
    ).toEqual(["legacy"]);
  });

  it("未选来源账号时不做任何排除以外的过滤", () => {
    const accounts = [account("a", "cn"), account("b", "ai")];
    expect(
      linkedSessionTargetOptions(accounts, { client: "codebuddyIde", sourceAccountId: "" }).map((item) => item.id),
    ).toEqual(["a", "b"]);
  });
});

describe("失败与空态文案", () => {
  it("没有可用目标账号时给出原因（选择器此刻是禁用的）", () => {
    expect(
      linkedSessionTargetHint({ client: "workbuddy", sourceAccountId: "", targetCount: 2 }),
    ).toBeNull();
    expect(linkedSessionTargetHint({ client: "workbuddy", sourceAccountId: "a", targetCount: 1 })).toBeNull();

    // 通用原因：账号库里只有这一个账号。
    expect(linkedSessionTargetHint({ client: "workbuddy", sourceAccountId: "a", targetCount: 0 })).toContain(
      "没有另一个账号",
    );
    // IDE 版必须说清是「没有另一个同档位账号」——否则用户会以为账号库丢了账号。
    const ideHint = linkedSessionTargetHint({
      client: "codebuddyIde",
      sourceAccountId: "a",
      targetCount: 0,
      variantScope: "ai",
    });
    expect(ideHint).toContain("国际版");
    expect(ideHint).toContain("没有另一个");
  });

  it("「未找到数据目录」与「该账号没有会话」必须区分", () => {
    for (const client of ["vscodeExt", "jetbrains", "codebuddyIde"] as SessionGroupClient[]) {
      const missing = linkedSessionEmptySessionsHint({ client, dataRoot: null });
      expect(missing, client).toContain("未找到");
      expect(missing, client).toContain("登录");
      // 字段缺省（undefined）不等于「未找到目录」：按「没有会话」处理，避免误导。
      expect(linkedSessionEmptySessionsHint({ client, dataRoot: undefined }), client).toBe(
        "该账号没有可复制的会话。",
      );
    }
    expect(linkedSessionEmptySessionsHint({ client: "codebuddyIde", dataRoot: null })).toContain("CodeBuddy IDE");
    expect(linkedSessionEmptySessionsHint({ client: "jetbrains", dataRoot: null })).toContain("IDEA");
    expect(linkedSessionEmptySessionsHint({ client: "workbuddy", dataRoot: null })).toBe("该账号没有可复制的会话。");
  });

  it("复制失败时把后端原因带给用户，没有原因也不编造", () => {
    expect(linkedSessionCopyFailureToast("检测到 CodeBuddy IDE 正在运行")).toEqual({
      title: "复制并关联失败",
      description: "检测到 CodeBuddy IDE 正在运行",
    });
    expect(linkedSessionCopyFailureToast("   ")).toEqual({ title: "复制并关联失败" });
  });
});
