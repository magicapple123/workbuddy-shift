//! CodeBuddy IDE「关联会话」：复制后登记、切换预览与增量同步。
//!
//! 与 VS Code 插件侧**共用同一份实现**（`vscode_session_sync` 的 `*_in` 变体）；国内版与国际版
//! IDE 也共用同一份（档位差异只有「来源身份读法」与「运行态判定」，见
//! [`codebuddy_ide_session::IdeFlavor`]）。本模块只做三件与目标相关的事：解析数据根与来源身份
//! （当前 IDE 登录 uid）、检查 IDE 运行态、固定 [`SessionPaths::for_codebuddy_ide`] 命名空间。
//! 存储隔离与判定语义见 `session-links.md`。

use serde_json::{json, Value};
use std::path::Path;

use crate::modules::codebuddy_ide_session::{self, IdeFlavor};
use crate::modules::session::{SessionPaths, SyncSelection};
use crate::modules::variant::WbVariant;
use crate::modules::vscode_session::CODEBUDDY_IDE_STORE;
use crate::modules::vscode_session_sync;

/// 为本次复制成功的会话登记「源 ↔ 副本」关联，返回逐条失败原因。
///
/// **登记失败不回滚复制**：复制已完成、文件已落盘；失败原因由调用方并入报告 `linkErrors[]`。
pub fn register_copied_sessions(
    root: &Path,
    paths: &SessionPaths,
    variant: WbVariant,
    report: &Value,
) -> Vec<Value> {
    // 两个 IDE 共用一张关联表：只复用同 variant 的组，不把国际版登记并进国内版的组。
    vscode_session_sync::register_copied_sessions_isolated(
        CODEBUDDY_IDE_STORE,
        root,
        paths,
        variant,
        report,
    )
}

/// 档位对应的关联组 variant：国内版 `cn`，国际版 `ai`。
fn link_variant(flavor: IdeFlavor) -> WbVariant {
    match flavor {
        IdeFlavor::Cn => WbVariant::Cn,
        IdeFlavor::Intl => WbVariant::Ai,
    }
}

/// 来源账号 uid：当前 IDE 登录 secret（回退本地状态文件）；非法 uid（`default` / `Public` / 路径穿越）视为未登录。
fn active_source_uid(flavor: IdeFlavor) -> Result<String, String> {
    flavor.active_uid().ok_or_else(|| {
        "未检测到 CodeBuddy IDE 当前登录账号，无法同步会话。请先在 CodeBuddy IDE 中登录后重试。"
            .to_string()
    })
}

mod preview;
mod sync;

pub use preview::{links_preview, links_preview_intl};
pub(crate) use sync::sync_selected_for;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::session_link::{self, MemberState, StoreState, SyncMode};
    use crate::modules::vscode_session::{history_root_in, read_json, CopyItem};
    use std::path::{Path, PathBuf};

    include!("tests/sync.rs");
}
