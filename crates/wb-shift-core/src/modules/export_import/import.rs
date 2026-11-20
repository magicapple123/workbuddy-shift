//! 导入：解析/预览、合并结果与导入入口。

use super::*;

/// 解析导出/导入文件文本：必须是 JSON 数组，且每项为 JSON 对象。
///
/// 失败时返回带位置的明确错误文案（非法 JSON / 非数组 / 元素不是对象）。
pub fn parse_accounts_json(text: &str) -> Result<Vec<Value>, String> {
    if text.trim().is_empty() {
        return Err("文件内容为空".to_string());
    }
    let parsed: Value =
        serde_json::from_str(text).map_err(|e| format!("文件不是合法的 JSON：{e}"))?;
    let array = parsed
        .as_array()
        .ok_or_else(|| "文件内容应为 JSON 数组（账号列表）".to_string())?;
    for (index, item) in array.iter().enumerate() {
        if !item.is_object() {
            return Err(format!("文件第 {} 项不是合法的账号对象", index + 1));
        }
    }
    Ok(array.clone())
}

/// 生成导入文件的脱敏预览（含文件内索引，不含 token）。
///
/// 展示字段（uid/nickname/email）一律走 `account::display_value` 折叠，与账号
/// 列表接口同口径：WorkBuddy 5.6 加密信封等对象/数组不得漏进前端渲染路径，
/// 否则会被当作 React 子节点渲染导致整树卸载（issue #100）。
/// `encrypted` 标记 access_token 为加密信封：可导入，但仅切换可用。
pub fn preview_accounts(text: &str) -> Result<Value, String> {
    let array = parse_accounts_json(text)?;
    let items: Vec<Value> = array
        .iter()
        .enumerate()
        .map(|(index, item)| {
            json!({
                "index": index,
                "uid": account::display_value(item, "uid"),
                "nickname": account::display_value(item, "nickname"),
                "email": account::display_value(item, "email"),
                "hasToken": account::secret_value(item, "access_token").is_some(),
                "encrypted": account::is_envelope(item, "access_token"),
            })
        })
        .collect();
    Ok(json!({ "accounts": items, "total": array.len() }))
}

/// 单条导入记录的合并动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MergeOutcome {
    /// 追加为新账号。
    Appended,
    /// 覆盖同 uid 的本地账号（保留导入记录原样）。
    Overwritten,
    /// 缺少可导入凭据（缺失 / 空串 / 普通对象），跳过。
    Skipped,
}

/// 纯函数：把一条导入记录合并进账号列表。
///
/// 按 uid 去重：同 uid 覆盖（保留导入记录原样）；uid 缺失或无法匹配则追加。
/// 凭据判定与采集口径一致（`account::secret_value`）：非空明文串或
/// `$wbEncrypted` 加密信封均可入库，缺失 / 空串 / 普通对象跳过。
pub(super) fn merge_import_record(accounts: &mut Vec<Value>, item: &Value) -> MergeOutcome {
    if account::secret_value(item, "access_token").is_none() {
        return MergeOutcome::Skipped;
    }
    if let Some(uid) = account::get_str(item, "uid").as_deref() {
        if let Some(existing) = accounts
            .iter_mut()
            .find(|a| account::get_str(a, "uid").as_deref() == Some(uid))
        {
            let mut replaced = item.clone();
            // 本地展示字段（备注 / 显示选择 / 标记）：备份未携带时保留本地值；
            // 备份自带则以备份为准（导入即恢复用户当时的展示配置）。
            // 白名单见 `account::LOCAL_ACCOUNT_FIELDS`——导入**旧备份**（不含 mark）时
            // 漏掉 `mark` 会把本地标记整条覆盖掉（R5-04）。
            account::inherit_local_fields(existing, &mut replaced);
            // 账号库不允许出现无 id 记录（删除按 id、列表 key、导出选择都依赖
            // id）：优先保留本地 id；本地也是无 id 的历史脏数据时补一个。
            if account::get_str(&replaced, "id").is_none() {
                replaced["id"] = account::get_str(existing, "id")
                    .map(Value::String)
                    .unwrap_or_else(|| Value::String(uuid::Uuid::new_v4().to_string()));
            }
            *existing = replaced;
            return MergeOutcome::Overwritten;
        }
    }
    // 追加同样必须带 id：缺 id 的记录在刷新回写时无法被按 id 覆盖，会被
    // 反复追加副本（issue #111）。
    let mut appended = item.clone();
    if account::get_str(&appended, "id").is_none() {
        appended["id"] = Value::String(uuid::Uuid::new_v4().to_string());
    }
    accounts.push(appended);
    MergeOutcome::Appended
}

/// 纯函数：解析文件文本并按选中索引把记录合并进账号列表，返回计数。
///
/// 不做文件读写，便于单测。索引越界视为跳过。
pub fn merge_import_records(
    accounts: &mut Vec<Value>,
    text: &str,
    indexes: &[usize],
) -> Result<ImportResult, String> {
    let array = parse_accounts_json(text)?;
    let mut result = ImportResult::default();
    for &index in indexes {
        match array.get(index) {
            None => result.skipped += 1,
            Some(item) => match merge_import_record(accounts, item) {
                MergeOutcome::Appended => result.imported += 1,
                MergeOutcome::Overwritten => {
                    result.imported += 1;
                    result.overwritten += 1;
                }
                MergeOutcome::Skipped => result.skipped += 1,
            },
        }
    }
    Ok(result)
}

/// 导入：读账号库 → 合并 → 写回，返回计数。
pub fn import_accounts(text: &str, indexes: &[usize]) -> Result<ImportResult, String> {
    let mut accounts = account::load_accounts();
    let result = merge_import_records(&mut accounts, text, indexes)?;
    account::save_accounts(&accounts).map_err(|e| format!("保存账号库失败：{e}"))?;
    Ok(result)
}
