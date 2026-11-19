//! Client-scoped session group directory and explicit-member operations.
//!
//! This module is the group-management boundary used by Tauri and WebUI. It reads the
//! existing namespace stores and delegates every write to the client-specific session
//! kernels; it never resolves an operation source from a client's active login state.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::modules::session::{SessionPaths, SyncSelection};
use crate::modules::session_link::{
    ContentState, LinkGroup, LinkMember, MemberState, StoreState, SyncVerdict,
};
use crate::modules::vscode_session::{
    CopyItem, SessionStoreSpec, CODEBUDDY_IDE_STORE, JETBRAINS_STORE, VSCODE_STORE,
};
use crate::modules::{
    account, codebuddy_ide_session, codebuddy_ide_session_sync, config, jetbrains, process,
    session, session_link, variant::WbVariant, vscode_ext, vscode_session, vscode_session_sync,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SessionClient {
    Workbuddy,
    CodebuddyIde,
    VscodeExt,
    Jetbrains,
}

impl SessionClient {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "workbuddy" => Ok(Self::Workbuddy),
            "codebuddyIde" => Ok(Self::CodebuddyIde),
            "vscodeExt" => Ok(Self::VscodeExt),
            "jetbrains" => Ok(Self::Jetbrains),
            _ => Err("不支持的会话客户端".to_string()),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Workbuddy => "workbuddy",
            Self::CodebuddyIde => "codebuddyIde",
            Self::VscodeExt => "vscodeExt",
            Self::Jetbrains => "jetbrains",
        }
    }

    fn store_paths(self) -> SessionPaths {
        match self {
            Self::Workbuddy => SessionPaths::for_variant(WbVariant::Cn),
            Self::CodebuddyIde => SessionPaths::for_codebuddy_ide(),
            Self::VscodeExt => SessionPaths::for_vscode_ext(),
            Self::Jetbrains => SessionPaths::for_jetbrains(),
        }
    }

    fn data_root(self) -> Option<PathBuf> {
        match self {
            Self::Workbuddy => None,
            Self::CodebuddyIde => codebuddy_ide_session::ide_data_root(),
            Self::VscodeExt => vscode_session::ext_data_root(),
            Self::Jetbrains => vscode_session::store_data_root(JETBRAINS_STORE),
        }
    }

    fn store_spec(self) -> Option<SessionStoreSpec> {
        match self {
            Self::Workbuddy => None,
            Self::CodebuddyIde => Some(CODEBUDDY_IDE_STORE),
            Self::VscodeExt => Some(VSCODE_STORE),
            Self::Jetbrains => Some(JETBRAINS_STORE),
        }
    }

    /// 是否「编辑器插件型」客户端：会话行在工作区索引里，写盘前需要关闭宿主进程。
    pub(super) fn is_editor_plugin(self) -> bool {
        matches!(self, Self::VscodeExt | Self::Jetbrains)
    }

    fn validate_scope(self, scope: Option<WbVariant>) -> Result<(), String> {
        match (self, scope) {
            (Self::CodebuddyIde, Some(_)) => Ok(()),
            (Self::CodebuddyIde, None) => {
                Err("CodeBuddy IDE 请求必须指定 cn 或 ai 档位".to_string())
            }
            // 其余三个客户端都不接受档位筛选（插件 / JetBrains 的存储与地区无关）。
            (_, None) => Ok(()),
            (_, Some(_)) => Err("只有 CodeBuddy IDE 支持档位筛选".to_string()),
        }
    }
}

pub fn parse_variant_scope(raw: Option<&str>) -> Result<Option<WbVariant>, String> {
    match raw.map(str::trim) {
        None | Some("") => Ok(None),
        Some("cn") => Ok(Some(WbVariant::Cn)),
        Some("ai") => Ok(Some(WbVariant::Ai)),
        Some(_) => Err("variantScope 只支持 cn 或 ai".to_string()),
    }
}

use graph::*;
use jetbrains_guard::*;
use liveness::*;
use members::*;
use sync::*;
use view::*;

mod graph;
mod jetbrains_guard;
mod liveness;
mod maintenance;
mod members;
mod sync;
mod view;

pub use graph::{sync_safe_batch, sync_unify_batch, GroupUnifyTarget};
pub use maintenance::prune_dead_groups;
pub use members::{
    add_member, copy_linked_sessions, delete_group, list_account_sessions, remove_member,
};
pub use sync::{jetbrains_restart_precheck, preview_pair, sync_pair, vscode_restart_precheck};
pub use view::{detail, list};

/// 插件侧同步入口运行中的报错文案（`restart=false`，逐字保持既有文案）。
pub(super) const VSCODE_SYNC_RUNNING_HINT: &str =
    "检测到 VS Code 正在运行，请先完全退出后再同步会话。";

/// 插件侧「添加关联账号」运行中的报错文案（`restart=false`，逐字保持既有文案）。
pub(super) const VSCODE_ADD_MEMBER_RUNNING_HINT: &str =
    "检测到 VS Code 正在运行，请先完全退出后再添加关联账号。";

/// JetBrains 侧同步入口运行中的报错文案（`restart=false`）。
pub(super) const JETBRAINS_SYNC_RUNNING_HINT: &str =
    "检测到 JetBrains IDE（IDEA / PyCharm）正在运行，请先完全退出后再同步会话。";

/// JetBrains 侧「添加关联账号」运行中的报错文案（`restart=false`）。
pub(super) const JETBRAINS_ADD_MEMBER_RUNNING_HINT: &str =
    "检测到 JetBrains IDE（IDEA / PyCharm）正在运行，请先完全退出后再添加关联账号。";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::session_link::{
        full_digest_of, normalize_jsonl, ContentSnapshot, SyncMode,
    };

    include!("tests/editor.rs");
    include!("tests/jetbrains.rs");
    include!("tests/maintenance.rs");
    include!("tests/plugin_liveness.rs");
    include!("tests/members.rs");
    include!("tests/graph.rs");
}
