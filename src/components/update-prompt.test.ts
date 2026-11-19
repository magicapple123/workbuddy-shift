import { describe, expect, it } from "vitest";

import type { UpdateSnapshot } from "@/lib/types";

import {
  NOTES_PREVIEW_LINES,
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
