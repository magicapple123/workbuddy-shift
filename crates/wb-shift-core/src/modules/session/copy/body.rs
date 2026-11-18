//! 复制正文：正文路径解析、读取、末条 incomplete 归一与原子写入。
use super::*;

/// 目标正文路径：目标档 `projects` 下沿用源文件所在的工作区子目录。
///
/// 客户端按会话行的 `cwd` 推导工作区目录名（跨档实验已验证：同名目录下客户端可读），
/// 而 `cwd` 在复制时原样保留，所以跨档时目录名也必须保持一致（design §2.2）。
/// 同档时 `source_paths == target_paths`，结果与源文件同目录，与改造前逐字相同。
pub(in crate::modules::session) fn target_body_path(
    source_path: &Path,
    source_paths: &SessionPaths,
    target_paths: &SessionPaths,
    new_cid: &str,
) -> PathBuf {
    let file_name = format!("{new_cid}.jsonl");
    let relative = source_path
        .parent()
        .and_then(|parent| parent.strip_prefix(source_paths.projects_dir()).ok())
        .filter(|relative| !relative.as_os_str().is_empty());
    match relative {
        Some(relative) => target_paths.projects_dir().join(relative).join(file_name),
        None => target_paths.projects_dir().join(file_name),
    }
}

/// 复制正文的「待写入文本 + 归一化摘要」：两者出自同一次计算，校验基准只能取这一份。
pub(in crate::modules::session) struct CopyBody {
    pub(in crate::modules::session) text: String,
    pub(in crate::modules::session) normalized: NormalizedContent,
}

/// 算出副本要写的正文与它的期望摘要。
///
/// 先把源正文里的会话 id 换成目标 id，再归一最后一条 assistant 消息的 `incomplete`（见
/// [`normalize_copy_tail`]），最后按**同一份待写入文本**计算归一化摘要。
///
/// 为什么必须在这里算、而不是写完再改正文：`session_link::normalize_jsonl` 对每一行
/// 原始 JSON 文本做哈希（只把本副本的 session id 换成固定标记），`status` 字段一变，
/// 该行摘要与 `total_digest` 就一起变。先按源文本落盘再改正文，回读摘要必然不等于源
/// 快照摘要，[`write_copy_body`] 的完整性校验会把「改对了的副本」判成失败；反过来
/// 为了绕过它而删掉校验，就等于放弃复制后自检。正确做法是让「写入的文本」和
/// 「校验基准」出自同一次计算（本函数），基线也用这份摘要，后续同步才不误判漂移。
pub(in crate::modules::session) fn copy_body(
    snapshot: &ContentSnapshot,
    cid: &str,
    new_cid: &str,
) -> Result<CopyBody, String> {
    let text = normalize_body_tail(&snapshot.text.replace(cid, new_cid), NORMALIZE_REASON_COPY);
    let normalized = session_link::normalize_jsonl(&text, new_cid)
        .map_err(|reason| format!("复制后的内容无法校验（{reason}），未复制"))?;
    Ok(CopyBody { text, normalized })
}

/// 复制/同步共用的场景标记：写进 `_meta.incompleteNormalized.reason`，便于事后追溯
/// 这一行是被哪条路径归一的。
pub(in crate::modules::session) const NORMALIZE_REASON_COPY: &str = "copy normalize";

/// 见 [`NORMALIZE_REASON_COPY`]。
pub(in crate::modules::session) const NORMALIZE_REASON_SYNC: &str = "sync normalize";

/// 把正文最后一条 assistant 消息的 `incomplete` 归一到 `completed`（见
/// [`session_link::normalize_last_incomplete`]）。
///
/// 定位口径是「最后一条 assistant 消息」而不是「最后一条记录」——尾部跟 tool 结果行的
/// 会话客户端同样会显示「正在执行工具」；该口径与存量修复脚本
/// `scripts/diagnostics/fix-incomplete-turns.py` 一致。正文里没有这样的消息、或它的 `status` 不是
/// `incomplete` 时原样返回（幂等，不会把已完成的会话改成别的样子）。
pub(in crate::modules::session) fn normalize_body_tail(text: &str, reason: &str) -> String {
    match session_link::normalize_last_incomplete(text, &normalize_stamp(), reason) {
        Some(normalized) => normalized,
        None => text.to_string(),
    }
}

/// 留痕时间戳：与存量修复脚本同格式（`%Y%m%d-%H%M%S`，本地时间）。
pub(in crate::modules::session) fn normalize_stamp() -> String {
    chrono::Local::now().format("%Y%m%d-%H%M%S").to_string()
}

/// 写入副本正文并做写后校验（复用同一次源快照，避免 TOCTOU）。
///
/// 校验基准是 `body` 里那份**待写入文本**的摘要（不是源快照摘要）：被归一过时
/// 两者本来就不相等，拿源摘要去比必然失败。
pub(in crate::modules::session) fn write_copy_body(
    body: &CopyBody,
    source_path: &Path,
    source_paths: &SessionPaths,
    target_paths: &SessionPaths,
    new_cid: &str,
) -> Result<PathBuf, String> {
    let dest = target_body_path(source_path, source_paths, target_paths, new_cid);
    if dest.exists() {
        return Err("目标内容已存在同名文件，已停止复制".to_string());
    }
    // 跨档时目标档可能没有该工作区目录（源目录名跨档一致，见 `target_body_path`）。
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("目标工作区目录创建失败：{error}"))?;
    }
    // 正文属于业务完成门禁：会话专用持久化写（sync_all + 父目录持久化）。
    session_backup::durable_write_str(&dest, &body.text)
        .map_err(|error| format!("复制后的内容保存失败：{error}"))?;
    match session_link::read_content_snapshot(&dest, new_cid) {
        ContentState::Ready(read_back)
            if read_back.normalized.total_digest == body.normalized.total_digest =>
        {
            Ok(dest)
        }
        ContentState::Ready(_) => Err("复制后的内容保存后校验不一致，未按成功处理".to_string()),
        ContentState::Missing => Err("复制后的内容保存后不存在，未按成功处理".to_string()),
        ContentState::Unavailable(reason) => Err(format!("复制后的内容保存后无法确认：{reason}")),
    }
}
