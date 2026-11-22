//! 写入 `state.vscdb` 的提交路径：行级快照 → 写入 → 内容回读校验 → 失败回滚。
//!
//! **为什么与读/探测分开**（本文件从 `inject.rs` 拆出）：`inject.rs` 负责「打开、读既有值、
//! 判断行是否存在、解析路径」这类**只读或路径**逻辑，本文件负责**会改动用户库**的那一步。
//! 改动敏感，评审要能在一个文件里看完「写之前存了什么、写完怎么验、验不过怎么退」。
//! 公开 API 与 SQL 行完全不变。

use std::time::Duration;

// `inject.rs` 提供失败分类与连接打开，`safestorage.rs` 提供加解密与 Buffer 编解码；
// 两处导入在本文件里都是无条件使用的（没有跨平台 cfg 分支），因此不加 unused 抑制。
use super::inject::*;
use super::safestorage::*;
use super::*;

/// 一次「写库 + 内容校验 + 失败回滚」所需的全部输入。
///
/// 打成结构而不是一长串位置参数：回滚时这些字段全都要用得上，位置参数太容易错位。
pub(super) struct SecretWrite<'a> {
    /// ItemTable 的完整 key。
    pub(super) key: &'a str,
    /// 已编码好的 Buffer JSON（= 要写进 `ItemTable.value` 的行值）。
    pub(super) buffer_json: &'a str,
    /// 行值里的密文字节：解密不可用时用它与回读值逐字节比对。
    pub(super) ciphertext: &'a [u8],
    /// 写入的明文：解密可用时用它与解密结果逐字节比对。
    pub(super) plaintext: &'a [u8],
    /// 数据根目录（Windows 从 `Local State` 取密钥）。
    pub(super) data_root: &'a Path,
    /// 目标描述符（macOS 用它的钥匙串服务名取密钥）。
    pub(super) target: &'a VscodeSafeStorageTarget,
    /// **行级快照**：写入前该 key 的旧行值；`None` = 写入前没有这一行。
    ///
    /// 为什么是行级而不是整库拷贝：`state.vscdb` 可能数百 MB，整库复制既慢又占盘；
    /// 回滚只需要还原我们自己会改的那一行，而这一行就是唯一被本次写入影响的数据。
    pub(super) previous: Option<&'a str>,
}

/// 写后内容校验的入参。
pub(super) struct ContentCheck<'a> {
    /// 从库里读回的行值原文（应当是 Buffer JSON）。
    pub(super) raw_value: &'a str,
    /// 本次写入的密文字节。
    pub(super) ciphertext: &'a [u8],
    /// 本次写入的明文。
    pub(super) plaintext: &'a [u8],
    /// 数据根目录（解密需要）。
    pub(super) data_root: &'a Path,
    /// 目标描述符（解密需要）。
    pub(super) target: &'a VscodeSafeStorageTarget,
}

/// 一次完整的写入尝试：打开 → 行级快照 → 加密 → 建表 / 写入 → 内容校验（失败即回滚）。
///
/// 读旧值（快照）必须在同一次尝试里做：上一次尝试撞锁失败时可能什么都没读到，
/// 前缀决定沿用 `v10` 还是 `v11`（Chromium 版本兼容），不能缓存。
pub(super) fn write_secret_once(
    target: &VscodeSafeStorageTarget,
    plaintext: &str,
    db_path: &Path,
    data_root: &Path,
    timeout: Duration,
) -> Result<(), InjectFailure> {
    write_secret_once_with(
        target,
        plaintext,
        db_path,
        data_root,
        timeout,
        verify_secret_content,
    )
}

/// [`write_secret_once`] 的可注入校验器版本。
///
/// 校验器必须可注入：单测要覆盖「校验判定不一致 → 回滚旧值」这条路径，而生产校验器
/// 只有在真能解密/比对时才判不一致（在真实库上无法稳定构造这种失败）。
pub(super) fn write_secret_once_with<F>(
    target: &VscodeSafeStorageTarget,
    plaintext: &str,
    db_path: &Path,
    data_root: &Path,
    timeout: Duration,
    verify: F,
) -> Result<(), InjectFailure>
where
    F: Fn(&ContentCheck<'_>) -> Result<(), String>,
{
    let conn = open_state_db_with_timeout(db_path, false, timeout)
        .map_err(|e| classify_db_error("打开 state.vscdb 失败", e))?;
    conn.execute(
        "CREATE TABLE IF NOT EXISTS ItemTable (key TEXT PRIMARY KEY, value TEXT)",
        [],
    )
    .map_err(|e| classify_db_error("初始化 ItemTable 失败", e))?;

    let db_key = secret_storage_item_key_for(target);
    // 快照与既有前缀来自同一次查询：一个用于回滚，一个用于 v10/v11 兼容。
    let previous = read_item_value(&conn, &db_key)?;
    let existing_prefix = previous.as_deref().and_then(prefix_of_stored_value);

    let encrypted = encrypt_secret_payload(
        plaintext.as_bytes(),
        existing_prefix.as_deref(),
        data_root,
        target,
    )
    .map_err(InjectFailure::Fatal)?;
    // `encode_secret_buffer` 吃所有权，但密文字节还要留作校验退化路径的比对基准。
    let buffer_json = encode_secret_buffer(encrypted.clone()).map_err(InjectFailure::Fatal)?;

    commit_secret_value(
        &conn,
        &SecretWrite {
            key: &db_key,
            buffer_json: &buffer_json,
            ciphertext: &encrypted,
            plaintext: plaintext.as_bytes(),
            data_root,
            target,
            previous: previous.as_deref(),
        },
        verify,
    )
}

/// 读出该 key 的旧行值（写入前的行级快照）；`None` = 原本没有这一行。
fn read_item_value(conn: &Connection, key: &str) -> Result<Option<String>, InjectFailure> {
    match conn.query_row("SELECT value FROM ItemTable WHERE key = ?1", [key], |row| {
        row.get::<_, String>(0)
    }) {
        Ok(value) => Ok(Some(value)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        // 空库 / 尚未初始化 ItemTable：按「无该行」处理（表刚由调用方建好，实际走不到）。
        Err(err) if is_missing_table_error(&err) => Ok(None),
        // 读不到旧值还继续写，等于放弃了回滚能力——所以这里不能像改造前那样静默当成 None。
        // 锁冲突交给重试循环，其余直接报给用户。
        Err(err) => Err(classify_db_error("读取 state.vscdb 旧值失败", err)),
    }
}

/// 从既有行值推断密文前缀（`v10` / `v11`）；值不是 Buffer 结构时返回 `None`。
fn prefix_of_stored_value(raw: &str) -> Option<String> {
    let parsed: serde_json::Value = serde_json::from_str(raw).ok()?;
    let bytes = decode_buffer_data(&parsed).ok()?;
    detect_prefix(&bytes).map(str::to_string)
}

/// 一次「写库 → 内容回读校验 → 失败回滚」的完整提交。
///
/// 与加解密无关，因此单测可以注入任意 `buffer_json` 覆盖回滚路径：Windows 上没有
/// `Local State` 就加不了密，但这段「写坏了要能还原」的逻辑仍必须被测到。
///
/// 契约：**只有回滚成功后才返错**（缺陷一 #3/#4）——回滚失败时文案要点明库可能不一致。
pub(super) fn commit_secret_value<F>(
    conn: &Connection,
    write: &SecretWrite<'_>,
    verify: F,
) -> Result<(), InjectFailure>
where
    F: Fn(&ContentCheck<'_>) -> Result<(), String>,
{
    conn.execute(
        "INSERT OR REPLACE INTO ItemTable (key, value) VALUES (?1, ?2)",
        rusqlite::params![write.key, write.buffer_json],
    )
    .map_err(|e| classify_db_error("写入 state.vscdb 失败", e))?;

    // 内容回读 + 校验。闭包只为把「读回失败」与「内容不一致」汇成同一个失败出口，
    // 保证两条路径都会走回滚，不会有人漏掉其中一条。
    let checked = (|| -> Result<(), InjectFailure> {
        let raw_value: String = conn
            .query_row(
                "SELECT value FROM ItemTable WHERE key = ?1",
                [write.key],
                |row| row.get(0),
            )
            .map_err(|e| classify_db_error("写后校验失败", e))?;
        let check = ContentCheck {
            raw_value: &raw_value,
            ciphertext: write.ciphertext,
            plaintext: write.plaintext,
            data_root: write.data_root,
            target: write.target,
        };
        verify(&check).map_err(InjectFailure::Fatal)
    })();

    match checked {
        Ok(()) => Ok(()),
        Err(failure) => Err(rollback_after_failure(conn, write, failure)),
    }
}

/// 校验失败 → 恢复写入前的行值，并决定最终报给用户什么。
///
/// - 回滚成功：原样返回校验错误（库已回到写入前，用户可以安全重试）；
/// - 回滚失败：两条错误都报出来，并明确「数据库可能处于不一致状态，请用备份恢复」。
///   此时**不再重试**（哪怕原错误是锁冲突）：重试会把新的、未通过校验的值当成
///   「写入前的快照」，一旦再失败就永远回不到用户原来的值了。
fn rollback_after_failure(
    conn: &Connection,
    write: &SecretWrite<'_>,
    verify_failure: InjectFailure,
) -> InjectFailure {
    match restore_previous_value(conn, write.key, write.previous) {
        Ok(()) => verify_failure,
        Err(rollback_error) => InjectFailure::Fatal(format!(
            "{}\n\n回滚到切换前的值也失败了：{rollback_error}\n\n数据库可能处于不一致状态，请用备份恢复。",
            failure_text(&verify_failure)
        )),
    }
}

/// 把该 key 恢复成写入前的行值（`None` = 写入前没有这一行 → 删除该行）。
fn restore_previous_value(
    conn: &Connection,
    key: &str,
    previous: Option<&str>,
) -> Result<(), rusqlite::Error> {
    match previous {
        Some(value) => conn
            .execute(
                "INSERT OR REPLACE INTO ItemTable (key, value) VALUES (?1, ?2)",
                rusqlite::params![key, value],
            )
            .map(|_| ()),
        // 原本没有这一行 → 删掉它，而不是写回空值：空值会被读成「另一种」状态
        // （既不是「未登录」也不是合法载荷），后续排障会误判。
        None => conn
            .execute("DELETE FROM ItemTable WHERE key = ?1", [key])
            .map(|_| ()),
    }
}

/// 失败的文本形式（`Fatal` 是文案，`Busy` 是底层错误）：回滚失败时两条都要展示。
pub(super) fn failure_text(failure: &InjectFailure) -> String {
    match failure {
        InjectFailure::Fatal(message) => message.clone(),
        InjectFailure::Busy(err) => err.to_string(),
    }
}

/// 生产用的写入后内容校验。
///
/// **首选**：解密回来与 `plaintext` 逐字节比较——这是唯一能证明「用户下次打开编辑器
/// 真的能读到这份凭据」的判据。
///
/// **退化**：解密要读钥匙串（macOS `security`）或 DPAPI / Secret Service，这些能力在少数
/// 上下文里可能拿不到（钥匙串被锁、授权被拒、Linux 无密钥环）。此时不能因为「读不回来」
/// 就把用户的库判成写坏——加密刚刚成功、密文是我们自己算出来的，所以退化为
/// 「重新解析 Buffer → 与刚写入的密文逐字节相等」：它能证明库里存的就是我们算出的那一串，
/// 只是不能证明这串能被解开。两种情况都写清在文案里，排障时才分得清是哪一档。
pub(super) fn verify_secret_content(check: &ContentCheck<'_>) -> Result<(), String> {
    let parsed: serde_json::Value =
        serde_json::from_str(check.raw_value).map_err(|e| format!("写后校验 JSON 失败: {e}"))?;
    if parsed.get("type").and_then(|v| v.as_str()) != Some("Buffer") {
        return Err("写后校验失败：value 不是 Buffer".to_string());
    }
    let read_back = decode_buffer_data(&parsed)?;
    match decrypt_secret_payload(&read_back, check.data_root, check.target) {
        Ok(decrypted) => {
            if decrypted == check.plaintext {
                Ok(())
            } else {
                Err(format!(
                    "写后校验失败：解密结果与写入内容不一致（读回 {} 字节，期望 {} 字节）",
                    decrypted.len(),
                    check.plaintext.len()
                ))
            }
        }
        Err(decrypt_error) => {
            if read_back == check.ciphertext {
                Ok(())
            } else {
                Err(format!(
                    "写后校验失败：读回的密文与刚写入的密文不一致（本次无法解密回明文: {decrypt_error}）"
                ))
            }
        }
    }
}
