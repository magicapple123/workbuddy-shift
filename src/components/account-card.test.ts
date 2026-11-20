import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";

import { AccountCard } from "./account-card";
import type { AccountMeta, CreditExpiry, CreditResource } from "@/lib/types";

/**
 * 企业版卡片展示（issue #167）：
 * - `isEnterprise` 渲染「企业版」徽标，个人版账号不渲染；
 * - `unlimited` 渲染「不限量」+「企业配额」，不出现「暂无可用积分」空态；
 * - 个人版有限配额展示不回归。
 *
 * 组件无 DOM 依赖地走 `renderToStaticMarkup`（与现有 Vitest node 环境一致，
 * 不引入 jsdom / testing-library）。
 */

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

function resource(overrides: Partial<CreditResource> = {}): CreditResource {
  return {
    packageCode: "demo-package",
    packageName: "CodeBuddy 个人版积分包",
    total: 2000,
    remaining: 1500,
    used: 500,
    status: 1,
    expireAt: Date.now() + 30 * 24 * 3600 * 1000,
    expired: false,
    expiringSoon: false,
    ...overrides,
  };
}

function render(accountMeta: AccountMeta, credit: CreditExpiry, compact = false): string {
  return renderToStaticMarkup(
    createElement(AccountCard, {
      account: accountMeta,
      onDelete: () => {},
      credit,
      compact,
      featuresDisabled: false,
    }),
  );
}

/** 截取 `<header>` 段，用于断言徽标是否出现在卡片头部。 */
function headerOf(html: string): string {
  return html.slice(html.indexOf("<header"), html.indexOf("</header>"));
}

describe("企业版账号卡片", () => {
  it("企业版有限配额：渲染「企业版」徽标与单条「企业配额」", () => {
    const html = render(
      account({ isEnterprise: true }),
      {
        ok: true,
        unlimited: false,
        totalCapacity: 2000,
        totalRemaining: 1500,
        expiringSoonRemaining: 0,
        expiredRemaining: 0,
        soonestExpireAt: Date.now() + 30 * 24 * 3600 * 1000,
        expiringSoon: false,
        expired: false,
        resources: [resource({ packageName: "企业配额" })],
      },
    );

    expect(html).toContain("企业版");
    expect(html).toContain("企业配额");
    expect(html).toContain("1,500");
    expect(html).not.toContain("暂无可用积分");
    expect(html).not.toContain("不限量");
  });

  it("企业版不限量：渲染「不限量」，不出现「暂无可用积分」", () => {
    const html = render(
      account({ isEnterprise: true }),
      {
        ok: true,
        unlimited: true,
        totalCapacity: null,
        totalRemaining: null,
        expiringSoonRemaining: 0,
        expiredRemaining: 0,
        soonestExpireAt: null,
        expiringSoon: false,
        expired: false,
        resources: [],
      },
    );

    expect(html).toContain("不限量");
    expect(html).toContain("企业配额");
    expect(html).toContain("企业配额 · 不限量");
    expect(html).not.toContain("暂无可用积分");
    expect(html).not.toContain("0 个积分包");
  });

  it("企业账号查询失败：渲染可读错误，不出现「暂无可用积分」", () => {
    const html = render(account({ isEnterprise: true }), {
      ok: false,
      error: "企业账号缺少企业 ID，无法查询企业配额",
    });

    expect(html).toContain("企业版");
    expect(html).toContain("企业账号缺少企业 ID，无法查询企业配额");
    expect(html).not.toContain("暂无可用积分");
    expect(html).not.toContain("不限量");
  });

  it("个人版账号不回归：无「企业版」徽标，正常展示积分包与余额", () => {
    const html = render(account(), {
      ok: true,
      totalCapacity: 2000,
      totalRemaining: 1500,
      expiringSoonRemaining: 0,
      expiredRemaining: 0,
      soonestExpireAt: Date.now() + 30 * 24 * 3600 * 1000,
      expiringSoon: false,
      expired: false,
      resources: [resource()],
    });

    expect(html).not.toContain("企业版");
    expect(html).not.toContain("不限量");
    expect(html).toContain("1 个积分包");
    expect(html).toContain("1,500");
  });

  it("宽松卡片：企业版徽标在头部 chips 中", () => {
    const html = render(account({ isEnterprise: true }), {
      ok: true,
      unlimited: false,
      totalCapacity: 2000,
      totalRemaining: 1500,
      expiringSoonRemaining: 0,
      expiredRemaining: 0,
      soonestExpireAt: null,
      expiringSoon: false,
      expired: false,
      resources: [resource({ packageName: "企业配额" })],
    });

    expect(headerOf(html)).toContain("企业版");
  });

  it("紧凑卡片：企业版徽标在积分区，头部不出现（避免挤压账号名）", () => {
    const html = render(
      account({ isEnterprise: true }),
      {
        ok: true,
        unlimited: false,
        totalCapacity: 2000,
        totalRemaining: 1500,
        expiringSoonRemaining: 0,
        expiredRemaining: 0,
        soonestExpireAt: null,
        expiringSoon: false,
        expired: false,
        resources: [resource({ packageName: "企业配额" })],
      },
      true,
    );

    // 头部单行空间已被状态 chip 与操作按钮占满：徽标不得出现在 header 内。
    expect(headerOf(html)).not.toContain("企业版");
    // 积分区保留标注与「企业配额」语义。
    expect(html).toContain("企业版");
    expect(html).toContain("企业配额");
    expect(html).toContain("1,500");
  });
});

/** DemoAction 包装层的无障碍标记：出现一次 = 一个写操作被演示模式接管。 */
const DEMO_WRAPPER_MARK = 'aria-label="演示模式下不可操作"';

function countDemoWrappers(html: string): number {
  return html.split(DEMO_WRAPPER_MARK).length - 1;
}

/**
 * 演示模式一致性：所有客户端的写操作入口都必须被 `DemoAction` 接管（不执行真实写操作），
 * 不能出现 WorkBuddy / VS Code / JetBrains 被接管、而 IDE / CLI 仍可点击的不一致。
 */
describe("演示模式下客户端操作一致禁用", () => {
  it("宽松卡片：5 个客户端操作入口全部由 DemoAction 接管，写操作菜单项不渲染", async () => {
    vi.resetModules();
    vi.stubEnv("VITE_DEMO_MODE", "1");
    try {
      const { AccountCard: DemoAccountCard } = await import("./account-card");
      const html = renderToStaticMarkup(
        createElement(DemoAccountCard, {
          account: account(),
          onDelete: () => {},
          featuresDisabled: false,
        }),
      );
      expect(countDemoWrappers(html)).toBe(5);
      expect(html).not.toContain("刷新 Token");
      expect(html).not.toContain("删除账号");
      // 只读的「账号信息」入口仍在（菜单内容由 Radix 在打开时才渲染，静态标记只校验触发器）。
      expect(html).toContain("管理账号 测试账号");
    } finally {
      vi.unstubAllEnvs();
      vi.resetModules();
    }
  });

  it("紧凑卡片：产品切换收成一个下拉（R5-05），不再内联 5 个写操作按钮", async () => {
    vi.resetModules();
    vi.stubEnv("VITE_DEMO_MODE", "1");
    try {
      const { AccountCard: DemoAccountCard } = await import("./account-card");
      const html = renderToStaticMarkup(
        createElement(DemoAccountCard, {
          account: account(),
          onDelete: () => {},
          compact: true,
          featuresDisabled: false,
        }),
      );
      // 紧凑卡改成「一个下拉 + 置灰原因」：不再有 5 份 DemoAction 包装，也没有内联的写按钮。
      expect(countDemoWrappers(html)).toBe(0);
      expect(html.split('aria-label="切换产品"').length - 1).toBe(1);
      expect(html).not.toContain("设为 WorkBuddy 当前账号");
      expect(html).not.toContain("切换到 CodeBuddy IDE");
    } finally {
      vi.unstubAllEnvs();
      vi.resetModules();
    }
  });

  it("非演示模式：操作按钮直接渲染，没有 DemoAction 包装", () => {
    const html = render(account(), {
      ok: true,
      unlimited: false,
      totalCapacity: 2000,
      totalRemaining: 1500,
      expiringSoonRemaining: 0,
      expiredRemaining: 0,
      soonestExpireAt: null,
      expiringSoon: false,
      expired: false,
      resources: [resource({ packageName: "企业配额" })],
    });
    expect(countDemoWrappers(html)).toBe(0);
  });
});

/**
 * 紧凑卡片头部原本内联 5 份「当前账号」角标（标记 + 勾选角标），现统一复用
 * `ProductCurrentState` 的图标角标形态；宽卡片 footer 的胶囊形态保持原样。
 *
 * R5-05 起紧凑卡的产品入口收成一个下拉，头部不再出现 5 份角标：
 * 当前产品由下拉触发器上的图标表达，其余产品在菜单里用勾选表达。
 */
describe("当前账号角标复用 ProductCurrentState", () => {
  it("宽松卡片：5 个当前账号角标都有 role=status 与产品 aria-label（footer 胶囊形态）", () => {
    const html = renderToStaticMarkup(
      createElement(AccountCard, {
        account: account(),
        onDelete: () => {},
        featuresDisabled: false,
        workbuddyActive: true,
        codebuddyCnIdeActive: true,
        vscodeExtActive: true,
        jetbrainsActive: true,
        codebuddyCliActive: true,
      }),
    );
    expect(html.split('role="status"').length - 1).toBe(5);
    for (const label of [
      "WorkBuddy 当前账号",
      "CodeBuddy IDE 当前账号",
      "VS Code CodeBuddy 插件当前账号",
      "JetBrains IDE CodeBuddy 插件当前账号",
      "CodeBuddy CLI 当前账号",
    ]) {
      expect(html).toContain(`aria-label="${label}"`);
    }
    // 当前账号态不是操作入口，不应出现演示模式的只读包装。
    expect(countDemoWrappers(html)).toBe(0);
  });

  it("紧凑卡片：产品入口收成一个下拉，头部不再内联 5 份角标", () => {
    const html = renderToStaticMarkup(
      createElement(AccountCard, {
        account: account(),
        onDelete: () => {},
        compact: true,
        featuresDisabled: false,
        workbuddyActive: true,
        codebuddyCnIdeActive: true,
        vscodeExtActive: true,
        jetbrainsActive: true,
        codebuddyCliActive: true,
      }),
    );
    expect(html.split('aria-label="切换产品"').length - 1).toBe(1);
    expect(html.split('role="status"').length - 1).toBe(0);
  });
});

/**
 * R5-04：标记在卡片上**只有图标**（Tag，无文字、无左侧色条），可点击，读屏可读。
 *
 * 过期后图标消失（`activeMark` 过滤），但账号库里的记录仍在——这条只校验渲染，
 * 持久化由 core 单测覆盖。
 */
describe("账号标记图标", () => {
  const liveMark = {
    text: "即将限额",
    color: "warning" as const,
    createdAt: Date.now(),
    expiresAt: Date.now() + 2 * 3_600_000,
  };

  it("有标记：状态区渲染可点击的标记图标，aria-label 含文本与剩余时间", () => {
    const html = render(
      account({ mark: liveMark }),
      {
        ok: true,
        unlimited: false,
        totalCapacity: 2000,
        totalRemaining: 1500,
        expiringSoonRemaining: 0,
        expiredRemaining: 0,
        soonestExpireAt: null,
        expiringSoon: false,
        expired: false,
        resources: [],
      },
    );
    const header = headerOf(html);
    expect(header).toContain("aria-label=\"标记：即将限额，剩 2 小时，点击编辑\"");
    // 只有图标：状态区里不应出现标记文本。
    expect(header).not.toContain(">即将限额<");
    // 可点击 → 是 button 而不是 span。
    expect(header).toContain("<button");
  });

  it("没有标记：不渲染标记图标", () => {
    const html = render(account(), {
      ok: true,
      unlimited: false,
      totalCapacity: 2000,
      totalRemaining: 1500,
      expiringSoonRemaining: 0,
      expiredRemaining: 0,
      soonestExpireAt: null,
      expiringSoon: false,
      expired: false,
      resources: [],
    });
    expect(headerOf(html)).not.toContain("标记：");
  });

  it("标记已过期：图标消失（记录保留由 core 单测覆盖）", () => {
    const html = render(
      account({
        mark: { ...liveMark, createdAt: Date.now() - 7_200_000, expiresAt: Date.now() - 60_000 },
      }),
      {
        ok: true,
        unlimited: false,
        totalCapacity: 2000,
        totalRemaining: 1500,
        expiringSoonRemaining: 0,
        expiredRemaining: 0,
        soonestExpireAt: null,
        expiringSoon: false,
        expired: false,
        resources: [],
      },
    );
    expect(headerOf(html)).not.toContain("标记：");
  });

  it("紧凑卡片同样只渲染图标（不挤占账号名、不出现文字 chip）", () => {
    const html = render(
      account({ mark: liveMark }),
      {
        ok: true,
        unlimited: false,
        totalCapacity: 2000,
        totalRemaining: 1500,
        expiringSoonRemaining: 0,
        expiredRemaining: 0,
        soonestExpireAt: null,
        expiringSoon: false,
        expired: false,
        resources: [],
      },
      true,
    );
    expect(headerOf(html)).toContain("标记：即将限额");
    expect(headerOf(html)).not.toContain(">即将限额<");
  });
});

/**
 * R5-05 紧凑卡 3 槽收纳的渲染侧断言：超出槽位的状态折叠成 `+N`，
 * `aria-label` 列出被收起的项（选择逻辑本身在 `status-selection.test.ts`）。
 */
describe("紧凑卡状态收纳", () => {
  it("标记 + 3 个系统状态 → 渲染 +1，且被收起项的文案进 aria-label", () => {
    const html = renderToStaticMarkup(
      createElement(AccountCard, {
        account: account({
          mark: {
            text: "即将限额",
            color: "warning",
            createdAt: Date.now(),
            expiresAt: Date.now() + 3_600_000,
          },
        }),
        onDelete: () => {},
        compact: true,
        featuresDisabled: false,
        todayCheckedIn: true,
        travelStatus: { label: "traveling", locationName: "巴黎", rewardCredit: 100, arriveAt: null },
        rateLimits: [{ model: "claude", resetAt: Date.now() + 600_000, firstSeenAt: 0, hitCount: 1 }],
      }),
    );
    const header = headerOf(html);
    // 4 个状态 → 槽 3 → 收起 1 个（旅行，优先级最后）。
    expect(header).toContain("+1");
    expect(header).toContain("还有 1 个状态：");
    // 标记永远不进 +N。
    expect(header).toContain("标记：即将限额");
  });
});
