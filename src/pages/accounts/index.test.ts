import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";

import AccountsPage from "./index";

/**
 * 页面拆分后的最小渲染冒烟：初始 store 为空时必须能渲染空态，
 * 不因新增 hooks/组件的 props 或导入缺失在首屏崩溃。
 *
 * 走 `renderToStaticMarkup`（与现有 Vitest node 环境一致，不引入 jsdom）；
 * effects 不执行，因此不会发起真实请求。
 */
describe("账号页", () => {
  it("空账号时渲染标题与空态", () => {
    const html = renderToStaticMarkup(createElement(AccountsPage));
    expect(html).toContain("账号管理");
    expect(html).toContain("添加与迁移账号");
    expect(html).toContain("暂无账号");
  });
});
