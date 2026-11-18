//! VS Code CodeBuddy 插件会话的内容身份（design §3）。
//!
//! WorkBuddy 的记录单位是 JSONL 的一行，本模块把它换成**一条消息**：
//!
//! - 记录序列 = 会话 `index.json` 的 `messages[]` **顺序**（目录顺序不可靠，不用 `read_dir`）；
//! - 单条记录摘要 = `messages/<id>.json` 的**规范化 JSON** 取 SHA-256（长度编码后计算，
//!   与 WorkBuddy 的行摘要同一口径）；
//! - 规范化 = 把「副本特异的 id」替换为占位符：`messages[].id` → `m0`/`m1`…、
//!   `requests[].id` → `r0`/`r1`…（各按索引首次出现顺序编号）、会话自身 id → [`SESSION_ID_MARKER`]。
//!
//! 归一化只替换**已知 id**，其它 32-hex（如 `extra.traceId`）原样保留——沿用 WorkBuddy 的
//! 保守哲学：未知差异宁可作为差异呈现，也不猜。同一逻辑会话的源与副本因此得到逐条相同的
//! 摘要，这是「可快进 / 分叉 / 一致」判定的地基。
//!
//! 反直觉但已实测（design §3）：`isComplete: false` 是助手消息的**常态**（26 条消息的会话里
//! 13 条助手消息全为 false，而其 `requests[].state` 全为 `complete`）。**不得**用它判断
//! 「消息是否写完」——摘要只按字节算。
//!
//! 并发前提：读写这些文件的调用方都必须在「VS Code 已完全退出」之后执行
//! （`vscode-ext-process.md` 的顺序不变量），因此这里不做读期间变化的复查。
//!
//! 已知限制（安全方向）：不排除「易变尾部」。复制时最后一条消息若处于半写入状态、
//! 之后在源侧完成，则源的该位置摘要变化 → 判定为分叉（而非误判可快进），用户可用覆盖模式处理。

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::Path;

use crate::modules::session_link::{
    self, full_digest_of, line_digest_of, to_hex, ContentSnapshot, ContentState, NormalizedContent,
    SESSION_ID_MARKER,
};
use crate::modules::vscode_session::{read_json, replace_ids_in_extra};

/// 派生 id 的域前缀与版本（公式变化必须升版本，避免新旧 id 混用）。
const DERIVE_PREFIX: &str = "vscode-session-link:v1";

mod identity;
mod link;

pub use identity::{derive_message_id, derive_request_id, request_digest, request_digests};
pub use link::{
    message_ids_of, positional_translation, read_session_content, read_session_index,
    translate_message, translate_message_entry, translate_request_entry, translate_session_index,
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::vscode_session::is_hex32;
    use std::path::PathBuf;

    include!("tests/link.rs");
}
