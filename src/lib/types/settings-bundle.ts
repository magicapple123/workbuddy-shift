/**
 * 设置包（四份自动化配置的打包结果）与工具数据目录的契约类型。
 *
 * 形状由 `crates/wb-shift-core/src/modules/config/configs/bundle.rs` 固定：
 * `{ version, exportedAt, configs: { checkin, travel, rotate, rateLimit } }`。
 * 桌面端与 WebUI 都直接返回 core 的 `serde_json::Value`，这里只补上前端的静态形状，
 * 不做任何字段改名——导出文件是用户会保存、会手改、会跨机器搬运的**稳定契约**，
 * 键名必须与磁盘配置文件逐字一致（`snake_case`）。
 *
 * 刻意不含更新（GitHub）配置：那份带 `proxy`，可能包含代理口令，明文导出等于把
 * 用户的代理链路写进一个会被随手分享的文件（理由同 core 的 `bundle.rs` 文件头）。
 */

import type { AutoRotateConfig, CheckinConfig, RateLimitConfig, TravelConfig } from "./stats";

/** 允许打包的四份配置的键；顺序与 core 的 `SETTINGS_BUNDLE_KEYS` 一致。 */
export type SettingsBundleKey = "checkin" | "travel" | "rotate" | "rateLimit";

/** 导入时严格比对的版本号（与 core 的 `SETTINGS_BUNDLE_VERSION` 同步）。 */
export const SETTINGS_BUNDLE_VERSION = 1;

/** 一份完整的设置包。导出总是四份齐全；导入允许只带其中几份。 */
export interface SettingsBundle {
  version: number;
  /** 毫秒时间戳；只用于展示「这份包是什么时候导出的」。 */
  exportedAt: number;
  configs: {
    checkin?: CheckinConfig;
    travel?: TravelConfig;
    rotate?: AutoRotateConfig;
    rateLimit?: RateLimitConfig;
  };
}

/** 导入结果：写成功的键、备份目录（写前快照）、导入时刻。 */
export interface SettingsImportReport {
  imported: SettingsBundleKey[];
  /** 写前备份目录（`backups/settings-import-<毫秒>`），失败时据它人工回滚。 */
  backupDir: string;
  importedAt: number;
}

/** `GET /api/data-dir` / `get_data_dir` 的回包：工具数据目录绝对路径。 */
export interface DataDirInfo {
  path: string;
}
