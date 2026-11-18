

/**
 * 应用内通知存档条目：toast 只存活几秒，这里保存最近 100 条供事后回看
 * （支持排障与验收核对，例如切号成功后到底提示了什么）。
 */
export interface AppNotification {
  level: "success" | "error" | "warning" | "info";
  title: string;
  description?: string;
  /** 毫秒时间戳。 */
  at: number;
}

/**
 * 错误日志来源（`~/.workbuddy-shift/error.log` 的 `kind` 字段）：
 * 渲染崩溃 / 未捕获异常或 Promise 拒绝 / 后端错误。
 */
export type ErrorLogKind = "frontend_crash" | "frontend_unhandled" | "backend";

export interface GithubConfig {
  owner?: string;
  repo?: string;
  proxy?: string;
}

export interface UpdateInfo {
  ok: boolean;
  current?: string;
  latest?: string;
  latestTag?: string;
  hasUpdate?: boolean;
  releaseName?: string;
  releaseUrl?: string;
  publishedAt?: string;
  error?: string;
  message?: string;
}

/** 统一更新服务的阶段（Rust 状态机，经 `update-state` 事件推送）。 */
export type UpdatePhase =
  | "idle"
  | "checking"
  | "upToDate"
  | "available"
  | "downloading"
  | "readyToRestart"
  | "error";

/**
 * 更新状态快照：托盘菜单与前端弹窗显示同一阶段（单一真相源在 Rust）。
 *
 * `latest` 无 `v` 前缀；`percent` 在下载总量未知时为 null；`message` 是错误 / 提示文案。
 */
export interface UpdateSnapshot {
  phase: UpdatePhase;
  latest: string | null;
  percent: number | null;
  message: string | null;
  checkedAt: number | null;
}
