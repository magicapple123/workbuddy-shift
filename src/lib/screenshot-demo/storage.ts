import type {
  StorageCategory,
  StorageScan,
  StorageSession,
  StorageSessionsPayload,
  StorageTrashPayload,
} from "../types";

import type { DemoContext } from "./accounts";

/** 演示模式支持的存储管理只读命令。 */
export const STORAGE_DEMO_COMMANDS = [
  "storage_scan",
  "storage_sessions",
  "storage_trash",
] as const;

const GB = 1024 ** 3;
const MB = 1024 ** 2;

function category(
  id: string,
  group: StorageCategory["group"],
  label: string,
  bytes: number,
  files: number,
  units: number,
  deletable: boolean,
  note: string,
  protectedReason: string | null = null,
): StorageCategory {
  return {
    id,
    group,
    label,
    path: `~/.workbuddy-shift/${id.replace(/^[a-z]+\.(cn|ai)\./, "")}`,
    bytes,
    files,
    units,
    deletable,
    note,
    protected: protectedReason,
  };
}

const CATEGORIES: StorageCategory[] = [
  category("tool.baselines", "tool", "无引用基线", 412 * MB, 585, 10, true, "共 8.4 GB · 仅删未被关联主表引用的快照"),
  category("tool.previews", "tool", "预览凭据", 300 * 1024, 200, 1, true, "一次性预览凭据，可安全重建"),
  category("tool.session-backups", "tool", "旧会话备份", 17.3 * MB, 96, 1, true, "旧版按时间戳留存，保留期已过"),
  category("tool.login-backups", "tool", "登录态备份", 6.2 * MB, 227, 227, true, "每次切换前留下的登录态副本，保留最新一份即可"),
  category("tool.cache", "tool", "统计缓存", 2.7 * MB, 3, 3, true, "下次刷新会重新生成"),
  category("tool.session-transactions", "tool", "会话操作备份", 0, 0, 0, false, "进行中/待恢复的会话操作材料", "由备份生命周期自动清理"),
  category("tool.trash", "tool", "回收站", 128 * MB, 412, 0, false, "在「回收站」页签里恢复或彻底删除"),
  category("client.cn.logs", "client", "国内版日志", 436 * MB, 2841, 1, true, "客户端缓存，会按需重建；要求客户端退出"),
  category("client.cn.traces", "client", "国内版运行追踪", 756 * MB, 1902, 1, true, "客户端缓存，会按需重建；要求客户端退出"),
  category("client.cn.file-tree-manifests", "client", "国内版文件树清单", 572 * MB, 3210, 1, true, "客户端缓存，会按需重建；要求客户端退出"),
  category("client.cn.file-history", "client", "国内版文件历史", 153 * MB, 8790, 1, true, "客户端缓存，会按需重建；要求客户端退出"),
  category("client.cn.incomplete-backup", "client", "国内版未完成备份", 174 * MB, 610, 1, true, "客户端中断的升级备份，可直接清理"),
  category("client.cn.clipboard-images", "client", "国内版剪贴板图片", 62 * MB, 380, 380, true, "共 96 MB · 只清 30 天前的文件"),
  category("client.cn.binaries", "client", "国内版运行时", 2.27 * GB, 512, 0, false, "客户端运行时文件，删除会破坏安装", "客户端运行时文件，删除会破坏安装"),
  category("client.cn.plugins", "client", "国内版插件", 375 * MB, 5120, 0, false, "已安装插件，删除会丢失扩展", "已安装插件，删除会丢失扩展"),
  category("client.cn.projects", "client", "国内版会话正文目录", 1.73 * GB, 524, 0, false, "会话正文在「会话资产」页签按会话清理", "会话正文在「会话资产」页签按会话清理"),
  category("client.cn.blobs", "client", "国内版图片附件", 143 * MB, 2201, 0, false, "多会话共享，只随会话按引用计数删零引用文件", "多会话共享，只随会话按引用计数删零引用文件"),
  category("client.cn.artifact-index", "client", "国内版产物索引", 69 * MB, 137, 0, false, "索引指向工作区真实文件", "索引指向工作区真实文件"),
];

/** 「其它 / 未分类」：让演示数据的「分类之和 = 根总量」与真实扫描同口径。 */
function otherCategory(group: StorageCategory["group"], suffix: string, bytes: number): StorageCategory {
  return {
    id: `${group}.${suffix}other`,
    group,
    label: group === "client" ? "国内版其它 / 未分类" : "其它 / 未分类",
    path: group === "client" ? "~/.workbuddy" : "~/.workbuddy-shift",
    bytes: Math.max(0, bytes),
    files: 0,
    units: 0,
    deletable: false,
    note: "根总量减去已列分类，含不可清理的大目录；只读展示，不提供清理",
    protected: "根总量减去已列分类；只读展示，不提供清理",
  };
}

function scan(): StorageScan {
  return {
    generatedAt: Date.now(),
    variant: "cn",
    cached: false,
    roots: [
      { id: "store", label: "工具数据", path: "~/.workbuddy-shift", exists: true, bytes: 84 * MB, files: 1119 },
      { id: "cn", label: "WorkBuddy（国内版）", path: "~/.workbuddy", exists: true, bytes: 7.2 * GB, files: 103837 },
      { id: "ai", label: "WorkBuddy（国际版）", path: "~/.workbuddy-ai", exists: false },
    ],
    totals: { bytes: 7.28 * GB, files: 104956 },
    categories: [
      ...CATEGORIES,
      otherCategory("tool", "", 84 * MB - CATEGORIES.filter((item) => item.group === "tool").reduce((sum, item) => sum + item.bytes, 0)),
      otherCategory("client", "cn.", 7.2 * GB - CATEGORIES.filter((item) => item.group === "client").reduce((sum, item) => sum + item.bytes, 0)),
    ],
    sessions: {
      counts: { live: 225, softDeleted: 31, orphan: 4, redundant: 18, present: 198, partial: 33, bodyMissing: 9 },
      bytes: 1.83 * GB,
      files: 7403,
    },
    trash: { entries: 3, bytes: 128 * MB, files: 412 },
    clientRunning: false,
    protectedPending: false,
    timings: { totalMs: 360, catalogMs: 240, sessionsMs: 110, trashMs: 10, probeMs: 40 },
    warnings: [
      "回收站不释放空间：只有「彻底删除」才会真正释放。",
      "删除会话后云端同步可能重新拉回，页面会给出提示。",
    ],
  };
}

function sessions(): StorageSessionsPayload {
  const now = Date.now();
  const rows: StorageSession[] = [
    ["0b3a1c1e-6b2d-4f6d-9c4f-8a1b2c3d4e5f", "修复会话分组显示", "D:/workbuddy-shift", "present", "live", false],
    ["1f0e2d3c-4b5a-6978-8796-a5b4c3d2e1f0", "存储管理页设计评审", "D:/WorkBuddy_data/spaces/2026_11_16_13-46-13", "partial", "live", true],
    ["2a1b0c9d-8e7f-6a5b-4c3d-2e1f0a9b8c7d", "Token 统计导出", "D:/WorkBuddy_data/spaces/2026_11_16_13-46-13", "bodyMissing", "live", false],
    ["3b2c1d0e-9f8a-7b6c-5d4e-3f2a1b0c9d8e", "已删除的临时会话", "D:/ResumeForge", "present", "softDeleted", false],
    ["4c3d2e1f-0a9b-8c7d-6e5f-4a3b2c1d0e9f", "仅剩残留文件", "D:/AICapabilityHubPro", "orphan", "none", false],
  ].map(([id, title, cwd, presence, rowStatus, redundant]) => ({
    id: id as string,
    uid: "demo-uid",
    title: title as string,
    cwd: cwd as string,
    updatedAt: now - 3600_000,
    variant: "cn",
    rowStatus: rowStatus as StorageSession["rowStatus"],
    presence: presence as StorageSession["presence"],
    redundant: Boolean(redundant),
    groupId: redundant ? "demo-group" : null,
    liveCopies: redundant ? 3 : 0,
    bytes: 18 * MB,
    files: 42,
    entries: [
      { kind: "body", label: "会话正文", path: `${cwd}/projects/ws/${id}.jsonl`, bytes: 12 * MB, files: 1, exists: presence !== "bodyMissing" },
      { kind: "sidecar", label: "附属目录", path: `${cwd}/projects/ws/${id}/`, bytes: 4 * MB, files: 30, exists: true },
      { kind: "artifact-index", label: "产物索引", path: `${cwd}/artifact-index/${id}.json`, bytes: 20 * 1024, files: 1, exists: true },
      { kind: "file-history", label: "文件历史", path: `${cwd}/file-history/${id}/`, bytes: 1.8 * MB, files: 9, exists: true },
      { kind: "changes-index", label: "变更索引", path: `${cwd}/changes-index/${id}.json`, bytes: 8 * 1024, files: 1, exists: true },
      { kind: "changes-detail", label: "变更明细", path: `${cwd}/changes-detail/${id}/`, bytes: 120 * 1024, files: 2, exists: presence === "present" },
    ],
  }));
  // 演示数据里同组同账号有 3 份存活副本 → 留 1 份，可清理 2 条。
  return { variant: "cn", clientRunning: false, redundantCleanable: 2, sessions: rows };
}

function trash(): StorageTrashPayload {
  const now = Date.now();
  return {
    entries: [
      {
        id: "20261117-193012",
        createdAt: now - 5400_000,
        bytes: 96 * MB,
        files: 300,
        rows: 1,
        dbBackups: ["~/.workbuddy-shift/backups/storage/20261117-193012/cn.db"],
        items: [
          {
            rootId: "cn",
            originalPath: "~/.workbuddy/logs/app.log",
            bytes: 96 * MB,
            files: 300,
            mtime: now - 86_400_000,
            category: "client.cn.logs",
            sessionId: null,
            kind: "dir",
            result: "moved",
            state: "restorable",
          },
        ],
      },
      {
        id: "20261116-101500",
        createdAt: now - 104_400_000,
        bytes: 32 * MB,
        files: 112,
        rows: 0,
        dbBackups: [],
        items: [],
      },
    ],
    problems: [],
    bytes: 128 * MB,
    files: 412,
  };
}

/** 演示分发：未命中返回 undefined，由上层继续尝试其它域。 */
export function storageDemoResponse(
  command: string,
  _ctx: DemoContext,
  _args?: Record<string, unknown>,
): unknown {
  switch (command) {
    case "storage_scan":
      return scan();
    case "storage_sessions":
      return sessions();
    case "storage_trash":
      return trash();
    default:
      return undefined;
  }
}
