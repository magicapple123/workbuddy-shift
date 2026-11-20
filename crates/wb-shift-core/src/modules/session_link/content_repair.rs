//! 复制路径专用的 JSONL 容错修复：纯文本变换，只读源内容、绝不回写源文件。
//!
//! 为什么单独一层而不是放松 [`super::content::normalize_jsonl`]：严格语义（空 / 非法 /
//! 截断一律 `Unavailable`）是复制、同步、基线比较的共同前提，放松它就等于让「内容身份」
//! 不再可信。生产上真实存在的坏行有三类，都是**客户端自己就能容错显示**的形态
//! （进程被杀留下末行半截、退出时没写换行导致两个对象粘在同一行、某一行被裸 NUL 填充撑大），
//! 用户因此认为「会话好好的」，却在复制时被拒绝。因此容错只发生在**复制**这一条路径上：
//!
//! 复制读源 → 严格校验失败 → 本模块给出「可解释、有损性明确」的修复结果 → 复制继续，
//! 源文件与严格校验行为都不变。修复结果必须能重新通过严格校验，否则一律报错。

use super::content::{describe_parse_failure, normalize_jsonl};

/// 一次复制前的 JSONL 修复结果。
///
/// `text` 是可以直接写入副本的正文（已是修复后的形态）；其余附加字段只用于向用户
/// 解释「这次复制到底动了什么」，本身不参与摘要与校验。四种报告口径互斥且可组合：
/// 整行丢弃 / 末行「保留完整值、只丢残尾」/ 粘连拆行 / 剥离裸 NUL 填充。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsonlRepair {
    /// 修复后的正文：行尾风格沿用原文（`\r\n` 不会被悄悄改成 `\n`）。
    pub text: String,
    /// 丢弃了末尾不完整记录的**行号**（1-based）。两类有损修复都会赋值：
    /// 整行丢弃（`kept_tail_line_records == None`）与「保留完整值 + 丢残尾」。
    pub dropped_tail_line: Option<usize>,
    /// `(原行号, 拆出的记录数)`：只为「粘连拆行」这一无损修复赋值。
    pub split_lines: Vec<(usize, usize)>,
    /// 该末行里**被保留下来的完整记录数**。`Some(_)` 表示做的是「保留完整值、只丢残尾」，
    /// 而不是整行丢弃——报告文案必须据此区分，不能把保留了记录说成「丢了一条」。
    pub kept_tail_line_records: Option<usize>,
    /// `(原行号, 剥离的裸 NUL 字节数)`：只为「NUL 填充」这一无损修复赋值。
    ///
    /// 与 `split_lines` 分开记录，是因为两者说明的不是同一件事：NUL 只是把一行撑大，
    /// 剥掉之后记录还在（中间行也能修）；粘连拆行则是两个值挤在同一行。混在一起会让
    /// 报告把「有填充」说成「有粘连」。
    pub nul_stripped: Vec<(usize, usize)>,
}

/// 复制路径上的 JSONL 修复：`Ok(None)` 表示严格校验本来就能通过，无需修复。
///
/// 只允许三类修复（有损那一类再分两种落法），且都不做任何猜测：
///
/// - **粘连拆行（无损）**：一行里能流式解析出 ≥2 个完整 JSON 值，且值之间与值之后
///   只剩空白。按字节偏移拆成多行，内容一个字节都不丢（复制出的副本可读、可继续同步）。
/// - **NUL 填充（无损）**：一行里混进了裸 `0x00`（issue #1 用户实测：第 1000 行行首
///   8 974 个 NUL + 其后 389 字节完整 JSON）。按 `0x00` 切段后逐段解释——纯填充段丢弃、
///   完整值段保留、粘连段展开；所有段都能无损解释才算修复成功。因为**不丢任何记录**，
///   这一类不受「只允许末行」的限制：中间行同样可修。整行只有填充 → 连行尾一起移除
///   （留一个空行会被严格校验判成异常）。
/// - **末行截断（有损）**：只发生在**最后一个非空行**、**前面已有 ≥1 条完整记录**、
///   且解析失败的原因是真正的「读到文件末尾」（`error.is_eof()`）：
///   - 该行里有已写完整的记录 → **保留它们**（拆成独立行），只丢末尾那半截；
///   - 该行整行都是半截（没有任何完整值）→ 丢弃整行。
///
///   三条限制各自堵一种「静默丢数据 / 谎报」：
///   - 错误不是 EOF（尾逗号 `{"a":1,}`、单引号键 `{'a':1}`、`{"a":NaN}`、值后跟
///     `garbage`）→ 那是「写完但语法不合法」，不是没写完，不能当截断丢掉；
///   - 行内有完整值 → 整行丢弃会连带丢掉已写完整的记录，必须先保留再丢残尾；
///   - 前面没有任何完整记录 → 只有一次没写完的写入，拒绝比给出半轮副本更安全。
///
/// 其余形态一律 `Err`（空行、纯垃圾行、半行但不在末行、NUL 把一条记录劈开、其它非法
/// 语法）：这些代表真正的数据损坏，静默丢弃或猜测性补齐都会让副本与源会话悄悄分叉。
///
/// 修复后必须重新通过严格校验才返回 `Ok(Some(..))`，保证调用方拿到的文本可以直接进入
/// 复制流程（摘要、基线、写后回读都以它为准）。
pub fn repair_jsonl_for_copy(
    text: &str,
    own_session_id: &str,
) -> Result<Option<JsonlRepair>, String> {
    let original_reason = match normalize_jsonl(text, own_session_id) {
        Ok(_) => return Ok(None),
        Err(reason) => reason,
    };
    let parts = split_parts(text);
    // 与 `normalize_jsonl` 同口径：末尾的空行不算记录（中间空行仍算异常）。
    let mut effective = parts.len();
    while effective > 0 && parts[effective - 1].body_empty {
        effective -= 1;
    }

    let mut out = String::with_capacity(text.len() + 64);
    let mut records = 0usize;
    let mut dropped_tail_line: Option<usize> = None;
    let mut split_lines: Vec<(usize, usize)> = Vec::new();
    let mut kept_tail_line_records: Option<usize> = None;
    let mut nul_stripped: Vec<(usize, usize)> = Vec::new();
    for (index, part) in parts.iter().enumerate() {
        let line_no = index + 1;
        if index >= effective {
            // 末尾空行：原样保留（不制造额外差异）。
            out.push_str(part.segment);
            continue;
        }
        if part.line.trim().is_empty() {
            return Err(format!("第 {line_no} 行为空，内容可能不完整"));
        }
        let separator = if part.cr { "\r\n" } else { "\n" };
        match serde_json::from_str::<serde::de::IgnoredAny>(part.line) {
            Ok(_) => {
                out.push_str(part.segment);
                records += 1;
            }
            Err(error) => {
                let is_last = index + 1 == effective;
                // 第三类形态：行里混进了裸 NUL。它**无损**，因此不受「只允许末行」的限制。
                // 判定顺序与 `classify_nul_line` 的文档一致（填充 → 完整值 → 截断 → 拒绝）。
                if part.line.contains('\0') {
                    match classify_nul_line(part.line) {
                        // 所有非空段都是完整值：剥掉填充、按原行尾风格一条一行写出。
                        NulLine::Lossless { values, stripped } => {
                            out.push_str(&values.join(separator));
                            out.push_str(part.ending());
                            records += values.len();
                            nul_stripped.push((line_no, stripped));
                            continue;
                        }
                        // 整行只有填充：连行尾一起移除（留空行会被严格校验判成异常）。
                        NulLine::Blank { stripped } => {
                            nul_stripped.push((line_no, stripped));
                            continue;
                        }
                        // 段内有「写了一半」的值：沿用末行规则（末行 + 前面已有完整记录）。
                        NulLine::Truncated { values, stripped } if is_last && records >= 1 => {
                            if !values.is_empty() {
                                out.push_str(&values.join(separator));
                                out.push_str(part.ending());
                                records += values.len();
                                kept_tail_line_records = Some(values.len());
                            }
                            nul_stripped.push((line_no, stripped));
                            dropped_tail_line = Some(line_no);
                            continue;
                        }
                        // 非末行的截断、或段内有垃圾（NUL 把一条记录劈开）：不得猜测修补。
                        NulLine::Truncated { .. } | NulLine::Rejected => {
                            return Err(describe_parse_failure(
                                part.line, line_no, is_last, &error,
                            ));
                        }
                    }
                }
                match classify_damaged_line(part.line) {
                    // 无损：多个完整值粘在同一行 → 拆成多条记录。
                    LineDamage::Glued(values) => {
                        out.push_str(&values.join(separator));
                        out.push_str(part.ending());
                        records += values.len();
                        split_lines.push((line_no, values.len()));
                    }
                    // 有损：只允许发生在末行、且前面已至少有一条完整记录。
                    LineDamage::TruncatedTail(values) if is_last && records >= 1 => {
                        // 行内已经写完整的记录必须保留（先拆成独立行），只丢末尾半截。
                        if !values.is_empty() {
                            out.push_str(&values.join(separator));
                            out.push_str(part.ending());
                            records += values.len();
                            kept_tail_line_records = Some(values.len());
                        }
                        // 上一行必定带换行，因此修复后的正文仍然以换行结束。
                        dropped_tail_line = Some(line_no);
                    }
                    _ => return Err(describe_parse_failure(part.line, line_no, is_last, &error)),
                }
            }
        }
    }

    if dropped_tail_line.is_none() && split_lines.is_empty() && nul_stripped.is_empty() {
        // 防御性分支：没有应用任何修复说明两次判断不一致，保守返回原始诊断。
        return Err(original_reason);
    }
    let normalized = normalize_jsonl(&out, own_session_id)
        .map_err(|reason| format!("修复后的内容仍无法校验（{reason}）"))?;
    if normalized.record_count != records {
        return Err(format!(
            "修复后的记录数不一致（校验得到 {}，拆分为 {records}）",
            normalized.record_count
        ));
    }
    Ok(Some(JsonlRepair {
        text: out,
        dropped_tail_line,
        split_lines,
        kept_tail_line_records,
        nul_stripped,
    }))
}

/// 一行「严格校验失败」的文本属于哪种形态：修复动作由形态决定，不能只看「哪里失败」。
#[derive(Debug)]
enum LineDamage<'a> {
    /// ≥2 个完整 JSON 值挤在一行，且值之间与值之后只剩空白 → 可无损拆行。
    Glued(Vec<&'a str>),
    /// 若干完整值 + 末尾半截（流式解析在**文件末尾**报错）→ 完整值可保留，只有残尾不可保留。
    /// 空 `Vec` 表示整行都是半截（行内没有任何完整值）。
    TruncatedTail(Vec<&'a str>),
    /// 其余损坏（尾逗号、单引号键、`NaN`、值后跟垃圾……）→ 一律拒绝，不得猜测修补。
    Damaged,
}

/// 判定失败行的形态：关键是把「没写完」与「写完但不合法」分开。
///
/// 用 `StreamDeserializer` 逐个取值并记录 `byte_offset()`：值之间的字节只能是空白、
/// 每段必须能独立复解析，末值之后的残留则由解析器给出的错误类型定性——
/// 只有 `error.is_eof()`（真正读到文件末尾）才是「写了一半」，
/// 尾逗号 / 单引号键 / `NaN` / 值后跟 `garbage` 都不是 EOF，必须当损坏拒绝。
fn classify_damaged_line(line: &str) -> LineDamage<'_> {
    let mut spans: Vec<(usize, usize)> = Vec::new();
    let mut stream = serde_json::Deserializer::from_str(line).into_iter::<serde::de::IgnoredAny>();
    let tail = loop {
        let start = stream.byte_offset();
        match stream.next() {
            Some(Ok(_)) => {
                let end = stream.byte_offset();
                if end <= start {
                    return LineDamage::Damaged;
                }
                spans.push((start, end));
            }
            Some(Err(error)) => break Some(error),
            None => break None,
        }
    };
    // 逐段取出原文并复核：偏移必须连续、值之间只允许空白、每段都能独立解析。
    let mut values: Vec<&str> = Vec::with_capacity(spans.len());
    let mut cursor = 0usize;
    for (start, end) in spans {
        if start < cursor || end > line.len() {
            return LineDamage::Damaged;
        }
        let Some(gap) = line.get(cursor..start) else {
            return LineDamage::Damaged;
        };
        if !gap.trim().is_empty() {
            return LineDamage::Damaged;
        }
        let Some(raw) = line.get(start..end).map(str::trim) else {
            return LineDamage::Damaged;
        };
        if raw.is_empty() || serde_json::from_str::<serde::de::IgnoredAny>(raw).is_err() {
            return LineDamage::Damaged;
        }
        values.push(raw);
        cursor = end;
    }
    match tail {
        // 尾部只剩空白：≥2 个值才是「粘连」（单个值 + 空白本来就能通过严格校验）。
        None => match line.get(cursor..) {
            Some(rest) if rest.trim().is_empty() && values.len() >= 2 => LineDamage::Glued(values),
            _ => LineDamage::Damaged,
        },
        Some(error) if error.is_eof() => LineDamage::TruncatedTail(values),
        Some(_) => LineDamage::Damaged,
    }
}

/// 一行（严格校验失败的）「带裸 NUL 填充」文本的分段解释结果。
///
/// 判据只有一条：**能否证明剥离 NUL 没有丢记录**。能证明 → 无损修复；不能证明 → 拒绝。
/// NUL 本身不是空白字符（`str::trim` 不认它），所以这类行既不会命中「空行」判断，
/// 也无法被 `serde_json` 解析（第 1 列就报 `expected value`），必须单独识别。
#[derive(Debug)]
enum NulLine<'a> {
    /// 所有非空段都是完整值（段内粘连已展开）→ 剥离 NUL 无损，且与位置无关。
    Lossless {
        values: Vec<&'a str>,
        stripped: usize,
    },
    /// 整行只有 NUL / 空白填充，没有任何记录 → 连行尾一起移除（留空行会被判异常）。
    Blank { stripped: usize },
    /// 某一段是「写了一半」的值：段内完整值可保留，但**是否允许**由调用方按末行规则判定。
    Truncated {
        values: Vec<&'a str>,
        stripped: usize,
    },
    /// 有段无法解释（NUL 把一条记录劈开 / 段内有垃圾）→ 拒绝，不得猜测拼接。
    Rejected,
}

/// 按裸 `0x00` 切段并逐段定性（顺序即优先级：填充 → 完整值 → 截断 → 拒绝）。
///
/// 只有「每一段都能独立解释」才允许修复：任何一段是垃圾或拼接残片，NUL 就可能落在一条
/// 记录内部，此时任何补法都是猜测（可能把两条记录缝成一条，也可能把一条劈成两条）。
fn classify_nul_line(line: &str) -> NulLine<'_> {
    let mut values: Vec<&str> = Vec::new();
    let mut truncated = false;
    for segment in line.split('\0') {
        // 空段（相邻 NUL）与纯空白段都是填充，直接丢弃。
        if segment.trim().is_empty() {
            continue;
        }
        if serde_json::from_str::<serde::de::IgnoredAny>(segment).is_ok() {
            values.push(segment.trim());
            continue;
        }
        match classify_damaged_line(segment) {
            // 段内仍然是「多个完整值粘连」：同样无损，一并展开。
            LineDamage::Glued(glued) => values.extend(glued),
            // 段内有完整值 + 残尾：完整值留下，把「有截断」记下来交给调用方处置。
            LineDamage::TruncatedTail(tail) => {
                values.extend(tail);
                truncated = true;
            }
            LineDamage::Damaged => return NulLine::Rejected,
        }
    }
    let stripped = line.bytes().filter(|byte| *byte == 0).count();
    if truncated {
        return NulLine::Truncated { values, stripped };
    }
    if values.is_empty() {
        return NulLine::Blank { stripped };
    }
    NulLine::Lossless { values, stripped }
}

/// 正文的一个原始行片段：保留原始字节，便于逐字重建（含行尾风格）。
struct LinePart<'a> {
    /// 原样片段，含行尾（`\n` 与可能的 `\r`）。
    segment: &'a str,
    /// 去掉 `\r` / `\n` 的行内容（与 `normalize_jsonl` 的解析口径一致）。
    line: &'a str,
    /// 去掉 `\n` 后是否为空：`normalize_jsonl` 只按这个口径剔除末尾空行。
    body_empty: bool,
    /// 该行以 `\r\n` 结束。
    cr: bool,
    /// 该行以 `\n` 结束（末行可能没有换行）。
    nl: bool,
}

impl LinePart<'_> {
    /// 该行的原始行尾（用于拆行后给最后一段补回同样的行尾）。
    fn ending(&self) -> &'static str {
        match (self.cr, self.nl) {
            (true, true) => "\r\n",
            (false, true) => "\n",
            _ => "",
        }
    }
}

/// 按 `split_inclusive('\n')` 切行：与 `split('\n')` 的切分位置一致，但保留行尾。
fn split_parts(text: &str) -> Vec<LinePart<'_>> {
    let mut parts = Vec::new();
    for segment in text.split_inclusive('\n') {
        let (body, nl) = match segment.strip_suffix('\n') {
            Some(body) => (body, true),
            None => (segment, false),
        };
        let (line, cr) = match body.strip_suffix('\r') {
            Some(line) => (line, true),
            None => (body, false),
        };
        parts.push(LinePart {
            segment,
            line,
            body_empty: body.is_empty(),
            cr,
            nl,
        });
    }
    parts
}
