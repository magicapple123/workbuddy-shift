import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";

import type { AccountMeta } from "@/lib/types";

import { AccountStatusChips, RELOGIN_RECOVERY_HINT } from "./status-chips";

/**
 * 账号卡片状态 chip 的凭据相关文案。
 *
 * 走 `renderToStaticMarkup`（与 `account-card.test.ts` 一致，不引入 jsdom / testing-library）。
 * R6-05 钉住一件事：失效提示不再指向早已不存在的「从本机导入」入口，恢复路径是 OAuth 重新扫码。
 */

const NEEDS_RELOGIN_LABEL = "需重新登录";
const LEGACY_HINT_KEYWORD = "从本机导入";

function account(overrides: Partial<AccountMeta> = {}): AccountMeta {
  return {
    id: "acc-1",
    uid: "u-1",
    email: "user@example.com",
    nickname: "测试账号",
    enterpriseName: null,
    expiresAt: null,
    refreshExpiresAt: null,
    refreshedAt: null,
    createdAt: null,
    needsRelogin: false,
    needsReloginReason: null,
    ...overrides,
  };
}

/** 渲染卡片头部的状态区（其余 chip 都不触发，只看凭据失效的那个）。 */
function render(accountMeta: AccountMeta): string {
  return renderToStaticMarkup(
    createElement(AccountStatusChips, {
      account: accountMeta,
      activeProductCount: 0,
      compact: false,
      now: Date.now(),
    }),
  );
}

describe("失效提示的恢复路径（R6-05）", () => {
  it("hint 不再提「从本机导入」，改为「删除该账号后重新 OAuth 扫码」", () => {
    const html = render(account({ needsRelogin: true, needsReloginReason: "缺少 refresh token" }));

    expect(html).toContain(NEEDS_RELOGIN_LABEL);
    expect(html).not.toContain(LEGACY_HINT_KEYWORD);
    expect(html).toContain("OAuth 扫码");
    expect(html).toContain(RELOGIN_RECOVERY_HINT);
  });

  it("Token 过期（未标 needsRelogin）用同一句恢复说明", () => {
    const html = render(account({ expiresAt: Date.now() - 60_000 }));

    expect(html).toContain("Token 已过期");
    expect(html).not.toContain(LEGACY_HINT_KEYWORD);
    expect(html).toContain(RELOGIN_RECOVERY_HINT);
  });
});
