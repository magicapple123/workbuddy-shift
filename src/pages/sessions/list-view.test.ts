import { describe, expect, it } from "vitest";

import type { SessionGroupSummary } from "@/lib/types";

import { PAGE_SIZE, PAGINATION_HIDE_MAX, PROJECT_SECTIONS_PER_PAGE } from "./constants";
import { filterAndSortGroups, groupByProject, paginateList } from "./list-view";

/**
 * 关联会话列表「搜索 / 筛选 / 排序 / 分组 / 分页」纯逻辑单测。
 *
 * 这一组用例守的是三件最容易回归、又最难靠肉眼发现的事：
 * 1. 搜索与状态筛选、排序**正交**（搜索不能把筛选结果顶掉，排序不能因搜索失效）；
 * 2. 分组后同一项目的会话**不会被拆到两页**，节头的「N 个会话」永远是该项目全部会话数；
 * 3. 分页条显隐只有一处规则（平铺看条目、分组看页数），页面不再自己算一遍阈值。
 */

function summary(overrides: Partial<SessionGroupSummary> & { title: string }): SessionGroupSummary {
  return {
    key: `key-${overrides.title}`,
    client: "workbuddy",
    variantScope: null,
    groupId: `group-${overrides.title}`,
    groupVariant: "cn",
    projectLabel: "",
    latestActivityAt: 0,
    memberCount: 2,
    activeMemberCount: 2,
    accountNames: ["账号 A", "账号 B"],
    summaryStatus: "latest",
    summaryText: "内容一致",
    safeSourceMemberId: null,
    hasSafeSource: false,
    ...overrides,
  };
}

describe("filterAndSortGroups", () => {
  const groups = [
    summary({ title: "登录页重构", projectLabel: "workbuddy-shift", latestActivityAt: 300, excerpt: "把登录页的校验改成受控" }),
    summary({ title: "导出 xlsx", projectLabel: "workbuddy-shift", latestActivityAt: 100, excerpt: "导出按钮加进度" }),
    summary({ title: "随机记录", projectLabel: "sandbox", latestActivityAt: 200, excerpt: null }),
  ];

  it("默认按最近更新降序，时间相同的按标题排序保证稳定", () => {
    const sorted = filterAndSortGroups(groups, { statusFilter: "all", sortOrder: "recent", query: "" });
    expect(sorted.map((group) => group.title)).toEqual(["登录页重构", "随机记录", "导出 xlsx"]);

    const tied = filterAndSortGroups(
      [
        summary({ title: "B 会话", latestActivityAt: 500 }),
        summary({ title: "A 会话", latestActivityAt: 500 }),
      ],
      { statusFilter: "all", sortOrder: "recent", query: "" },
    );
    expect(tied.map((group) => group.title)).toEqual(["A 会话", "B 会话"]);
  });

  it("sortOrder 为 oldest 时按最早更新升序", () => {
    const sorted = filterAndSortGroups(groups, { statusFilter: "all", sortOrder: "oldest", query: "" });
    expect(sorted.map((group) => group.title)).toEqual(["导出 xlsx", "随机记录", "登录页重构"]);
  });

  it("搜索同时命中标题、项目名与摘要，且不区分大小写", () => {
    const byTitle = filterAndSortGroups(groups, { statusFilter: "all", sortOrder: "recent", query: "登录" });
    expect(byTitle.map((group) => group.title)).toEqual(["登录页重构"]);

    const byProject = filterAndSortGroups(groups, { statusFilter: "all", sortOrder: "recent", query: "WORKBUDDY-SHIFT" });
    expect(byProject.map((group) => group.title)).toEqual(["登录页重构", "导出 xlsx"]);

    const byExcerpt = filterAndSortGroups(groups, { statusFilter: "all", sortOrder: "recent", query: "进度" });
    expect(byExcerpt.map((group) => group.title)).toEqual(["导出 xlsx"]);
  });

  it("纯空白搜索词等同不搜索（不会把所有卡片筛掉）", () => {
    const result = filterAndSortGroups(groups, { statusFilter: "all", sortOrder: "recent", query: "   " });
    expect(result).toHaveLength(groups.length);
  });

  it("搜索与状态筛选叠加生效，且排序在筛选之后仍然有效", () => {
    const mixed = [
      summary({ title: "待同步-项目A", projectLabel: "项目A", summaryStatus: "behind", latestActivityAt: 10 }),
      summary({ title: "内容一致-项目A", projectLabel: "项目A", summaryStatus: "latest", latestActivityAt: 20 }),
      summary({ title: "待同步-项目B", projectLabel: "项目B", summaryStatus: "behind", latestActivityAt: 30 }),
    ];
    const result = filterAndSortGroups(mixed, { statusFilter: "behind", sortOrder: "recent", query: "项目" });
    expect(result.map((group) => group.title)).toEqual(["待同步-项目B", "待同步-项目A"]);
  });

  it("无命中时返回空数组（页面据此渲染搜索空态）", () => {
    expect(filterAndSortGroups(groups, { statusFilter: "all", sortOrder: "recent", query: "不存在的词" })).toEqual([]);
  });

  it("不修改入参数组的顺序", () => {
    const input = [...groups];
    filterAndSortGroups(input, { statusFilter: "all", sortOrder: "oldest", query: "" });
    expect(input.map((group) => group.title)).toEqual(["登录页重构", "导出 xlsx", "随机记录"]);
  });
});

describe("groupByProject", () => {
  it("同项目会话收进同一节，节内保持入参顺序", () => {
    const groups = [
      summary({ title: "a1", projectLabel: "项目A", latestActivityAt: 900 }),
      summary({ title: "b1", projectLabel: "项目B", latestActivityAt: 800 }),
      summary({ title: "a2", projectLabel: "项目A", latestActivityAt: 700 }),
      summary({ title: "a3", projectLabel: "项目A", latestActivityAt: 600 }),
    ];
    const sections = groupByProject(groups);
    expect(sections).toHaveLength(2);
    expect(sections[0].project).toBe("项目A");
    expect(sections[0].groups.map((group) => group.title)).toEqual(["a1", "a2", "a3"]);
    expect(sections[1].groups.map((group) => group.title)).toEqual(["b1"]);
  });

  it("节的顺序跟随各节第一条会话在排序里的位置（最近更新的项目在最前）", () => {
    const groups = [
      summary({ title: "新-项目A", projectLabel: "项目A", latestActivityAt: 900 }),
      summary({ title: "旧-项目B", projectLabel: "项目B", latestActivityAt: 100 }),
      summary({ title: "次新-项目B", projectLabel: "项目B", latestActivityAt: 500 }),
    ];
    expect(groupByProject(groups).map((section) => section.project)).toEqual(["项目A", "项目B"]);
  });

  it("空项目名归入「未标记项目」（project 为空字符串），不会与同名项目混淆", () => {
    const sections = groupByProject([
      summary({ title: "无项目", projectLabel: "" }),
      summary({ title: "有项目", projectLabel: "未标记项目" }),
    ]);
    expect(sections.map((section) => section.project)).toEqual(["", "未标记项目"]);
    expect(sections[0].groups).toHaveLength(1);
    expect(sections[1].groups).toHaveLength(1);
  });

  it("空列表返回空节数组", () => {
    expect(groupByProject([])).toEqual([]);
  });
});

describe("paginateList", () => {
  function many(count: number, project = "项目A"): SessionGroupSummary[] {
    // 标题带项目前缀：工厂里的 `groupId` / `key` 由标题派生，跨项目重名会让「两页合起来
    // 无重复无遗漏」这条断言失效（那是测试代码的问题，不是分页逻辑的问题）。
    return Array.from({ length: count }, (_, index) =>
      summary({ title: `${project}-${index + 1}`, projectLabel: project, latestActivityAt: 1000 - index }));
  }

  it("平铺模式按条目分页，并夹紧越界页码", () => {
    const groups = many(PAGE_SIZE * 2 + 1);
    const first = paginateList(groups, [], false, 1);
    expect(first.pageCount).toBe(3);
    expect(first.visibleGroups).toHaveLength(PAGE_SIZE);
    expect(first.visibleGroups[0].title).toBe("项目A-1");
    expect(first.visibleSections).toEqual([]);

    const last = paginateList(groups, [], false, 3);
    expect(last.currentPage).toBe(3);
    expect(last.visibleGroups).toHaveLength(1);

    // 页码越界（例如筛选后页数变少）时夹到最后一页，不停在空页上。
    expect(paginateList(groups, [], false, 99).currentPage).toBe(3);
    expect(paginateList(groups, [], false, 0).currentPage).toBe(1);
  });

  it("条目不超过 PAGINATION_HIDE_MAX 时隐藏分页条", () => {
    const groups = many(PAGINATION_HIDE_MAX);
    const page = paginateList(groups, [], false, 1);
    expect(page.pageCount).toBe(1);
    expect(page.showPagination).toBe(false);
    expect(paginateList(many(PAGINATION_HIDE_MAX + 1), [], false, 1).showPagination).toBe(true);
  });

  it("分组模式按「节」分页：同一项目不会被拆到两页，节头数量即该项目全部会话数", () => {
    // 5 个项目，每个项目 3 条会话：共 15 条（> PAGE_SIZE），但只有 5 节。
    const groups = ["项目A", "项目B", "项目C", "项目D", "项目E"].flatMap((project) => many(3, project));
    const sections = groupByProject(groups);
    expect(sections).toHaveLength(5);

    const first = paginateList(groups, sections, true, 1);
    expect(first.pageCount).toBe(2);
    expect(first.visibleSections).toHaveLength(PROJECT_SECTIONS_PER_PAGE);
    expect(first.visibleSections.map((section) => section.project)).toEqual(["项目A", "项目B", "项目C", "项目D"]);
    // 节内条目全给：4 节 × 3 条 = 12 条，超过平铺的 PAGE_SIZE 也不拆节。
    expect(first.visibleGroups).toHaveLength(12);
    for (const section of first.visibleSections) expect(section.groups).toHaveLength(3);

    const second = paginateList(groups, sections, true, 2);
    expect(second.visibleSections.map((section) => section.project)).toEqual(["项目E"]);
    expect(second.visibleGroups).toHaveLength(3);

    // 两页合起来正好是全部会话，既无重复也无遗漏。
    const ids = [...first.visibleGroups, ...second.visibleGroups].map((group) => group.groupId);
    expect(new Set(ids).size).toBe(groups.length);
  });

  it("分组模式只有真的不止一页时才显示分页条", () => {
    const onePageGroups = many(3, "项目A").concat(many(3, "项目B"));
    const onePageSections = groupByProject(onePageGroups);
    expect(paginateList(onePageGroups, onePageSections, true, 1).showPagination).toBe(false);

    const multi = ["项目A", "项目B", "项目C", "项目D", "项目E"].flatMap((project) => many(1, project));
    expect(paginateList(multi, groupByProject(multi), true, 1).showPagination).toBe(true);
  });

  it("空列表仍算一页，且不显示分页条（页面渲染空态而不是分页条）", () => {
    const page = paginateList([], [], false, 1);
    expect(page.pageCount).toBe(1);
    expect(page.currentPage).toBe(1);
    expect(page.visibleGroups).toEqual([]);
    expect(page.showPagination).toBe(false);
  });
});
