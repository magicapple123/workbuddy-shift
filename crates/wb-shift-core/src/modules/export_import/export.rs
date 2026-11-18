//! 导出：记录选择、文件名/路径校验与写文件。

use super::*;

/// 纯函数：从账号列表中挑出 id 命中的完整记录（含 token）。
pub fn select_export_records(accounts: &[Value], ids: &[String]) -> Result<Vec<Value>, String> {
    let ids: Vec<&str> = ids
        .iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();
    if ids.is_empty() {
        return Err("请先选择要导出的账号".to_string());
    }
    let mut exported: Vec<Value> = Vec::new();
    for id in ids {
        if let Some(acc) = accounts
            .iter()
            .find(|a| a.get("id").and_then(|v| v.as_str()) == Some(id))
        {
            exported.push(acc.clone());
        }
    }
    if exported.is_empty() {
        return Err("未找到要导出的账号".to_string());
    }
    Ok(exported)
}

/// 导出：按账号 id 列表返回完整记录（含 token）。
pub fn export_accounts(ids: &[String]) -> Result<Vec<Value>, String> {
    select_export_records(&account::load_accounts(), ids)
}

/// 导出文件名白名单：只允许 `workbuddy-shift-accounts-YYYY-MM-DD.json` 这类由前端生成的文件名。
pub(super) fn validate_export_file_name(file_name: &str) -> Result<(), String> {
    let name = file_name.trim();
    if name.is_empty()
        || name.contains('/')
        || name.contains('\\')
        || name.contains("..")
        || !name.ends_with(".json")
    {
        return Err("导出文件名不合法".to_string());
    }
    Ok(())
}

/// 校验导出目标路径：必须是绝对路径且以 `.json` 结尾（保存对话框产物）。
pub(super) fn validate_export_path(path: &str) -> Result<(), String> {
    let p = Path::new(path.trim());
    if !p.is_absolute() {
        return Err("导出路径必须是绝对路径".to_string());
    }
    if !p
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("json"))
        .unwrap_or(false)
    {
        return Err("导出文件名必须以 .json 结尾".to_string());
    }
    Ok(())
}

/// 纯函数：把记录数组写入指定目录下的 JSON 文件，返回完整路径。
pub fn write_records_to_file(
    dir: &Path,
    records: &[Value],
    file_name: &str,
) -> Result<String, String> {
    validate_export_file_name(file_name)?;
    let content = serde_json::to_string_pretty(records).map_err(|e| e.to_string())?;
    let path = dir.join(file_name);
    crate::modules::config::atomic_write(&path, &content)
        .map_err(|e| format!("写入导出文件失败：{e}"))?;
    Ok(path.to_string_lossy().into_owned())
}

/// 导出：按账号 id 列表把完整记录写入用户选择的路径（保存对话框产物），返回该路径。
pub fn export_accounts_to_path(ids: &[String], path: &str) -> Result<String, String> {
    validate_export_path(path)?;
    let records = select_export_records(&account::load_accounts(), ids)?;
    let path_buf = PathBuf::from(path.trim());
    let content = serde_json::to_string_pretty(&records).map_err(|e| e.to_string())?;
    crate::modules::config::atomic_write(&path_buf, &content)
        .map_err(|e| format!("写入导出文件失败：{e}"))?;
    Ok(path_buf.to_string_lossy().into_owned())
}
