//! 会话路径解析：工具存储根、档位数据根与关联存储各类文件 / 目录名。
//!
//! 三个宿主（WorkBuddy 桌面版、VS Code CodeBuddy 插件、CodeBuddy IDE）共用同一份内核，
//! 关联存储按 [`LinkNamespace`] 取不同文件名与目录名，互不可见（design §2）。
//! 单测注入临时目录，绝不触碰真实用户目录或客户端数据目录。

use crate::modules::config::store_dir;
use crate::modules::variant::WbVariant;
use std::path::{Path, PathBuf};

/// 关联存储的命名空间：决定关联表 / 基线 / 预览凭据 / 存储锁的名字。
///
/// 三个宿主（WorkBuddy 桌面版、VS Code CodeBuddy 插件、CodeBuddy IDE）共用同一份内核
/// （[`crate::modules::session_link`]），但各自的关联关系互不可见：
/// 同一工具存储根下按命名空间取不同的文件名与目录名，避免互相污染。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LinkNamespace {
    /// WorkBuddy 桌面版（默认，路径与改造前逐字相同）。
    #[default]
    WorkBuddy,
    /// VS Code CodeBuddy 插件的会话（独立文件名与目录）。
    VscodeExt,
    /// CodeBuddy IDE（国内版桌面客户端）的会话（独立文件名与目录）。
    CodeBuddyIde,
}

/// 会话操作涉及的路径集合：工具存储根（`~/.workbuddy-shift`）与档位数据根。
///
/// 生产入口用 [`SessionPaths::for_variant`]（WorkBuddy）与 [`SessionPaths::for_vscode_ext`]
/// （VS Code 插件）；单测注入临时目录，绝不触碰真实 `~/.workbuddy-shift` 或客户端数据目录。
#[derive(Clone, Debug)]
pub struct SessionPaths {
    /// 工具存储根：关联表、基线、操作日志、锁与备份都在这里。
    pub store_root: PathBuf,
    /// 档位数据根：`projects/`、`workbuddy.db`、`edge-sync-mapping-*.db`。
    /// VS Code 命名空间下不使用该字段（恒为空路径）。
    pub data_root: PathBuf,
    /// 该档位的官方登录态文件（来源账号 uid 的判据）。
    /// VS Code 命名空间下不使用该字段（恒为空路径）。
    pub auth_file: PathBuf,
    /// 关联存储命名空间；决定下面几个 `*_links*` 路径的名字。
    pub link_namespace: LinkNamespace,
}

impl SessionPaths {
    pub fn for_variant(variant: WbVariant) -> Self {
        Self {
            store_root: store_dir(),
            data_root: variant.data_root(),
            auth_file: variant.auth_file_path(),
            link_namespace: LinkNamespace::WorkBuddy,
        }
    }

    /// VS Code CodeBuddy 插件的关联存储路径。
    ///
    /// 只用到 `store_root`：关联表 / 基线 / 预览凭据 / 存储锁都落在 `~/.workbuddy-shift` 下
    /// VS Code 专属的名字里（design §2）；扩展的会话文件由调用方按数据根另行解析，
    /// 不走 `data_root` / `auth_file`（因此两者留空，避免误用）。
    pub fn for_vscode_ext() -> Self {
        Self::for_vscode_ext_at(store_dir())
    }

    /// [`Self::for_vscode_ext`] 的可测实现：显式传入工具存储根。
    pub fn for_vscode_ext_at(store_root: PathBuf) -> Self {
        Self {
            store_root,
            data_root: PathBuf::new(),
            auth_file: PathBuf::new(),
            link_namespace: LinkNamespace::VscodeExt,
        }
    }

    /// CodeBuddy IDE（国内版桌面客户端）的关联存储路径。
    ///
    /// 与 [`Self::for_vscode_ext`] 同构：只用到 `store_root`，会话文件由调用方按数据根另行解析。
    pub fn for_codebuddy_ide() -> Self {
        Self::for_codebuddy_ide_at(store_dir())
    }

    /// [`Self::for_codebuddy_ide`] 的可测实现：显式传入工具存储根。
    pub fn for_codebuddy_ide_at(store_root: PathBuf) -> Self {
        Self {
            store_root,
            data_root: PathBuf::new(),
            auth_file: PathBuf::new(),
            link_namespace: LinkNamespace::CodeBuddyIde,
        }
    }

    pub fn workbuddy_db(&self) -> PathBuf {
        self.data_root.join("workbuddy.db")
    }

    pub fn projects_dir(&self) -> PathBuf {
        self.data_root.join("projects")
    }

    pub fn edge_sync_db(&self, variant: WbVariant) -> PathBuf {
        edge_sync_db_path(&self.data_root, variant)
    }

    pub fn backup_root(&self) -> PathBuf {
        self.store_root.join("backups")
    }

    /// 关联组主表：三个目标各一份，互不可见（design §2）。
    pub fn session_links_file(&self) -> PathBuf {
        match self.link_namespace {
            LinkNamespace::WorkBuddy => self.store_root.join("session_links.json"),
            LinkNamespace::VscodeExt => self.store_root.join("vscode_session_links.json"),
            LinkNamespace::CodeBuddyIde => self.store_root.join("codebuddy_ide_session_links.json"),
        }
    }

    /// 关联存储目录（基线 / 凭据 / 操作日志的父目录）。
    pub fn session_links_dir(&self) -> PathBuf {
        match self.link_namespace {
            LinkNamespace::WorkBuddy => self.store_root.join("session-links"),
            LinkNamespace::VscodeExt => self.store_root.join("vscode-session-links"),
            LinkNamespace::CodeBuddyIde => self.store_root.join("codebuddy-ide-session-links"),
        }
    }

    pub fn baselines_dir(&self) -> PathBuf {
        self.session_links_dir().join("baselines")
    }

    pub fn operations_dir(&self) -> PathBuf {
        self.session_links_dir().join("operations")
    }

    /// 预览凭据目录（design §6：绑定保存在服务端，前端只拿 id）。
    /// 不属于未完成痕迹——凭据是一次性的，主文件缺失时不得据此拒绝初始化。
    pub fn preview_tokens_dir(&self) -> PathBuf {
        self.session_links_dir().join("previews")
    }

    pub fn locks_dir(&self) -> PathBuf {
        self.store_root.join("locks")
    }

    pub fn variant_ops_lock_file(&self, variant: WbVariant) -> PathBuf {
        self.locks_dir()
            .join(format!("session-ops-{}.lock", variant.as_str()))
    }

    /// Long-running client-scoped session operation lock. IDE and VS Code stores are shared
    /// across WorkBuddy variants, so their mutations must serialize across both variants.
    pub fn client_ops_lock_file(&self) -> PathBuf {
        let name = match self.link_namespace {
            LinkNamespace::WorkBuddy => "workbuddy",
            LinkNamespace::VscodeExt => "vscode-ext",
            LinkNamespace::CodeBuddyIde => "codebuddy-ide",
        };
        self.locks_dir().join(format!("session-client-{name}.lock"))
    }

    /// 关联存储的短时全局锁文件：与主表同生命周期，按命名空间分开。
    pub fn link_store_lock_file(&self) -> PathBuf {
        match self.link_namespace {
            LinkNamespace::WorkBuddy => self.locks_dir().join("session-links.lock"),
            LinkNamespace::VscodeExt => self.locks_dir().join("vscode-session-links.lock"),
            LinkNamespace::CodeBuddyIde => {
                self.locks_dir().join("codebuddy-ide-session-links.lock")
            }
        }
    }
}

/// 客户端数据根下的会话数据库（按档位取根）。
pub fn workbuddy_db_path(variant: WbVariant) -> PathBuf {
    variant.data_root().join("workbuddy.db")
}

/// 映射库文件名解析：`edge-sync-mapping.db` 记 0，`edge-sync-mapping-vN.db` 记 N
/// （N 为整数）。其它名字（含 `-shm` / `-wal` 伴生文件，它们不以 `.db` 结尾）返回 None。
fn edge_sync_db_version(path: &Path) -> Option<u64> {
    let name = path.file_name()?.to_str()?;
    let middle = name
        .strip_prefix("edge-sync-mapping")?
        .strip_suffix(".db")?;
    if middle.is_empty() {
        return Some(0);
    }
    let digits = middle.strip_prefix("-v")?;
    // `u64::parse` also accepts a leading `+`; WorkBuddy's filename contract is
    // digits only, so reject non-canonical names instead of treating them as candidates.
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    digits.parse::<u64>().ok()
}

/// 没有任何候选时返回的默认文件名（保留各档位既有的「云端映射库 xxx 不存在」文案）。
fn edge_sync_db_default_name(variant: WbVariant) -> &'static str {
    match variant {
        WbVariant::Cn => "edge-sync-mapping-v2.db",
        WbVariant::Ai => "edge-sync-mapping-v4.db",
    }
}

/// 云端映射库解析：扫描数据根下所有 `edge-sync-mapping*.db`，返回版本号最大的一个。
///
/// 写死任何版本都会再次失效：WorkBuddy 客户端自行演进文件名，本机实测 v2（迁移残留）、
/// v3、v4 并存，而 v3 从未出现在本工具任何代码历史里（2026-09 实测）。
/// 判据不用 mtime——本工具自身写入会刷新 mtime，按它选会自我强化错误结果；
/// 也不用行数——需逐个打开数据库，还要处理损坏与锁。两个档位走同一套发现逻辑。
/// 目录不存在、读取失败或没有任何候选时回落到默认文件名，不得 panic。
pub(super) fn edge_sync_db_path(root: &Path, variant: WbVariant) -> PathBuf {
    let best = std::fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let version = edge_sync_db_version(&path)?;
            path.is_file().then_some((version, path))
        })
        // 同版本号时按路径定序，保证结果与目录遍历顺序无关。
        .max_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.as_path().cmp(b.1.as_path())));
    match best {
        Some((_, path)) => path,
        None => root.join(edge_sync_db_default_name(variant)),
    }
}
