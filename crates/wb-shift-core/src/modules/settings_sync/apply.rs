//! 执行同步：备份 → 原子写；切号钩子的留痕；备份列举与还原。
//!
//! 三条硬口径：
//! - 「真实变更」才落盘：目标只剩主题 `revision` 归属改写时直接返回 `unchanged`，
//!   不写盘、不产生备份（否则每次都会多出一个只有 manifest 的空备份目录）；
//! - 每次执行都往状态文件追加一条记录（`trigger: auto | manual`），自动同步也留痕；
//! - 备份可还原：先校验 uid / stamp，再把当前文件备份一份，最后逐文件回拷。

use super::*;
use std::path::Component;

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
            .unwrap_or_default()
            .to_string();
        let label = target.get("label").cloned().unwrap_or(Value::Null);
        if target.get("status").and_then(Value::as_str) == Some("skipped") {
            results.push(target.clone());
            continue;
        }
        let Some(target_account) = ctx
            .accounts
            .iter()
            .find(|account| account_uid(account) == uid)
        else {
            results.push(json!({
                "uid": uid,
                "label": label,
                "status": "skipped",
                "reason": "账号不存在",
                "changes": [],
            }));
            continue;
        };
        let target_dir = ctx.account_dir(target_account);
        match sync_one(
            ctx,
            &source_dir,
            &source_global,
            &target_dir,
            &uid,
            &stamp,
            now,
        ) {
            Ok(report) => {
                let mut report = report;
                report["label"] = label;
                results.push(report);
            }
            Err(error) => results.push(json!({
                "uid": uid,
                "label": label,
                "status": "failed",
                "reason": error,
                "changes": [],
            })),
        }
    }

    let counts = json!({
        "synced": count_status(&results, "synced"),
        "unchanged": count_status(&results, "unchanged"),
        "skipped": count_status(&results, "skipped"),
        "failed": count_status(&results, "failed"),
    });
    let status = overall_status(&counts);
    let summary = json!({
        "at": now,
        "trigger": "manual",
        "status": status,
        "variant": plan.get("variant").cloned().unwrap_or(Value::Null),
        "source": plan.get("source").cloned().unwrap_or(Value::Null),
        "targets": results,
        "counts": counts,
        "backupRoot": backup_root(&ctx.store_root).to_string_lossy(),
    });
    push_state(&ctx.store_root, &summary);
    Ok(summary)
}

fn count_status(results: &[Value], wanted: &str) -> usize {
    results
        .iter()
        .filter(|item| item.get("status").and_then(Value::as_str) == Some(wanted))
        .count()
}

/// 一次执行的整体状态：有写入 → synced（有失败则 partial）；只有 unchanged / skipped 时如实回报。
fn overall_status(counts: &Value) -> &'static str {
    let synced = counts.get("synced").and_then(Value::as_u64).unwrap_or(0);
    let unchanged = counts.get("unchanged").and_then(Value::as_u64).unwrap_or(0);
    let failed = counts.get("failed").and_then(Value::as_u64).unwrap_or(0);
    if synced > 0 && failed > 0 {
        "partial"
    } else if synced > 0 {
        "synced"
    } else if failed > 0 {
        "failed"
    } else if unchanged > 0 {
        "unchanged"
    } else {
        "skipped"
    }
}

/// 同步单个目标账号：先算全部待写文件，统一备份后再原子写。
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
    if outcome.has_real_changes() {
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
        if !scope_outcome.has_real_changes() {
            continue;
        }
        scope_changes.push(json!({ "scope": scope, "changes": scope_outcome.changed }));
        writes.push((target_file, scope_outcome.merged));
    }

    if writes.is_empty() {
        // 没有真实变更：不写盘、不建备份目录，只回报「已一致」。
        return Ok(json!({
            "uid": target_uid,
            "status": "unchanged",
            "changes": [],
            "cosmetic": outcome.cosmetic,
            "themeRewrite": false,
            "scopes": [],
            "backup": Value::Null,
            "note": "目标设置已与源账号一致",
        }));
    }

    let backup_dir = backup_root(&ctx.store_root)
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
        "status": "synced",
        "changes": outcome.changed,
        "cosmetic": outcome.cosmetic,
        "themeRewrite": outcome.theme_rewrite,
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
        "restore": "在设置页「备份与还原」里点还原，或把本目录下的文件按相对路径复制回目标账号目录",
    });
    let text = serde_json::to_string_pretty(&payload).map_err(|error| error.to_string())?;
    atomic_write(&backup_dir.join("manifest.json"), &text)
        .map_err(|error| format!("写入备份清单失败（未写入任何设置）：{error}"))?;
    Ok(())
}

/// 切号钩子：`restart=true` 时按配置把源账号设置同步到目标账号。
///
/// 失败不阻断切换——返回结果由调用方挂到切换报告里，用户能看到原因。无论成功、跳过
/// 还是失败都写一条 `trigger: "auto"` 的记录，页面上的「上次结果」才看得见自动同步。
pub fn sync_on_switch(
    ctx: &SettingsSyncContext,
    variant: WbVariant,
    source_uid: &str,
    target_uid: &str,
    restarted: bool,
) -> Value {
    let config = load_config(&ctx.store_root);
    if !config.enabled || !config.auto_on_switch {
        // 开关关闭不是一次「执行」，不写历史，避免把离线按钮状态刷进记录。
        return json!({ "status": "disabled", "reason": "设置同步已关闭" });
    }
    if !restarted {
        // 不重启时客户端不会重读 preferences，写了也会被内存里的旧值覆盖。
        return record_auto(
            ctx,
            variant,
            source_uid,
            target_uid,
            "skipped",
            Some("客户端未重启，设置可能被覆盖"),
            Vec::new(),
        );
    }
    if source_uid.is_empty() || target_uid.is_empty() {
        return record_auto(
            ctx,
            variant,
            source_uid,
            target_uid,
            "skipped",
            Some("缺少源账号或目标账号 uid"),
            Vec::new(),
        );
    }
    if source_uid == target_uid {
        return record_auto(
            ctx,
            variant,
            source_uid,
            target_uid,
            "skipped",
            Some("源账号与目标账号相同"),
            Vec::new(),
        );
    }
    let Some(source_account) = ctx
        .accounts
        .iter()
        .find(|account| account_uid(account) == source_uid)
    else {
        return record_auto(
            ctx,
            variant,
            source_uid,
            target_uid,
            "skipped",
            Some("源账号不在账号库中"),
            Vec::new(),
        );
    };
    let Some(target_account) = ctx
        .accounts
        .iter()
        .find(|account| account_uid(account) == target_uid)
    else {
        return record_auto(
            ctx,
            variant,
            source_uid,
            target_uid,
            "skipped",
            Some("目标账号不在账号库中"),
            Vec::new(),
        );
    };
    if account::variant_of(source_account) != variant
        || account::variant_of(target_account) != variant
    {
        return record_auto(
            ctx,
            variant,
            source_uid,
            target_uid,
            "rejected",
            Some("跨档位同步已拒绝（cn / intl 数据根隔离）"),
            Vec::new(),
        );
    }
    let source_dir = ctx.account_dir(source_account);
    let target_dir = ctx.account_dir(target_account);
    if !target_dir.is_dir() {
        return record_auto(
            ctx,
            variant,
            source_uid,
            target_uid,
            "skipped",
            Some("目标账号从未在本机登录（目录不存在）"),
            Vec::new(),
        );
    }
    let Some(source_global) = read_prefs(&source_dir.join("global").join("preferences.json"))
    else {
        return record_auto(
            ctx,
            variant,
            source_uid,
            target_uid,
            "skipped",
            Some("源账号没有 preferences.json"),
            Vec::new(),
        );
    };
    let now = now_ms();
    let stamp = timestamp_id(now);
    match sync_one(
        ctx,
        &source_dir,
        &source_global,
        &target_dir,
        target_uid,
        &stamp,
        now,
    ) {
        Ok(mut report) => {
            report["label"] = json!(account_label(target_account));
            let status = report
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("synced")
                .to_string();
            record_auto(
                ctx,
                variant,
                source_uid,
                target_uid,
                &status,
                None,
                vec![report],
            )
        }
        Err(error) => record_auto(
            ctx,
            variant,
            source_uid,
            target_uid,
            "failed",
            Some(&error),
            vec![json!({
                "uid": target_uid,
                "label": account_label(target_account),
                "status": "failed",
                "reason": error,
                "changes": [],
            })],
        ),
    }
}

/// 写一条 `trigger: "auto"` 的历史，并返回挂到切换报告里的精简结果。
fn record_auto(
    ctx: &SettingsSyncContext,
    variant: WbVariant,
    source_uid: &str,
    target_uid: &str,
    status: &str,
    reason: Option<&str>,
    targets: Vec<Value>,
) -> Value {
    let label_of = |uid: &str| -> Value {
        ctx.accounts
            .iter()
            .find(|account| account_uid(account) == uid)
            .map(|account| json!(account_label(account)))
            .unwrap_or(Value::Null)
    };
    let entry = json!({
        "at": now_ms(),
        "trigger": "auto",
        "status": status,
        "reason": reason,
        "variant": variant.as_str(),
        // 跳过分支没有 target 明细，这里仍记下「本来要同步到哪个账号」。
        "targetUid": target_uid,
        "source": {
            "uid": source_uid,
            "label": label_of(source_uid),
            "directory": Value::Null,
        },
        "targets": targets,
        "counts": {
            "synced": if status == "synced" { 1 } else { 0 },
            "unchanged": if status == "unchanged" { 1 } else { 0 },
            "skipped": if status == "skipped" { 1 } else { 0 },
            "failed": if status == "failed" { 1 } else { 0 },
        },
        "backupRoot": backup_root(&ctx.store_root).to_string_lossy(),
    });
    push_state(&ctx.store_root, &entry);

    let mut report = json!({ "status": status });
    if let Some(reason) = reason {
        report["reason"] = json!(reason);
    }
    if let Some(first) = entry
        .get("targets")
        .and_then(Value::as_array)
        .and_then(|items| items.first())
    {
        report["report"] = first.clone();
    }
    report
}

/// 列举某账号的备份（按时间戳倒序）。`uid` 走字符白名单校验，挡路径穿越。
pub fn list_backups(ctx: &SettingsSyncContext, uid: &str) -> Result<Value, String> {
    let uid = validate_uid(uid)?;
    let root = backup_root(&ctx.store_root).join(&uid);
    let mut entries = Vec::new();
    if root.is_dir() {
        let items = std::fs::read_dir(&root)
            .map_err(|error| format!("读取备份目录失败：{error}"))?
            .flatten();
        for item in items {
            let stamp = item.file_name().to_string_lossy().to_string();
            // 只认 `YYYYMMDD-HHMMSS`；`pre-restore-*` 之类的内部目录不列出来。
            if !is_valid_stamp(&stamp) || !item.path().is_dir() {
                continue;
            }
            let dir = item.path();
            let manifest = read_json_value(&dir.join("manifest.json"));
            entries.push(json!({
                "stamp": stamp,
                "createdAt": manifest.as_ref()
                    .and_then(|value| value.get("createdAt"))
                    .cloned()
                    .unwrap_or(Value::Null),
                "target": manifest.as_ref()
                    .and_then(|value| value.get("target"))
                    .cloned()
                    .unwrap_or(Value::Null),
                "files": manifest.as_ref()
                    .and_then(|value| value.get("files"))
                    .cloned()
                    .unwrap_or_else(|| json!([])),
                "path": dir.to_string_lossy(),
                "hasManifest": manifest.is_some(),
            }));
        }
    }
    entries.sort_by(|left, right| {
        let key = |value: &Value| value["stamp"].as_str().unwrap_or_default().to_string();
        key(right).cmp(&key(left))
    });
    Ok(json!({
        "uid": uid,
        "backupRoot": backup_root(&ctx.store_root).to_string_lossy(),
        "entries": entries,
    }))
}

/// 还原某账号的某次备份：先校验 → 再把当前文件备份一份 → 最后逐文件回拷。
///
/// 备份清单里 `existed: false` 的文件是同步时新建的，还原时删掉，回到同步前状态。
pub fn restore_backup(
    ctx: &SettingsSyncContext,
    uid: &str,
    stamp: &str,
) -> Result<Value, String> {
    let uid = validate_uid(uid)?;
    if !is_valid_stamp(stamp) {
        return Err(format!("备份时间戳不合法（需 YYYYMMDD-HHMMSS）：{stamp}"));
    }
    let backup_dir = backup_root(&ctx.store_root).join(&uid).join(stamp);
    if !backup_dir.is_dir() {
        return Err(format!("备份不存在：{stamp}"));
    }
    let manifest = read_json_value(&backup_dir.join("manifest.json"))
        .ok_or_else(|| "备份清单缺失或损坏，无法还原".to_string())?;
    let Some(account) = ctx
        .accounts
        .iter()
        .find(|account| account_uid(account) == uid)
    else {
        return Err(format!("账号不在账号库中：{uid}"));
    };
    let target_dir = ctx.account_dir(account);
    if !target_dir.is_dir() {
        return Err("目标账号目录不存在（从未在本机登录），无法还原".to_string());
    }
    let files = manifest
        .get("files")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if files.is_empty() {
        return Err("备份清单里没有任何文件".to_string());
    }

    // 先把全部相对路径校验完，再动盘：任何一条不合法都不写。
    let mut plans: Vec<RestorePlan> = Vec::new();
    for file in &files {
        let rel = file.get("file").and_then(Value::as_str).unwrap_or_default();
        let safe = safe_relative(rel)?;
        let existed = file
            .get("existed")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        let source = backup_dir.join(&safe);
        if existed && !source.is_file() {
            return Err(format!("备份文件缺失：{rel}"));
        }
        plans.push(RestorePlan {
            source,
            dest: target_dir.join(&safe),
            existed,
            rel: rel.to_string(),
        });
    }

    // 还原前把当前文件再备份一份，随时可以「还原这次还原」。
    let pre_dir = unique_dir(
        &backup_root(&ctx.store_root).join(&uid),
        &format!("pre-restore-{}", timestamp_id(now_ms())),
    );
    let current: Vec<(PathBuf, Map<String, Value>)> = plans
        .iter()
        .map(|plan| (plan.dest.clone(), Map::new()))
        .collect();
    backup_targets(&target_dir, &current, &pre_dir, &uid)?;

    let mut restored = Vec::new();
    for plan in &plans {
        if plan.existed {
            if let Some(parent) = plan.dest.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|error| format!("创建目录 {} 失败：{error}", parent.display()))?;
            }
            std::fs::copy(&plan.source, &plan.dest).map_err(|error| {
                format!("还原 {} 失败：{error}", plan.dest.display())
            })?;
            restored.push(json!({ "file": plan.rel.clone(), "action": "restored" }));
        } else if plan.dest.is_file() {
            std::fs::remove_file(&plan.dest)
                .map_err(|error| format!("清理 {} 失败：{error}", plan.dest.display()))?;
            restored.push(json!({ "file": plan.rel.clone(), "action": "removed" }));
        } else {
            restored.push(json!({ "file": plan.rel.clone(), "action": "absent" }));
        }
    }

    Ok(json!({
        "uid": uid,
        "stamp": stamp,
        "restoredAt": now_ms(),
        "target": target_dir.to_string_lossy(),
        "files": restored,
        "preRestoreBackup": pre_dir.to_string_lossy(),
    }))
}

/// 一条还原计划：从哪拷、拷到哪、原来是否存在。
struct RestorePlan {
    source: PathBuf,
    dest: PathBuf,
    existed: bool,
    rel: String,
}

/// 账号 uid 白名单：只允许字母 / 数字 / `-` / `_`，挡 `../` 与绝对路径。
fn validate_uid(uid: &str) -> Result<String, String> {
    let trimmed = uid.trim();
    if trimmed.is_empty() {
        return Err("缺少账号 uid".to_string());
    }
    if trimmed.len() > 128 {
        return Err("账号 uid 过长".to_string());
    }
    if !trimmed
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
    {
        return Err(format!("账号 uid 含非法字符：{trimmed}"));
    }
    Ok(trimmed.to_string())
}

/// 备份时间戳白名单：只接受 `YYYYMMDD-HHMMSS`。
fn is_valid_stamp(stamp: &str) -> bool {
    let bytes = stamp.as_bytes();
    if bytes.len() != 15 || bytes[8] != b'-' {
        return false;
    }
    if !bytes
        .iter()
        .enumerate()
        .all(|(index, byte)| index == 8 || byte.is_ascii_digit())
    {
        return false;
    }
    let number = |start: usize, end: usize| -> i32 {
        stamp[start..end].parse::<i32>().unwrap_or(-1)
    };
    let month = number(4, 6);
    let day = number(6, 8);
    let hour = number(9, 11);
    let minute = number(11, 13);
    let second = number(13, 15);
    (1..=12).contains(&month)
        && (1..=31).contains(&day)
        && (0..=23).contains(&hour)
        && (0..=59).contains(&minute)
        && (0..=59).contains(&second)
}

/// 清单里的相对路径必须是纯 `Normal` 组件：不含 `..`、根、盘符。
fn safe_relative(rel: &str) -> Result<PathBuf, String> {
    let path = Path::new(rel);
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => out.push(part),
            _ => return Err(format!("备份清单里有非法路径：{rel}")),
        }
    }
    if out.as_os_str().is_empty() {
        return Err(format!("备份清单里有非法路径：{rel}"));
    }
    Ok(out)
}

fn read_json_value(path: &Path) -> Option<Value> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str::<Value>(&text).ok()
}

/// 取一个当前不存在的目录名：`base`、`base-1`、`base-2`……
fn unique_dir(parent: &Path, base: &str) -> PathBuf {
    let mut candidate = parent.join(base);
    let mut index = 1;
    while candidate.exists() {
        candidate = parent.join(format!("{base}-{index}"));
        index += 1;
    }
    candidate
}
