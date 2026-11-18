//! 执行同步：备份 → 原子写；以及切号钩子与分项结果。

use super::*;

/// 执行同步：先备份目标文件，再逐文件原子写；备份失败即零写入。
pub fn apply(
    ctx: &SettingsSyncContext,
    source: Option<&str>,
    dry_run: bool,
) -> Result<Value, String> {
    let plan = preview(ctx, source)?;
    if dry_run {
        return Ok(plan);
    }
    let source_dir = PathBuf::from(
        plan.get("source")
            .and_then(|source| source.get("directory"))
            .and_then(Value::as_str)
            .unwrap_or_default(),
    );
    let source_global = read_prefs(&source_dir.join("global").join("preferences.json"))
        .ok_or_else(|| "源账号设置读取失败".to_string())?;
    let now = now_ms();
    let stamp = timestamp_id(now);
    let mut results = Vec::new();

    for target in plan
        .get("targets")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
    {
        let uid = target
            .get("uid")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if target.get("skipped").and_then(Value::as_bool) == Some(true) {
            results.push(target.clone());
            continue;
        }
        let Some(target_account) = ctx
            .accounts
            .iter()
            .find(|account| account_uid(account) == uid)
        else {
            results.push(json!({ "uid": uid, "skipped": true, "reason": "账号不存在" }));
            continue;
        };
        let target_dir = ctx.account_dir(target_account);
        match sync_one(
            ctx,
            &source_dir,
            &source_global,
            &target_dir,
            uid,
            &stamp,
            now,
        ) {
            Ok(report) => results.push(report),
            Err(error) => results.push(json!({
                "uid": uid,
                "skipped": true,
                "reason": error,
            })),
        }
    }

    let summary = json!({
        "at": now,
        "variant": plan.get("variant").cloned().unwrap_or(Value::Null),
        "source": plan.get("source").cloned().unwrap_or(Value::Null),
        "targets": results,
        "backupRoot": ctx.store_root.join("backups").join("settings").to_string_lossy(),
    });
    write_state(&ctx.store_root, &summary);
    Ok(summary)
}

fn sync_one(
    ctx: &SettingsSyncContext,
    source_dir: &Path,
    source_global: &Map<String, Value>,
    target_dir: &Path,
    target_uid: &str,
    stamp: &str,
    now: i64,
) -> Result<Value, String> {
    let global_file = target_dir.join("global").join("preferences.json");
    let target_global = read_prefs(&global_file).unwrap_or_default();
    let outcome = merge_preferences(source_global, &target_global, target_uid, now);

    // 先算全部待写文件（global + 目标已有 scope），再统一备份。
    let mut writes: Vec<(PathBuf, Map<String, Value>)> = Vec::new();
    if !outcome.changed.is_empty() {
        writes.push((global_file.clone(), outcome.merged.clone()));
    }
    let mut scope_changes = Vec::new();
    for scope in scope_names(target_dir) {
        let source_file = source_dir
            .join("scoped")
            .join(&scope)
            .join("preferences.json");
        let Some(source_prefs) = read_prefs(&source_file) else {
            continue;
        };
        let target_file = target_dir
            .join("scoped")
            .join(&scope)
            .join("preferences.json");
        let target_prefs = read_prefs(&target_file).unwrap_or_default();
        let scope_outcome = merge_preferences(&source_prefs, &target_prefs, target_uid, now);
        if scope_outcome.changed.is_empty() {
            continue;
        }
        scope_changes.push(json!({ "scope": scope, "changes": scope_outcome.changed }));
        writes.push((target_file, scope_outcome.merged));
    }

    if writes.is_empty() {
        return Ok(json!({
            "uid": target_uid,
            "skipped": false,
            "changes": [],
            "scopes": [],
            "backup": Value::Null,
            "note": "目标设置已与源账号一致",
        }));
    }

    let backup_dir = ctx
        .store_root
        .join("backups")
        .join("settings")
        .join(target_uid)
        .join(stamp);
    backup_targets(target_dir, &writes, &backup_dir, target_uid)?;

    let mut written = Vec::new();
    for (path, content) in &writes {
        let text = serde_json::to_string_pretty(content).map_err(|error| error.to_string())?;
        atomic_write(path, &text)
            .map_err(|error| format!("写入 {} 失败：{error}", path.display()))?;
        written.push(path.to_string_lossy().to_string());
    }

    Ok(json!({
        "uid": target_uid,
        "skipped": false,
        "changes": outcome.changed,
        "scopes": scope_changes,
        "backup": backup_dir.to_string_lossy(),
        "written": written,
    }))
}

/// 备份目标现有文件与一份 manifest；任何一步失败都返回错误（调用方零写入）。
fn backup_targets(
    target_dir: &Path,
    writes: &[(PathBuf, Map<String, Value>)],
    backup_dir: &Path,
    target_uid: &str,
) -> Result<(), String> {
    std::fs::create_dir_all(backup_dir).map_err(|error| format!("创建备份目录失败：{error}"))?;
    let mut manifest = Vec::new();
    for (path, _) in writes {
        let rel = path
            .strip_prefix(target_dir)
            .map_err(|_| "备份路径不在目标账号目录内".to_string())?;
        let dest = backup_dir.join(rel);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("创建备份子目录失败：{error}"))?;
        }
        let existed = path.is_file();
        if existed {
            std::fs::copy(path, &dest).map_err(|error| {
                format!("备份 {} 失败（未写入任何设置）：{error}", path.display())
            })?;
            let size = std::fs::metadata(&dest).map(|meta| meta.len()).unwrap_or(0);
            manifest.push(json!({
                "file": rel.to_string_lossy(),
                "bytes": size,
                "existed": true,
            }));
        } else {
            manifest.push(json!({
                "file": rel.to_string_lossy(),
                "bytes": 0,
                "existed": false,
            }));
        }
    }
    let payload = json!({
        "uid": target_uid,
        "createdAt": now_ms(),
        "target": target_dir.to_string_lossy(),
        "files": manifest,
        "restore": "把本目录下的文件按相对路径复制回目标账号目录即可还原",
    });
    let text = serde_json::to_string_pretty(&payload).map_err(|error| error.to_string())?;
    atomic_write(&backup_dir.join("manifest.json"), &text)
        .map_err(|error| format!("写入备份清单失败（未写入任何设置）：{error}"))?;
    Ok(())
}

/// 切号钩子：`restart=true` 时按配置把源账号设置同步到目标账号。
///
/// 失败不阻断切换——返回结果由调用方挂到切换报告里，用户能看到原因。
pub fn sync_on_switch(
    ctx: &SettingsSyncContext,
    variant: WbVariant,
    source_uid: &str,
    target_uid: &str,
    restarted: bool,
) -> Value {
    let config = load_config(&ctx.store_root);
    if !config.enabled || !config.auto_on_switch {
        return json!({ "status": "disabled", "reason": "设置同步已关闭" });
    }
    if !restarted {
        // 不重启时客户端不会重读 preferences，写了也会被内存里的旧值覆盖。
        return json!({ "status": "skipped", "reason": "客户端未重启，设置可能被覆盖" });
    }
    if source_uid.is_empty() || target_uid.is_empty() {
        return json!({ "status": "skipped", "reason": "缺少源账号或目标账号 uid" });
    }
    if source_uid == target_uid {
        return json!({ "status": "skipped", "reason": "源账号与目标账号相同" });
    }
    let Some(source_account) = ctx
        .accounts
        .iter()
        .find(|account| account_uid(account) == source_uid)
    else {
        return json!({ "status": "skipped", "reason": "源账号不在账号库中" });
    };
    let Some(target_account) = ctx
        .accounts
        .iter()
        .find(|account| account_uid(account) == target_uid)
    else {
        return json!({ "status": "skipped", "reason": "目标账号不在账号库中" });
    };
    if account::variant_of(source_account) != variant
        || account::variant_of(target_account) != variant
    {
        return json!({ "status": "rejected", "reason": "跨档位同步已拒绝（cn / intl 数据根隔离）" });
    }
    let source_dir = ctx.account_dir(source_account);
    let target_dir = ctx.account_dir(target_account);
    if !target_dir.is_dir() {
        return json!({ "status": "skipped", "reason": "目标账号从未在本机登录（目录不存在）" });
    }
    let Some(source_global) = read_prefs(&source_dir.join("global").join("preferences.json"))
    else {
        return json!({ "status": "skipped", "reason": "源账号没有 preferences.json" });
    };
    let stamp = timestamp_id(now_ms());
    match sync_one(
        ctx,
        &source_dir,
        &source_global,
        &target_dir,
        target_uid,
        &stamp,
        now_ms(),
    ) {
        Ok(report) => json!({ "status": "synced", "report": report }),
        Err(error) => json!({ "status": "failed", "reason": error }),
    }
}
