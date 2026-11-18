//! VS Code CodeBuddy 插件「关联会话」：复制后登记、切换预览与增量同步（design §5 / §6）。
//!
//! 与 WorkBuddy 侧**分层同构但不共用编排**：内核（关联表 / 配对基线 / `decide_sync` /
//! 预览凭据 / 存储锁）复用 [`crate::modules::session_link`]，存储按
//! [`SessionPaths::for_vscode_ext`] 的命名空间与 WorkBuddy 完全隔离；内容身份与写入形态
//! 则是插件专属的（一条消息一个文件、按记录顺序摘要、整体重建）。
//!
//! 触发时机（design §7）：只在切换 wrapper 内、编辑器**已关闭**之后执行；顺序固定为
//! 「复制 → 登记关联 → 执行同步 → 写凭证并重开编辑器」。**没有**操作日志、中断自动恢复
//! 与新增跨进程锁（D2 / D7）：中断后靠「重跑收敛」，覆盖模式靠整目录备份收场。
//!
//! 写入强度：会话文件与索引走 [`session_backup::durable_write_str`]（tmp + fsync + rename +
//! 父目录持久化，Windows 用可写句柄），备份与回滚沿用复制路径的既有原语。

use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::modules::account;
use crate::modules::config::{now_ms, utc_iso};
use crate::modules::session::{SessionPaths, SyncSelection, REASON_PREVIEW_STALE};
use crate::modules::session_backup;
use crate::modules::session_link::{
    self, BaselineState, ContentSnapshot, ContentState, LinkGroup, LinkMember, LinkStore,
    MemberState, PreviewBinding, PreviewMemberBinding, StoreState, SyncMode, SyncVerdict,
    NORMALIZATION_VERSION,
};
use crate::modules::variant::WbVariant;
use crate::modules::vscode_ext;
use crate::modules::vscode_session;
use crate::modules::vscode_session::{SessionStoreSpec, VSCODE_STORE};
use crate::modules::vscode_session_link;

/// 派生 id 冲突时的 salt 重试上限（design §6.1）。
const SALT_LIMIT: u32 = 8;
/// 整目录备份的后缀：`<utc_iso>/<workspaceHash>/<targetConvId>-overwrite/`。
const OVERWRITE_SUFFIX: &str = "-overwrite";

use backup::*;
use exec::*;
use index::*;
use liveness::*;
use plan::*;
use preview::*;
use register::*;

mod backup;
mod exec;
mod index;
mod liveness;
mod plan;
mod preview;
mod register;

pub(crate) use backup::{restore_dir, rollback_index};
pub(crate) use index::ConversationIndexState;
pub(crate) use liveness::{LivenessJudge, SessionLiveness};
pub use plan::{sync_selected, sync_selected_at, sync_selected_in};
pub(crate) use plan::{sync_selected_in_for_variant, SyncItemPlan};
pub(crate) use preview::links_preview_in_for_variant;
pub use preview::{links_preview, links_preview_at, links_preview_in};
pub use register::{register_copied_sessions, register_copied_sessions_in};
pub(crate) use register::{register_copied_sessions_isolated, session_location_and_content};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::vscode_session::{copy_sessions_in, CopyItem};
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    include!("tests/fixtures.rs");
    include!("tests/index_state.rs");
    include!("tests/liveness.rs");
    include!("tests/sync.rs");
    include!("tests/overwrite.rs");
    include!("tests/restore.rs");
}
