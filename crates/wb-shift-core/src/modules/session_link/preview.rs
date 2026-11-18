//! 会话同步预览凭据及其版本绑定校验。

use super::content::to_hex;
use super::link_store::LinkGroup;
use super::sync_decision::SyncVerdict;
use crate::modules::config::{atomic_write, now_ms};
use crate::modules::session::SessionPaths;
use crate::modules::variant::WbVariant;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::PathBuf;

// ---------------------------------------------------------------------------
// 预览凭据（design §6）
// ---------------------------------------------------------------------------

/// 预览凭据格式版本；读到其它版本一律视为过期。
///
/// v2：新增双方会话行的 `status` 与本次归档动作，执行前重新读取并逐项比对。
pub const PREVIEW_TOKEN_VERSION: u32 = 2;
/// 每档位保留的历史预览凭据条数（凭据是一次性的，不做长期保留）。
pub const KEEP_PREVIEW_TOKENS: usize = 200;

/// 预览时记录的单个成员绑定：执行前逐项核对，任何一项变化都算预览过期。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewMemberBinding {
    pub member_id: String,
    #[serde(default)]
    pub account_id: Option<String>,
    pub uid: String,
    pub session_id: String,
    /// 原始正文摘要（含空白）：预览之后正文有任何改动都会失配。
    pub raw_digest: String,
    /// 归一化总摘要：判定的依据。
    pub normalized_digest: String,
    /// 记录数（不称消息数）。
    pub record_count: usize,
}

/// 预览凭据绑定的全部版本信息：身份、组与基线版本、双方原始正文摘要、判定结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewBinding {
    pub variant: WbVariant,
    pub group_id: String,
    /// 组结构指纹（成员身份/状态 + 配对基线引用）。
    pub group_fingerprint: String,
    pub source: PreviewMemberBinding,
    pub target: PreviewMemberBinding,
    #[serde(default)]
    pub baseline_ref: Option<String>,
    #[serde(default)]
    pub baseline_total_digest: Option<String>,
    #[serde(default)]
    pub baseline_record_count: Option<usize>,
    pub verdict: SyncVerdict,
    /// 来源会话行的 `status`（表缺列时为 None，表示不支持状态同步）。
    #[serde(default)]
    pub source_status: Option<String>,
    /// 目标会话行的 `status`（表缺列时为 None，表示不支持状态同步）。
    #[serde(default)]
    pub target_status: Option<String>,
    /// 本次是否授权「仅同步归档」：`"statusOnly"` 或 None。
    ///
    /// 预览生成时即固定，不随用户后来是否勾选变化；执行时以绑定为准。
    #[serde(default)]
    pub archive_action: Option<String>,
}

/// 服务端保存的预览凭据。
///
/// 前端只拿得到 `preview_id`，绑定内容保存在服务端：伪造或篡改前端参数都不能扩大权限。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewToken {
    pub version: u32,
    pub preview_id: String,
    pub created_at: i64,
    pub binding: PreviewBinding,
}

/// 组结构指纹：成员（id/身份/状态）与配对基线引用，顺序无关。
///
/// 组内任何成员替换或基线变化都会改变指纹，因此可用作预览凭据的「组版本」。
/// 其它组的变化不会影响本指纹，避免同一次切换里的复制误伤无关组的预览。
pub fn group_fingerprint(group: &LinkGroup) -> String {
    let mut members: Vec<String> = group
        .members
        .iter()
        .map(|member| {
            format!(
                "{}|{}|{}|{}",
                member.member_id,
                member.uid,
                member.session_id,
                member.state.as_str()
            )
        })
        .collect();
    members.sort();
    let mut pairs: Vec<String> = group
        .pair_bases
        .iter()
        .map(|pair| {
            format!(
                "{}|{}|{}|{}",
                pair.member_ids[0],
                pair.member_ids[1],
                pair.baseline_ref,
                pair.normalization_version
            )
        })
        .collect();
    pairs.sort();

    let mut hasher = Sha256::new();
    hasher.update(b"workbuddy-shift-group-v1\0");
    hasher.update(group.variant.as_str().as_bytes());
    hasher.update([0u8]);
    hasher.update(group.id.as_bytes());
    hasher.update([0u8]);
    for entry in members {
        hasher.update(entry.as_bytes());
        hasher.update([0u8]);
    }
    for entry in pairs {
        hasher.update(entry.as_bytes());
        hasher.update([0u8]);
    }
    to_hex(&hasher.finalize())
}

/// 凭据 id 必须是本模块生成的 UUID：拒绝路径穿越等构造值。
pub fn valid_preview_id(preview_id: &str) -> bool {
    uuid::Uuid::parse_str(preview_id.trim()).is_ok()
}

fn preview_file(paths: &SessionPaths, preview_id: &str) -> PathBuf {
    paths
        .preview_tokens_dir()
        .join(format!("{preview_id}.json"))
}

/// 保存一份预览绑定并返回凭据 id。
pub fn save_preview_token(paths: &SessionPaths, binding: PreviewBinding) -> Result<String, String> {
    let preview_id = uuid::Uuid::new_v4().to_string();
    let token = PreviewToken {
        version: PREVIEW_TOKEN_VERSION,
        preview_id: preview_id.clone(),
        created_at: now_ms(),
        binding,
    };
    std::fs::create_dir_all(paths.preview_tokens_dir()).map_err(|error| error.to_string())?;
    // 先清理再写入：清理按时间排序，刚保存的凭据不会被自己的清理删掉
    // （不受文件系统时间戳精度影响）。
    prune_preview_tokens(paths, KEEP_PREVIEW_TOKENS.saturating_sub(1));
    let content = serde_json::to_string_pretty(&token).map_err(|error| error.to_string())?;
    atomic_write(&preview_file(paths, &preview_id), &content)
        .map_err(|error| format!("预览凭据写入失败：{error}"))?;
    Ok(preview_id)
}

/// 读取预览凭据；id 非法、文件缺失/损坏、版本不符一律返回 None（视为过期）。
pub fn load_preview_token(paths: &SessionPaths, preview_id: &str) -> Option<PreviewToken> {
    if !valid_preview_id(preview_id) {
        return None;
    }
    let text = std::fs::read_to_string(preview_file(paths, preview_id.trim())).ok()?;
    let token: PreviewToken = serde_json::from_str(&text).ok()?;
    (token.version == PREVIEW_TOKEN_VERSION && token.preview_id == preview_id.trim())
        .then_some(token)
}

/// 清理历史预览凭据，保留最近 `keep` 条。
pub fn prune_preview_tokens(paths: &SessionPaths, keep: usize) -> usize {
    let Ok(entries) = std::fs::read_dir(paths.preview_tokens_dir()) else {
        return 0;
    };
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = entries
        .flatten()
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "json"))
        .filter_map(|entry| {
            let stamp = entry.metadata().ok()?.modified().ok()?;
            Some((stamp, entry.path()))
        })
        .collect();
    if files.len() <= keep {
        return 0;
    }
    files.sort_by_key(|entry| std::cmp::Reverse(entry.0));
    let mut removed = 0usize;
    for (_, path) in files.into_iter().skip(keep) {
        if std::fs::remove_file(path).is_ok() {
            removed += 1;
        }
    }
    removed
}

/// 核对预览凭据与实时状态，返回不一致项的说明（空表示一致，可以继续）。
///
/// 逐字段比较而不是整体比较，是为了给用户「哪一项变了」的可读原因。任何不一致都必须
/// 跳过该项，不得沿用用户旧选择（design §5.2）。
pub fn verify_preview(preview: &PreviewToken, live: &PreviewBinding) -> Vec<String> {
    let mut stale: Vec<String> = Vec::new();
    if preview.version != PREVIEW_TOKEN_VERSION {
        stale.push("检查结果已过期，请重新检查".to_string());
    }
    let expected = &preview.binding;
    if expected.variant != live.variant {
        stale.push("当前应用已变化".to_string());
    }
    if expected.group_id != live.group_id {
        stale.push("会话的关联关系已变化".to_string());
    }
    if expected.group_fingerprint != live.group_fingerprint {
        stale.push("会话的关联关系或同步记录已变化".to_string());
    }
    if expected.source != live.source {
        stale.push("来源账号的内容已变化".to_string());
    }
    if expected.target != live.target {
        stale.push("目标账号的内容已变化".to_string());
    }
    if expected.baseline_ref != live.baseline_ref {
        stale.push("上次同步的记录已变化".to_string());
    } else if expected.baseline_total_digest != live.baseline_total_digest
        || expected.baseline_record_count != live.baseline_record_count
    {
        stale.push("上次同步的内容已变化".to_string());
    }
    if expected.verdict != live.verdict {
        stale.push("检查结果已变化，请重新检查".to_string());
    }
    if expected.source_status != live.source_status || expected.target_status != live.target_status
    {
        stale.push("会话的归档状态已变化，请重新检查".to_string());
    }
    if expected.archive_action != live.archive_action {
        stale.push("可执行的归档同步已变化，请重新检查".to_string());
    }
    stale
}
