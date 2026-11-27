import { describe, expect, it } from "vitest";

import {
  SETTINGS_GROUPS,
  SETTINGS_SECTIONS,
  filterSections,
  groupSettingsSections,
  visibleSettingsSections,
} from "./sections";

/**
 * 历史锚点：这些 id 曾被文档、深链（`/settings#settings-startup`）与截图证据引用，
 * 改名等于让旧链接静默失效。新增节只能追加，不能重命名或删除已发布的 id。
 */
const LEGACY_SECTION_IDS = [
  "settings-appearance",
  "settings-tools",
  "settings-archive-sync",
  "settings-sync",
  "settings-bundle",
  "settings-about",
  "settings-automation-sync",
  "settings-permission",
  "settings-auto-checkin",
  "settings-auto-rotate",
  "settings-rate-limit",
  "settings-companion",
  "settings-startup",
  "settings-notifications",
  "settings-error-log",
  "settings-updates",
];

describe("设置页节注册表", () => {
  it("保留全部历史锚点 id", () => {
    const ids = new Set(SETTINGS_SECTIONS.map((section) => section.id));
    for (const id of LEGACY_SECTION_IDS) expect(ids.has(id)).toBe(true);
  });

  it("每个节的 id 唯一，且分组都是四个固定组之一", () => {
    const ids = SETTINGS_SECTIONS.map((section) => section.id);
    expect(new Set(ids).size).toBe(ids.length);
    const groupIds = new Set(SETTINGS_GROUPS.map((group) => group.id));
    for (const section of SETTINGS_SECTIONS) expect(groupIds.has(section.group)).toBe(true);
  });

  it("分组顺序固定：通用 → 账号与同步 → 自动化 → 桌面与维护", () => {
    expect(SETTINGS_GROUPS.map((group) => group.title)).toEqual([
      "通用",
      "账号与同步",
      "自动化",
      "桌面与维护",
    ]);
    // 渲染顺序 = 注册表顺序：分组结果里各组的首次出现顺序必须与 SETTINGS_GROUPS 一致。
    const order = groupSettingsSections(SETTINGS_SECTIONS).map((entry) => entry.group.id);
    expect(order).toEqual(SETTINGS_GROUPS.map((group) => group.id));
  });

  it("搜索「开机」「令牌」「签到」都能命中对应节，清空后恢复全部", () => {
    const sections = SETTINGS_SECTIONS;

    const boot = filterSections(sections, "开机").map((section) => section.id);
    expect(boot).toContain("settings-startup");

    const token = filterSections(sections, "令牌").map((section) => section.id);
    expect(token).toContain("settings-auto-checkin");

    const checkin = filterSections(sections, "签到").map((section) => section.id);
    expect(checkin).toContain("settings-auto-checkin");

    // 清空（含纯空白）搜索 = 全部恢复。
    expect(filterSections(sections, "")).toHaveLength(sections.length);
    expect(filterSections(sections, "   ")).toHaveLength(sections.length);
  });

  it("搜索按标题、行标题与同义词匹配，且大小写不敏感", () => {
    const ids = (query: string) => filterSections(SETTINGS_SECTIONS, query).map((s) => s.id);
    // 节标题
    expect(ids("限额监听")).toContain("settings-rate-limit");
    // 行标题
    expect(ids("更新代理地址")).toEqual(["settings-updates"]);
    // 同义词 / 英文词
    expect(ids("license")).toEqual(["settings-about"]);
    expect(ids("GITHUB")).toContain("settings-updates");
    // 不存在的词 → 空结果（页面据此显示空态）
    expect(ids("这个词不该命中任何设置")).toEqual([]);
  });

  it("按环境过滤后仍然包含目录会用到的全部节（node 环境视作桌面端）", () => {
    const visible = visibleSettingsSections().map((section) => section.id);
    // node 环境 isDesktop() 为 true、isDemoMode() 为 false → 除「桌面专属」外全部可见。
    expect(visible).toContain("settings-startup");
    expect(visible).toContain("settings-updates");
    expect(visible).toContain("settings-bundle");
    expect(visible).toContain("settings-about");
  });

  it("诊断包一节可被「诊断 / 导出 / 日志 / 反馈 / issue / 排障」搜到，且所有形态都可见", () => {
    const ids = (query: string) => filterSections(SETTINGS_SECTIONS, query).map((s) => s.id);
    for (const query of ["诊断", "导出", "日志", "反馈", "issue", "排障"]) {
      expect(ids(query), `关键词「${query}」应命中诊断包`).toContain("settings-diagnostics");
    }
    // 网页版也要能复制 / 下载诊断包（服务端不落盘），因此不能是桌面专属可见性。
    const section = SETTINGS_SECTIONS.find((entry) => entry.id === "settings-diagnostics");
    expect(section?.availability).toBe("always");
    // 行标题必须与卡片里渲染的控件文字一致，否则搜不到眼前这一行。
    expect(section?.rows).toEqual(["预览诊断包", "导出诊断包"]);
  });
});
