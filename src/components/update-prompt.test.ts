import { describe, expect, it } from "vitest";

import type { UpdateSnapshot } from "@/lib/types";

import {
  NOTES_MAX_CHARS,
  NOTES_PREVIEW_LINES,
  cleanNotes,
  notesNeedToggle,
  promptVersion,
  publishedLabel,
  shouldPrompt,
} from "./update-prompt";

function snapshot(patch: Partial<UpdateSnapshot> = {}): UpdateSnapshot {
  return {
    phase: "available",
    latest: "1.2.3",
    percent: null,
    message: null,
    notes: null,
    publishedAt: null,
    checkedAt: null,
    busyKind: null,
    busySince: null,
    manualInstallRequired: false,
    ...patch,
  };
}

describe("update prompt", () => {
  it("shouldPrompt：只在「有新版本」且未忽略时弹出", () => {
    const dismissed = new Set<string>();
    expect(shouldPrompt(snapshot(), dismissed)).toBe(true);
    expect(shouldPrompt(snapshot({ latest: " 1.2.3 " }), dismissed)).toBe(true);
    expect(shouldPrompt(snapshot({ latest: "v1.2.3" }), dismissed)).toBe(true);
  });

  it("shouldPrompt：同一版本在本次会话内「稍后」过就不再弹", () => {
    const dismissed = new Set(["1.2.3"]);
    expect(shouldPrompt(snapshot(), dismissed)).toBe(false);
    expect(shouldPrompt(snapshot({ latest: "v1.2.3" }), dismissed)).toBe(false);
    expect(shouldPrompt(snapshot({ latest: "1.2.4" }), dismissed)).toBe(true);
  });

  it("shouldPrompt：其他阶段 / 无目标版本 / 无快照都不弹", () => {
    const dismissed = new Set<string>();
    expect(shouldPrompt(null, dismissed)).toBe(false);
    expect(shouldPrompt(undefined, dismissed)).toBe(false);
    for (const phase of ["idle", "checking", "upToDate", "downloading", "readyToRestart", "error"] as const) {
      expect(shouldPrompt(snapshot({ phase }), dismissed)).toBe(false);
    }
    expect(shouldPrompt(snapshot({ latest: null }), dismissed)).toBe(false);
    expect(shouldPrompt(snapshot({ latest: "   " }), dismissed)).toBe(false);
  });

  it("promptVersion 去掉 v 前缀与空白", () => {
    expect(promptVersion(snapshot())).toBe("1.2.3");
    expect(promptVersion(snapshot({ latest: "V2.0.0 " }))).toBe("2.0.0");
    expect(promptVersion(snapshot({ latest: null }))).toBeNull();
    expect(promptVersion(null)).toBeNull();
  });

  it("publishedLabel：合法时间才给文案", () => {
    expect(publishedLabel("2026-01-02T03:04:05Z")).toContain("发布于");
    expect(publishedLabel("")).toBeUndefined();
    expect(publishedLabel(null)).toBeUndefined();
    expect(publishedLabel("not-a-date")).toBeUndefined();
  });

  it("notesNeedToggle：超过三行或明显过长才需要展开", () => {
    expect(notesNeedToggle(null)).toBe(false);
    expect(notesNeedToggle("   ")).toBe(false);
    expect(notesNeedToggle("一\n二\n三")).toBe(false);
    expect(notesNeedToggle("一\n二\n三\n四")).toBe(true);
    expect(NOTES_PREVIEW_LINES).toBe(3);
    expect(notesNeedToggle("说".repeat(121))).toBe(true);
  });
});

describe("cleanNotes（R5-01 前端兜底）", () => {
  /** 已发布的 0.2.2 清单 notes 原文（整份 Release Markdown，不会回改）。 */
  const RAW_0_2_2 = [
    "# WorkBuddy Shift 0.2.2",
    "",
    "发布日期：2026-11-20",
    "",
    "本次更新修复了部分会话无法复制的问题，并让复制前的检查结果更容易理解。",
    "",
    "## 修复",
    "",
    "- **会话复制被整份拒绝**：末端半条记录现在会被跳过，其余内容与前面的记录都会保留。",
    "- **两行粘连导致复制失败**：现在会按记录边界无损拆开，内容不丢失。",
    "",
    "## 升级说明",
    "",
    "- 0.2.1 用户可直接在应用内升级。",
    "",
    "## 下载哪个？",
    "",
    "| 系统 | 文件 |",
    "| --- | --- |",
    "| Windows 10/11（64 位） | workbuddy-shift_0.2.2_x64-setup.exe |",
    "| macOS Apple Silicon（M1–M4） | workbuddy-shift_0.2.2_aarch64.dmg |",
    "",
    "## 其余文件是什么？",
    "",
    "| 文件 | 用途 |",
    "| --- | --- |",
    "| *.sig | 自动更新的签名文件，不要手动下载 |",
    "",
    "> `latest-macos-*.json` 与 `latest-darwin-*.json` 内容相同：两者都必须保留。",
  ].join("\n");

  it("空输入 / 只有空白 → 空串", () => {
    expect(cleanNotes(null)).toBe("");
    expect(cleanNotes(undefined)).toBe("");
    expect(cleanNotes("")).toBe("");
    expect(cleanNotes("  \n\n \t ")).toBe("");
  });

  it("旧格式全文：首行是正文，不出现 ## / ** / 表格 / 引用残留", () => {
    const text = cleanNotes(RAW_0_2_2);
    expect(text.split("\n")[0]).toBe(
      "本次更新修复了部分会话无法复制的问题，并让复制前的检查结果更容易理解。",
    );
    expect(text).not.toContain("#");
    expect(text).not.toContain("**");
    expect(text).not.toContain("|");
    expect(text).not.toContain("发布日期");
    expect(text).not.toContain("> ");
    // 表格整段丢弃：表头、分隔行、数据行都不该留下。
    expect(text).not.toContain("workbuddy-shift_0.2.2_x64-setup.exe");
    expect(text).not.toContain("---");
    // 实质内容必须留下（加粗标记被剥掉，文字保留）。
    expect(text).toContain("会话复制被整份拒绝：末端半条记录现在会被跳过");
    expect(text).toContain("0.2.1 用户可直接在应用内升级。");
  });

  it("表格整段丢弃（含表头 / 分隔行 / 数据行）", () => {
    const text = cleanNotes(
      ["| 系统 | 文件 |", "| --- | --- |", "| Windows | a.exe |", "尾巴一句"].join("\n"),
    );
    expect(text).toBe("尾巴一句");
  });

  it("链接保留文字、去掉地址；反引号与 __ 也清掉", () => {
    expect(cleanNotes("[完整说明](https://example.com/x) 已上线")).toBe("完整说明 已上线");
    expect(cleanNotes("`latest.json` 不用手动下载")).toBe("latest.json 不用手动下载");
    expect(cleanNotes("__重点__：已修复")).toBe("重点：已修复");
  });

  it("有序列表 / 引用 / 标题标记都被剥掉，连续空行压成一个", () => {
    const text = cleanNotes(
      ["# 标题", "", "> 引用一句", "", "", "1. 第一条", "2. 第二条", "", "- 第三条"].join("\n"),
    );
    expect(text).toBe("引用一句\n\n第一条\n第二条\n\n第三条");
  });

  it("card:begin / card:end 区块优先于整份正文", () => {
    const text = cleanNotes(
      [
        "# WorkBuddy Shift 0.2.4",
        "",
        "发布日期：2026-12-01",
        "",
        "<!-- card:begin -->",
        "本次让多客户端用量统计更准确。",
        "- 账号排序：支持按额度恢复时间排序",
        "<!-- card:end -->",
        "",
        "## 下载哪个？",
        "| 系统 | 文件 |",
        "| --- | --- |",
        "| Windows | a.exe |",
      ].join("\n"),
    );
    expect(text).toBe("本次让多客户端用量统计更准确。\n账号排序：支持按额度恢复时间排序");
  });

  it("超长说明按行边界截断到上限并加省略号", () => {
    const line = "这是一条足够长的更新说明，用来把摘要撑到上限之外。";
    const text = cleanNotes(Array.from({ length: 100 }, () => line).join("\n"));
    expect(text.length).toBeLessThanOrEqual(NOTES_MAX_CHARS);
    expect(text.endsWith("…")).toBe(true);
    // 截断发生在行边界：最后一段完整句子 + 省略号。
    const kept = text.split("\n");
    expect(kept[kept.length - 1]).toBe(`${line}…`);
    expect(NOTES_MAX_CHARS).toBe(600);
  });

  it("单行超长（无换行）也要截断到上限", () => {
    const text = cleanNotes("说".repeat(2000));
    expect(text.length).toBe(NOTES_MAX_CHARS);
    expect(text.endsWith("…")).toBe(true);
  });

  it("清洗后的文本可以直接喂给 notesNeedToggle", () => {
    const text = cleanNotes(RAW_0_2_2);
    expect(notesNeedToggle(text)).toBe(true);
    expect(notesNeedToggle(cleanNotes("只有一句话"))).toBe(false);
  });
});
