import type { SessionGroupSummary } from "@/lib/types";

import { PAGE_SIZE, PAGINATION_HIDE_MAX, PROJECT_SECTIONS_PER_PAGE } from "./constants";

/** 列表的呈现方式：平铺（默认）或按项目折叠成节。 */
export type SessionGroupMode = "none" | "project";

/** 按项目分组后的一节（`project === ""` 表示「未标记项目」）。 */
export interface SessionGroupSection {
  project: string;
  groups: SessionGroupSummary[];
}

/** 搜索 / 筛选 / 排序三件套的状态（三个维度正交，互不覆盖）。 */
export interface ListFilterInput {
  /** 状态筛选：`"all"` 不过滤，其余按 `summaryStatus` 精确匹配。 */
  statusFilter: string;
  /** 排序：`"oldest"` 最早更新在前，其它值按「最近更新」降序。 */
  sortOrder: string;
  /** 搜索词：空/纯空白 = 不过滤；否则同时匹配标题、项目名与卡片摘要（不区分大小写）。 */
  query: string;
}

/**
 * 对会话组做「状态筛选 → 搜索 → 排序」，返回分页之前的全部条目。
 *
 * 顺序是刻意的：先按状态筛、再按搜索筛，两者取交集（不是互相覆盖）；排序最后做，
 * 保证筛选后条目的相对次序与用户选的排序口径一致。
 */
export function filterAndSortGroups(
  groups: SessionGroupSummary[],
  input: ListFilterInput,
): SessionGroupSummary[] {
  const keyword = input.query.trim().toLowerCase();
  const result = groups.filter((group) => {
    if (input.statusFilter !== "all" && group.summaryStatus !== input.statusFilter) return false;
    if (!keyword) return true;
    // 标题 / 项目名 / 摘要：用户在卡片上能看到的三样东西都能搜。
    return [group.title, group.projectLabel, group.excerpt ?? ""]
      .some((field) => field.toLowerCase().includes(keyword));
  });
  result.sort((left, right) => (input.sortOrder === "oldest"
    ? left.latestActivityAt - right.latestActivityAt
    : left.latestActivityAt === right.latestActivityAt
      ? left.title.localeCompare(right.title)
      : right.latestActivityAt - left.latestActivityAt));
  return result;
}

/**
 * 按 `projectLabel` 收拢成节；空项目名归入「未标记项目」节（`project === ""`）。
 *
 * 节的排列取各节**第一条**会话在入参里的位置（`Map` 插入顺序），因此节的次序与排序口径
 * 一致——「最近更新」时最活跃的项目在最前；节内条目保持入参顺序（同一排序）。
 */
export function groupByProject(groups: SessionGroupSummary[]): SessionGroupSection[] {
  const buckets = new Map<string, SessionGroupSummary[]>();
  for (const group of groups) {
    const project = group.projectLabel || "";
    const bucket = buckets.get(project);
    if (bucket) bucket.push(group);
    else buckets.set(project, [group]);
  }
  return Array.from(buckets, ([project, items]) => ({ project, groups: items }));
}

/** 一页要渲染的内容与分页条状态。 */
export interface ListPage {
  /** 总页数：至少 1（空列表也算一页，避免「第 0 页」这种中间态）。 */
  pageCount: number;
  /** 夹紧到 `[1, pageCount]` 之后的当前页：筛选后页数变少时不会停在空页上。 */
  currentPage: number;
  /** 当前页要渲染的会话组（分组模式下是当前页各节的组按节顺序展平）。 */
  visibleGroups: SessionGroupSummary[];
  /** 当前页要渲染的节（仅分组模式有内容）。 */
  visibleSections: SessionGroupSection[];
  /** 是否显示底部分页条。 */
  showPagination: boolean;
}

/**
 * 按当前模式切出一页。
 *
 * 两种模式的分页单位不同：
 * - 平铺：按**条目**分页，每页 `PAGE_SIZE` 个；
 * - 分组：按**节**分页，每页 `PROJECT_SECTIONS_PER_PAGE` 节、节内条目全给。
 *   这样同一项目永远落在同一页的同一节里，节头的「N 个会话」始终是该项目的全部会话数。
 *
 * 分页条的显隐也在这里定：平铺沿用「条目太少就不显示」的老规则（`PAGINATION_HIDE_MAX`），
 * 分组则只有真的不止一页时才显示（否则会出现一个只能点「1」的分页条）。
 */
export function paginateList(
  filteredGroups: SessionGroupSummary[],
  sections: SessionGroupSection[],
  grouped: boolean,
  page: number,
): ListPage {
  const unitCount = grouped ? sections.length : filteredGroups.length;
  const unitSize = grouped ? PROJECT_SECTIONS_PER_PAGE : PAGE_SIZE;
  const pageCount = Math.max(1, Math.ceil(unitCount / unitSize));
  const currentPage = Math.min(Math.max(1, Math.trunc(page) || 1), pageCount);
  const pageStart = (currentPage - 1) * unitSize;
  const visibleSections = grouped ? sections.slice(pageStart, pageStart + unitSize) : [];
  const visibleGroups = grouped
    ? visibleSections.flatMap((section) => section.groups)
    : filteredGroups.slice(pageStart, pageStart + unitSize);
  const showPagination = grouped ? pageCount > 1 : unitCount > PAGINATION_HIDE_MAX;
  return { pageCount, currentPage, visibleGroups, visibleSections, showPagination };
}
