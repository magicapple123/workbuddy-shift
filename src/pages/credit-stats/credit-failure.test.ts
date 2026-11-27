import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { MemoryRouter } from "react-router-dom";
import { describe, expect, it } from "vitest";

import { FailureActions } from "@/components/failure-actions";
import type { AccountMeta, CreditOfficialUsageError } from "@/lib/types";
import {
  creditFailureActionsProps,
  findCreditFailure,
  reloginAccountOf,
  type FailureActionsProps,
} from "./credit-failure";

/** 渲染动作行（本仓测试环境是 node：只有 SSR，没有 jsdom / @testing-library）。 */
function render(props: FailureActionsProps): string {
  return renderToStaticMarkup(
    createElement(MemoryRouter, { children: createElement(FailureActions, props) }),
  );
}

/** 只用到 `id` 与 `variant` 两列，其余字段与「重新登录」选档位无关。 */
const ACCOUNTS: AccountMeta[] = [
  { id: "acc-1", variant: "cn" } as unknown as AccountMeta,
  { id: "acc-2", variant: "ai" } as unknown as AccountMeta,
];

/**
 * 后端 `officialUsage.errors[]` 的一项。
 *
 * 这是**数据通道**：桌面端 `get_credit_expiry` / `get_credit_statistics` 失败时也是正常
 * resolve（Value 里是 `{ok:false,…}` / `errors[]`），`catch` 不会执行——所以动作只能从
 * 这里派生。档位由 core 的 `official_usage::UsageFetchError` 一路带上来。
 */
function errorsWith(entry: Partial<CreditOfficialUsageError>): CreditOfficialUsageError[] {
  return [{ accountId: "acc-1", accountName: "账号一", error: "取数失败", ...entry }];
}

describe("积分页失败动作（数据通道 errors[]）", () => {
  it("login_expired → 渲染「重新登录」，并接上登录流程", () => {
    const hit = findCreditFailure(
      errorsWith({
        accountId: "acc-1",
        accountName: "账号一",
        error: "登录已失效，需要重新登录",
        failureKind: "login_expired",
        httpStatus: 401,
      }),
    );

    expect(hit).not.toBeNull();
    expect(hit?.failure.failureKind).toBe("login_expired");
    expect(hit?.failure.httpStatus).toBe(401);
    // 文案仍是后端给的那句（一字节不改）。
    expect(hit?.failure.message).toBe("登录已失效，需要重新登录");
    expect(hit?.accountId).toBe("acc-1");

    // 登录目标账号能对上 → 传 handler → 按钮出现。
    const account = reloginAccountOf(hit, ACCOUNTS);
    expect(account?.id).toBe("acc-1");

    let opened = 0;
    const props = creditFailureActionsProps(hit, ACCOUNTS, () => {
      opened += 1;
    });
    expect(typeof props.onRelogin).toBe("function");
    // 直接调用 handler = 点按钮后走的那条路（本仓无 DOM，无法模拟真实 click）。
    props.onRelogin?.();
    expect(opened).toBe(1);

    const markup = render(props);
    expect(markup).toContain("重新登录");
    expect(markup).not.toContain("检查更新代理");
  });

  it("upstream → 零按钮、无空位、无空 div", () => {
    const hit = findCreditFailure(
      errorsWith({
        error: "账号不存在",
        failureKind: "upstream",
        httpStatus: 400,
      }),
    );

    // 档位存在但「上游已给人话」→ 没有动作。
    expect(hit).not.toBeNull();
    expect(hit?.failure.action).toBeNull();

    // 渲染结果是空串：连外层 div 都没渲染，不存在空位。
    expect(render(creditFailureActionsProps(hit, ACCOUNTS, () => {}))).toBe("");
  });

  it("没有 failureKind 的纯文案错误 → 零按钮（也不伪造档位）", () => {
    const hit = findCreditFailure(
      errorsWith({ error: "官方用量分页超过安全上限" }),
    );

    expect(hit).toBeNull();
    expect(render(creditFailureActionsProps(null, ACCOUNTS, () => {}))).toBe("");
    // 没有 hit 时也不给 handler（不留点了没用的按钮）。
    expect(creditFailureActionsProps(null, ACCOUNTS, () => {}).onRelogin).toBeUndefined();
  });

  it("空 errors / 未定义 errors 不会产生失败动作", () => {
    expect(findCreditFailure([])).toBeNull();
    expect(findCreditFailure(undefined)).toBeNull();
    expect(render(creditFailureActionsProps(null, ACCOUNTS, () => {}))).toBe("");
  });

  it("多条错误时取第一条带档位的（前面无档位的项被跳过）", () => {
    const hit = findCreditFailure([
      { accountId: "acc-2", accountName: "账号二", error: "官方响应格式无效" },
      {
        accountId: "acc-1",
        accountName: "账号一",
        error: "服务端返回了网页而不是数据，可能是代理或网关拦截",
        failureKind: "gateway_html",
        httpStatus: 502,
      },
    ]);
    expect(hit?.accountId).toBe("acc-1");
    expect(hit?.failure.action).toBe("check_proxy");
  });

  it("账号已被删（accountId 对不上）→ 不给 handler，按钮不出现", () => {
    const hit = findCreditFailure(
      errorsWith({
        accountId: "acc-gone",
        accountName: "已删除账号",
        error: "登录已失效，需要重新登录",
        failureKind: "login_expired",
        httpStatus: 401,
      }),
    );
    expect(hit).not.toBeNull();
    expect(reloginAccountOf(hit, ACCOUNTS)).toBeNull();
    const props = creditFailureActionsProps(hit, ACCOUNTS, () => {});
    expect(props.onRelogin).toBeUndefined();
    // relogin 没有落点 → 整块不渲染（没有「点了没反应」的按钮）。
    expect(render(props)).toBe("");
  });
});
