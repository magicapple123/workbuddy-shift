/**
 * 设置页的**静态节注册表**：分组、标题、搜索关键词、环境可见性。
 *
 * 为什么要注册表而不是扫 DOM：目录与搜索都要知道「这一节叫什么、有哪些行」。
 * 用 `document.body.innerText` 之类的 DOM 文本当索引会跟着渲染状态漂移
 * （折叠内容读不到、加载失败时标题缺失、演示模式少一张卡就整节消失），
 * 也无法在渲染前决定「要不要出现在目录里」。这里写死一份，页面渲染与目录/搜索
 * 都读同一份数据，改标题只改一处。
 *
 * `id` 是**稳定契约**：已有深链（`/settings#settings-startup`）与文档都引用它，
 * 改名等于让旧链接失效，所以新增节只能追加，不能重命名已发布的 id。
 */

import * as api from "@/lib/api";

/** 四个固定分组；顺序即渲染顺序与目录顺序。 */
export type SettingsGroupId = "general" | "account" | "automation" | "desktop";

export interface SettingsGroupSpec {
  id: SettingsGroupId;
  title: string;
}

export const SETTINGS_GROUPS: SettingsGroupSpec[] = [
  { id: "general", title: "通用" },
  { id: "account", title: "账号与同步" },
  { id: "automation", title: "自动化" },
  { id: "desktop", title: "桌面与维护" },
];

/**
 * 环境可见性：
 * - `always`：所有形态都渲染；
 * - `not-demo`：演示模式不渲染（会触发写操作 / 需要真实账号的卡）；
 * - `desktop-not-demo`：只有桌面正式版渲染（会话悬浮窗）；
 * - `desktop-or-demo`：桌面版与演示页渲染，WebUI 不渲染（开机自启、自动更新）。
 *
 * 四个值与 `index.tsx` 里原有的一串三元判断逐条对应；现在只在这里写一次，
 * 目录、搜索与正文都按同一份结果过滤，不会出现「目录里有、页面上没有」。
 */
export type SettingsAvailability = "always" | "not-demo" | "desktop-not-demo" | "desktop-or-demo";

export interface SettingsSectionSpec {
  /** 锚点 id（`SettingsGroup` 的 `id` 与 `#hash` 深链都用它）。 */
  id: string;
  title: string;
  group: SettingsGroupId;
  /** 卡片内的行标题：搜索命中它们就等于命中这一节（与标题同权）。 */
  rows: string[];
  /** 同义词 / 字段名 / 英文词，补足用户可能输入但行标题里没有的词（如「令牌」）。 */
  keywords: string[];
  availability: SettingsAvailability;
}

/**
 * 全部节（含当前环境下不该渲染的）。顺序即页面渲染顺序。
 *
 * 行标题必须与卡片里实际渲染的控件文字一致，否则搜索会「搜不到眼前这一行」。
 */
export const SETTINGS_SECTIONS: SettingsSectionSpec[] = [
  {
    id: "settings-appearance",
    title: "外观",
    group: "general",
    rows: ["主题", "显示关联会话菜单"],
    keywords: ["外观", "主题", "深色", "浅色", "跟随系统", "导航", "theme"],
    availability: "always",
  },
  {
    id: "settings-tools",
    title: "支持工具",
    group: "general",
    rows: ["WorkBuddy", "CodeBuddy IDE", "CodeBuddy CLI", "VS Code 插件", "JetBrains 插件"],
    keywords: ["工具", "客户端", "入口", "隐藏", "灰度", "插件"],
    availability: "always",
  },
  {
    id: "settings-archive-sync",
    title: "切号时同步归档",
    group: "account",
    rows: ["切换账号时默认勾选归档同步"],
    keywords: ["切号", "同步", "归档", "会话", "默认勾选"],
    availability: "always",
  },
  {
    id: "settings-sync",
    title: "设置跨账号同步",
    group: "account",
    rows: ["启用设置同步", "切号时自动同步", "同步预览", "备份与还原"],
    keywords: ["设置", "跨账号", "同步", "主题", "工作区", "scoped", "备份", "还原"],
    availability: "not-demo",
  },
  {
    id: "settings-automation-sync",
    title: "定时任务",
    group: "account",
    rows: ["切号时同步 WorkBuddy 定时任务"],
    keywords: ["定时任务", "自动化", "任务", "切号", "任务搬运", "暂停"],
    availability: "not-demo",
  },
  {
    id: "settings-permission",
    title: "权限检测",
    group: "account",
    rows: ["检测权限", "打开完全磁盘访问", "认证文件路径"],
    keywords: ["权限", "认证文件", "磁盘访问", "auth.json", "authFile", "排障"],
    availability: "always",
  },
  {
    id: "settings-auto-checkin",
    title: "自动签到",
    group: "automation",
    rows: [
      "启用自动签到",
      "签到时间段",
      "保活阈值",
      "惰性刷新",
      "参与自动签到的账号",
      "签到日志",
      "派猫猫旅行",
    ],
    keywords: ["签到", "保活", "令牌", "刷新", "积分", "旅行", "派猫猫", "恢复默认"],
    availability: "not-demo",
  },
  {
    id: "settings-auto-rotate",
    title: "CodeBuddy CLI 自动轮换",
    group: "automation",
    rows: [
      "启用自动轮换",
      "检查间隔",
      "切换冷却",
      "到期差异阈值",
      "到期紧迫阈值",
      "最小剩余积分",
      "轮换日志",
    ],
    keywords: ["轮换", "切换", "cli", "到期", "积分", "恢复默认"],
    availability: "always",
  },
  {
    id: "settings-rate-limit",
    title: "限额监听",
    group: "automation",
    rows: [
      "启用限额监听",
      "扫描 CodeBuddy IDE 日志",
      "跨客户端限额提示",
      "接入客户端 hook",
      "诊断信息（日志探针）",
    ],
    keywords: ["限额", "限流", "429", "hook", "日志", "跨客户端", "恢复默认"],
    availability: "always",
  },
  {
    id: "settings-companion",
    title: "会话悬浮窗",
    group: "desktop",
    rows: ["启用会话悬浮窗", "悬浮窗设置"],
    keywords: ["悬浮窗", "悬浮栏", "companion", "桌面"],
    availability: "desktop-not-demo",
  },
  {
    id: "settings-startup",
    title: "启动设置",
    group: "desktop",
    rows: ["开机时静默启动到托盘"],
    keywords: ["开机", "启动", "自启", "登录项", "托盘", "静默"],
    availability: "desktop-or-demo",
  },
  {
    id: "settings-notifications",
    title: "通知历史",
    group: "desktop",
    rows: ["应用内提示存档", "清空"],
    keywords: ["通知", "提示", "历史", "存档", "清空"],
    availability: "always",
  },
  {
    id: "settings-error-log",
    title: "错误日志",
    group: "desktop",
    rows: ["打开日志位置"],
    keywords: ["错误", "日志", "崩溃", "排障", "反馈"],
    availability: "always",
  },
  {
    id: "settings-updates",
    title: "自动更新",
    group: "desktop",
    rows: ["当前版本", "启动时检查更新", "更新代理地址", "检查更新"],
    keywords: ["更新", "升级", "版本", "github", "代理", "release"],
    availability: "desktop-or-demo",
  },
  {
    id: "settings-bundle",
    title: "设置包",
    group: "desktop",
    rows: ["导出设置包", "导入设置包"],
    keywords: ["导出", "导入", "备份", "恢复", "设置包", "迁移", "换机"],
    availability: "always",
  },
  {
    id: "settings-about",
    title: "关于",
    group: "desktop",
    rows: ["版本", "工具数据目录", "许可证", "问题反馈"],
    keywords: ["关于", "版本", "数据目录", "许可证", "license", "issue", "反馈", "开源"],
    availability: "always",
  },
];

/** 该可见性在当前环境下是否成立（`index.tsx` 渲染与目录过滤共用这一个判断）。 */
export function sectionVisible(availability: SettingsAvailability): boolean {
  switch (availability) {
    case "not-demo":
      return !api.isDemoMode();
    case "desktop-not-demo":
      return api.isDesktop() && !api.isDemoMode();
    case "desktop-or-demo":
      return api.isDesktop() || api.isDemoMode();
    default:
      return true;
  }
}

/** 当前环境下渲染的节（顺序同注册表）。 */
export function visibleSettingsSections(): SettingsSectionSpec[] {
  return SETTINGS_SECTIONS.filter((section) => sectionVisible(section.availability));
}

/** 搜索归一化：去首尾空白 + 小写。中文没有大小写，但英文关键词要能大小写不敏感地命中。 */
export function normalizeQuery(raw: string): string {
  return raw.trim().toLowerCase();
}

/**
 * 单节是否命中关键词。
 *
 * 标题、行标题、同义词三者**同权**：用户搜「开机」命中的是行标题，
 * 搜「外观」命中的是节标题，搜「令牌」命中的是同义词，三者都不该被区别对待。
 */
export function sectionMatches(section: SettingsSectionSpec, query: string): boolean {
  const needle = normalizeQuery(query);
  if (!needle) return true;
  const haystack = [section.title, ...section.rows, ...section.keywords]
    .join("\n")
    .toLowerCase();
  return haystack.includes(needle);
}

/** 按关键词过滤；空关键词返回全部（= 清空搜索即恢复）。 */
export function filterSections(
  sections: SettingsSectionSpec[],
  query: string,
): SettingsSectionSpec[] {
  const needle = normalizeQuery(query);
  if (!needle) return sections;
  return sections.filter((section) => sectionMatches(section, needle));
}

export interface SettingsGrouped {
  group: SettingsGroupSpec;
  sections: SettingsSectionSpec[];
}

/** 按固定分组顺序分组；空组不返回（搜索把某组全过滤掉时，组标题也不该留）。 */
export function groupSettingsSections(sections: SettingsSectionSpec[]): SettingsGrouped[] {
  const grouped: SettingsGrouped[] = [];
  for (const group of SETTINGS_GROUPS) {
    const own = sections.filter((section) => section.group === group.id);
    if (own.length > 0) grouped.push({ group, sections: own });
  }
  return grouped;
}
