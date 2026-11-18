//! 配对基线数据模型、持久化与读取状态。

use super::super::content::{
    is_ordered_prefix, total_digest_of, NormalizedContent, NORMALIZATION_VERSION,
};
use super::store::{find_pair_base, LinkGroup, PairBase};
use crate::modules::config::now_ms;
use crate::modules::session::SessionPaths;
use crate::modules::session_backup;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// 配对基线记录格式版本。
pub const BASELINE_VERSION: u32 = 1;
/// 基线记录本体：有序归一化行摘要 + 总摘要 + 记录数。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BaselineRecord {
    pub version: u32,
    pub baseline_ref: String,
    pub normalization_version: u32,
    pub created_at: i64,
    pub record_count: usize,
    pub total_digest: String,
    pub line_digests: Vec<String>,
}

impl BaselineRecord {
    /// 记录自洽性：版本、记录数、总摘要都能从行摘要重算出来。
    pub fn is_self_consistent(&self) -> bool {
        self.version == BASELINE_VERSION
            && self.record_count == self.line_digests.len()
            && self.total_digest == total_digest_of(&self.line_digests)
    }
}

// ---------------------------------------------------------------------------

fn baseline_file(paths: &SessionPaths, baseline_ref: &str) -> PathBuf {
    paths.baselines_dir().join(format!("{baseline_ref}.json"))
}

/// 写入配对基线文件（内容自洽；同一 ref 覆盖写等价）。
pub fn save_baseline(
    paths: &SessionPaths,
    baseline_ref: &str,
    normalized: &NormalizedContent,
) -> Result<BaselineRecord, String> {
    let record = BaselineRecord {
        version: BASELINE_VERSION,
        baseline_ref: baseline_ref.to_string(),
        normalization_version: NORMALIZATION_VERSION,
        created_at: now_ms(),
        record_count: normalized.record_count,
        total_digest: normalized.total_digest.clone(),
        line_digests: normalized.line_digests.clone(),
    };
    if !record.is_self_consistent() {
        return Err("上次同步的记录内容不一致，无法保存".to_string());
    }
    std::fs::create_dir_all(paths.baselines_dir()).map_err(|error| error.to_string())?;
    let content = serde_json::to_string_pretty(&record).map_err(|error| error.to_string())?;
    // 基线属于业务完成门禁：必须走会话专用持久化写（design §4）。
    session_backup::durable_write_str(&baseline_file(paths, baseline_ref), &content)
        .map_err(|error| error.to_string())?;
    Ok(record)
}

/// 读取配对基线；缺失/损坏/版本不符/归一化版本不符一律视为不可验证（None）。
pub fn load_baseline(paths: &SessionPaths, baseline_ref: &str) -> Option<BaselineRecord> {
    let text = std::fs::read_to_string(baseline_file(paths, baseline_ref)).ok()?;
    let record: BaselineRecord = serde_json::from_str(&text).ok()?;
    if !record.is_self_consistent() || record.normalization_version != NORMALIZATION_VERSION {
        return None;
    }
    Some(record)
}

/// 由来源内容向新成员继承配对基线：要求来源内容有序前缀包含该基线，否则不推断。
///
/// 例：A/B 已有基线 X，来源 A 的当前内容以 X 为前缀，则新成员 C 可建立 B/C = X 的
/// 共同基线（C 的内容 = A 的当前内容）。无法证明包含关系时不建立。
pub fn inheritable_baseline(
    paths: &SessionPaths,
    pair: &PairBase,
    source_line_digests: &[String],
) -> Option<BaselineRecord> {
    if pair.normalization_version != NORMALIZATION_VERSION {
        return None;
    }
    let record = load_baseline(paths, &pair.baseline_ref)?;
    if !is_ordered_prefix(&record.line_digests, source_line_digests) {
        return None;
    }
    Some(record)
}

/// 读取某个成员对的共同基线状态（供判定使用，见 [`BaselineState`]）。
///
/// 归一化版本不符、文件缺失或损坏都返回 [`BaselineState::Unverifiable`]——
/// 不能与「从未建立过基线」混为一谈，两者都不允许快进。
pub fn load_pair_baseline(
    paths: &SessionPaths,
    group: &LinkGroup,
    member_a: &str,
    member_b: &str,
) -> BaselineState {
    let Some(pair) = find_pair_base(group, member_a, member_b) else {
        return BaselineState::Missing;
    };
    if pair.normalization_version != NORMALIZATION_VERSION {
        return BaselineState::Unverifiable(format!(
            "记录格式版本 {} 不受支持（当前 {}）",
            pair.normalization_version, NORMALIZATION_VERSION
        ));
    }
    match load_baseline(paths, &pair.baseline_ref) {
        Some(record) => BaselineState::Ready(record),
        None => BaselineState::Unverifiable("上次同步的记录缺失或已损坏".to_string()),
    }
}

// ---------------------------------------------------------------------------

/// 配对共同基线的可用状态。
///
/// 这是判定输入的一部分（不含 IO），调用方负责先读文件再传进来，
/// [`crate::modules::session_link::decide_sync`] 因此是纯函数：同样的输入必然得到同样的判定。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BaselineState {
    /// 可验证的共同基线：文件自洽且归一化版本一致。
    Ready(BaselineRecord),
    /// 该成员对从未建立过基线。
    Missing,
    /// 有基线引用但内容不可验证（文件缺失/损坏/归一化版本不符）。
    Unverifiable(String),
}

impl BaselineState {
    pub fn ready(&self) -> Option<&BaselineRecord> {
        match self {
            BaselineState::Ready(record) => Some(record),
            _ => None,
        }
    }

    /// 不可验证时的说明；[`BaselineState::Ready`] 返回 None。
    pub fn unusable_reason(&self) -> Option<String> {
        match self {
            BaselineState::Ready(_) => None,
            BaselineState::Missing => Some("找不到双方上次一致的内容，暂时无法同步".to_string()),
            BaselineState::Unverifiable(reason) => {
                Some(format!("上次一致的内容不可用（{reason}），暂时无法同步"))
            }
        }
    }
}
