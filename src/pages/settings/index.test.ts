import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";

import SettingsPage from "./index";
import { visibleSettingsSections } from "./sections";

/**
 * 设置页最小渲染冒烟（与账号页同一个套路，`renderToStaticMarkup`，不引 jsdom）。
 *
 * 两件事必须在改动结构时被守住：
 * 1. 首屏不因新增/重排卡片崩溃（effects 不执行，所以不会发起真实请求）；
 * 2. **注册表里的每一节都真的渲染出了自己的锚点**——目录与深链都靠 `id` 定位，
 *    漏挂一张卡不会报错，只会让 `/settings#settings-xxx` 跳到一个不存在的节点。
 */
describe("设置页", () => {
  it("渲染全部可见节，并保留它们的锚点 id", () => {
    const html = renderToStaticMarkup(createElement(SettingsPage));

    expect(html).toContain("设置");
    for (const section of visibleSettingsSections()) {
      expect(html, `缺少锚点 ${section.id}`).toContain(`id="${section.id}"`);
      expect(html, `缺少标题 ${section.title}`).toContain(section.title);
    }
  });

  it("四组标题按固定顺序出现在首屏", () => {
    const html = renderToStaticMarkup(createElement(SettingsPage));
    const order = ["通用", "账号与同步", "自动化", "桌面与维护"].map((title) =>
      html.indexOf(`>${title}<`),
    );
    for (const index of order) expect(index).toBeGreaterThan(-1);
    expect([...order].sort((a, b) => a - b)).toEqual(order);
  });
});
