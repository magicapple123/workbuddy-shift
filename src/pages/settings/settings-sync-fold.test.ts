import { describe, expect, it } from "vitest";

import {
  accountsFoldSummary,
  previewFoldSummary,
  SETTINGS_SYNC_FOLD_DEFAULT_OPEN,
} from "@/pages/settings/settings-sync-fold";

describe("设置同步卡折叠（R5-06）", () => {
  it("两块明细默认收起", () => {
    expect(SETTINGS_SYNC_FOLD_DEFAULT_OPEN.accounts).toBe(false);
    expect(SETTINGS_SYNC_FOLD_DEFAULT_OPEN.preview).toBe(false);
  });

  it("账号摘要给出 总数 / 可同步 / 未登录 三个计数", () => {
    const rows = [
      { hasDirectory: true },
      { hasDirectory: true },
      { hasDirectory: false },
    ];
    expect(accountsFoldSummary(rows)).toBe("3 个账号 · 2 个可同步 · 1 个未登录");
  });

  it("账号为空时摘要仍可用（0 个账号）", () => {
    expect(accountsFoldSummary([])).toBe("0 个账号 · 0 个可同步 · 0 个未登录");
  });

  it("全部未登录时可同步为 0", () => {
    expect(accountsFoldSummary([{ hasDirectory: false }, { hasDirectory: false }])).toBe(
      "2 个账号 · 0 个可同步 · 2 个未登录",
    );
  });

  it("预览摘要显示待同步账号数与项数", () => {
    expect(previewFoldSummary(3, 7)).toBe("待同步 3 个账号 · 7 项");
  });

  it("无变更时预览摘要显示已一致", () => {
    expect(previewFoldSummary(0, 0)).toBe("已一致");
  });
});
