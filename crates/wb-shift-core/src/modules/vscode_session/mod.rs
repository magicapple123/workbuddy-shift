//! 扩展数据仓（VS Code 内 CodeBuddy 插件 / CodeBuddy IDE 桌面客户端）会话复制。
//!
//! 两棵树同根同构，只差客户端段目录名（见 [`SessionStoreSpec`]）：
//! - VS Code 插件：`<root>/<uid>/VSCode/<uid>/history`（[`VSCODE_STORE`]）；
//! - CodeBuddy IDE：`<root>/<uid>/CodeBuddyIDE/<uid>/history`（[`CODEBUDDY_IDE_STORE`]）。
//!
//! 切换账号时，可把「当前账号」的会话（正文 + 索引）复制到「目标账号」目录，
//! 使目标账号重新打开客户端后能在对应工作区看到并续聊。与桌面版 `session` 模块（`modules/session/`）同构：
//! **本地目录复制 + 合并工作区索引**；插件侧副本一律取新 id（[`CopyIdPolicy::AlwaysNew`]），
//! IDE 侧默认沿用源 id、仅冲突时重随机（[`CopyIdPolicy::KeepUnlessConflict`]）。
//!
//! 存储布局（Windows 实测）：
//! ```text
//! %LOCALAPPDATA%\CodeBuddyExtension\Data\<uid>\VSCode\<uid>\
//!   history\<md5(工作区)>\
//!     index.json              # 工作区级会话索引 {conversations:[...], current}
//!     .index_bak.json         # 同结构备份
//!     <conversationId>\       # 32 位小写 hex
//!       index.json            # {messages:[...], requests:[...]}
//!       messages\<messageId>.json
//!       assets\*
//! ```
//!
//! 约定与约束：
//! - 会话 / 消息 / 请求 id 一律 **32 位小写 hex**（扩展 `generateId()` 产物），
//!   不得使用带连字符的 UUID。
//! - **只写目标 uid 目录**；源 uid 目录只读，绝不修改或删除。
//! - 显式排除 `default\` / `Public\` 目录（结构不同，见 [`is_safe_uid`]），
//!   uid 只能来自账号库 / 扩展登录态，不接受用户任意输入。
//! - 会话复制本体要求 VS Code 未运行（复用 [`vscode_ext::is_vscode_running`]）；
//!   由 [`switch_vscode_ext_with_copy`] 编排时，`restart = true` 会先自动关闭编辑器再复制。
//! - 单条原子性：先写 `<ws>\.tmp-<newId>\` 再 `rename` 成 `<ws>\<newId>\`；
//!   索引合并用「读-改-写 + `atomic_write`」，写前把将被改的索引备份到
//!   `backup_dir()/vscode-sessions/<utc_iso>/`。
//! - 失败逐条隔离：删临时/成品目录 + 回退该工作区索引，记入 `errors[]`，继续其余条目。
//!
//! macOS / Linux 的数据根**未实测**：按同一相对布局 best-effort 推导
//! （见 [`ext_data_root_candidates`]），仅在对应候选路径真实存在时才使用。

use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::modules::account::{self, get_str};
use crate::modules::config::{atomic_write, backup_dir, utc_iso};
use crate::modules::session::{SessionPaths, SyncSelection};
use crate::modules::vscode_ext;
use crate::modules::vscode_session_sync;

/// 会话历史目录名。
const HISTORY_DIR: &str = "history";

/// 扩展数据仓的分段与备份命名：区分「VS Code 插件」与「CodeBuddy IDE」两棵同构的会话树。
///
/// 布局：`<平台本地数据根>/<app_dir>/Data/<uid>/<client_dir>/<uid>/history/<工作区目录名>/`。
/// 工作区目录名是**不透明字符串**（实现只做目录枚举与按名复制，不重算 md5）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SessionStoreSpec {
    /// 应用数据目录名（`CodeBuddyExtension`）。
    pub app_dir: &'static str,
    /// 客户端段目录名（`VSCode` / `CodeBuddyIDE`）。
    pub client_dir: &'static str,
    /// 会话索引备份根目录名：`<工具存储根>/backups/<backup_kind>/<utc_iso>/`。
    pub backup_kind: &'static str,
}

/// VS Code 内 CodeBuddy 插件（`tencent-cloud.coding-copilot`）的数据仓。
pub const VSCODE_STORE: SessionStoreSpec = SessionStoreSpec {
    app_dir: "CodeBuddyExtension",
    client_dir: "VSCode",
    backup_kind: "vscode-sessions",
};

/// CodeBuddy IDE（国内版桌面客户端）的数据仓：与插件同根，只有客户端段不同。
pub const CODEBUDDY_IDE_STORE: SessionStoreSpec = SessionStoreSpec {
    app_dir: "CodeBuddyExtension",
    client_dir: "CodeBuddyIDE",
    backup_kind: "codebuddy-ide-sessions",
};

/// JetBrains IDE 内 CodeBuddy 插件的数据仓（IDEA / PyCharm 共用同一份）。
///
/// 与 VS Code 插件**逐字同构**，只有客户端段目录名不同（`JetBrains`）；因此索引读取、
/// 内容摘要、复制、登记、预览、增量同步全部走同一套 spec 参数化内核，不需要第二份实现。
pub const JETBRAINS_STORE: SessionStoreSpec = SessionStoreSpec {
    app_dir: "CodeBuddyExtension",
    client_dir: "JetBrains",
    backup_kind: "jetbrains-sessions",
};

/// 待复制的会话引用（工作区 hash + 会话 id）。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CopyItem {
    /// 工作区目录名 = `md5(工作区)`（32 位小写 hex）。
    pub workspace_hash: String,
    /// 会话 id（32 位小写 hex）。
    pub conversation_id: String,
}

/// 副本 id 策略：决定复制体沿用源 id 还是取新 id。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CopyIdPolicy {
    /// 副本一律取新 id（VS Code 插件：扩展按 id 归类，复制体必须与源区分）。
    AlwaysNew,
    /// 默认沿用源 id，仅当目标工作区已存在同 id 时取新 id 并重写引用
    /// （CodeBuddy IDE：真机实测沿用 id 的目标账号可直接加载；冲突重随机避免覆盖既有会话）。
    KeepUnlessConflict,
}

/// 复制策略：id 生成方式与目标工作区索引的写入范围。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CopyOptions {
    /// 副本 id 策略（见 [`CopyIdPolicy`]）。
    pub id_policy: CopyIdPolicy,
    /// 是否把合并后的索引同步写入目标工作区的 `.index_bak.json`。
    ///
    /// 插件侧写（扩展自用的索引备份需要与主索引一致）；IDE 侧**不写**——目标侧原有的
    /// `.index_bak.json` 保持原样，不存在也不新建（见 codebuddy-ide-target 契约）。
    pub write_workspace_index_backup: bool,
}

/// VS Code 插件侧的复制策略（历史行为：恒取新 id + 同步工作区索引备份）。
pub(crate) const VSCODE_COPY: CopyOptions = CopyOptions {
    id_policy: CopyIdPolicy::AlwaysNew,
    write_workspace_index_backup: true,
};

/// CodeBuddy IDE 侧的复制策略（沿用 id，冲突才重随机；不动目标侧索引备份）。
pub(crate) const IDE_COPY: CopyOptions = CopyOptions {
    id_policy: CopyIdPolicy::KeepUnlessConflict,
    write_workspace_index_backup: false,
};

mod copy;
mod index_merge;
mod list;
mod paths;
mod remap;

#[allow(unused_imports)]
pub(crate) use copy::copy_sessions_in_with;
pub use copy::{copy_sessions_in, copy_vscode_sessions, switch_vscode_ext_with_copy};
pub use list::{list_sessions_in, list_sessions_in_store, list_vscode_sessions};
#[allow(unused_imports)]
pub(crate) use paths::{
    copy_dir_recursive, is_hex32, is_safe_uid, read_json, remove_dir_all_if_exists,
};
pub use paths::{
    ext_data_root, ext_data_root_candidates, history_root, history_root_in, store_data_root,
    store_data_root_candidates, uid_data_dir_in,
};
#[allow(unused_imports)]
pub(crate) use remap::{
    remap_plan_from_maps, replace_exact_ids, replace_ids_in_extra, write_conversation, RemapPlan,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(unused_imports)]
    use super::copy::*;
    #[allow(unused_imports)]
    use super::index_merge::*;
    #[allow(unused_imports)]
    use super::list::*;
    #[allow(unused_imports)]
    use super::paths::*;
    #[allow(unused_imports)]
    use super::remap::*;
    include!("tests/copy.rs");
    include!("tests/remap.rs");
}
