//! 生命周期记录：符号链接防护、save/load、扫描、引用与状态机推进。

#[allow(unused_imports)]
use super::backup::*;
#[allow(unused_imports)]
use super::cleanup::*;
use super::*;

/// 拒绝根目录之下的符号链接跳转（含祖先路径）；不跟随、不删除链接目标。
pub(super) fn ensure_no_symlink_components(root: &Path, target: &Path) -> Result<(), String> {
    let Ok(relative) = target.strip_prefix(root) else {
        return Err(format!(
            "目标路径不在可信根内（{}），已停止删除",
            target.display()
        ));
    };
    let mut current = root.to_path_buf();
    for component in relative.components() {
        current.push(component);
        match std::fs::symlink_metadata(&current) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(format!(
                    "路径包含符号链接（{}），已停止删除",
                    current.display()
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
            Err(error) => {
                return Err(format!(
                    "路径状态无法确认（{}：{error}），已停止删除",
                    current.display()
                ));
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 维护记录的读写与扫描
// ---------------------------------------------------------------------------

/// 可靠写入维护记录（不裁剪、不合并，一条操作一份记录）。
pub fn save_lifecycle(paths: &SessionPaths, record: &BackupLifecycle) -> Result<(), String> {
    let file = lifecycle_file(paths, record.variant, &record.operation_id)?;
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("维护记录目录创建失败：{error}"))?;
    }
    let content = serde_json::to_string_pretty(record).map_err(|error| error.to_string())?;
    durable_write_str(&file, &content).map_err(|error| format!("维护记录写入失败：{error}"))?;
    Ok(())
}

/// 读取单条维护记录；缺失返回 None，损坏/未知版本返回 Err（调用方不得当作空）。
pub fn load_lifecycle(
    paths: &SessionPaths,
    variant: WbVariant,
    operation_id: &str,
) -> Result<Option<BackupLifecycle>, String> {
    let file = lifecycle_file(paths, variant, operation_id)?;
    let text = match std::fs::read_to_string(&file) {
        Ok(text) => text,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("维护记录不可读：{error}")),
    };
    let record: BackupLifecycle =
        serde_json::from_str(&text).map_err(|error| format!("维护记录内容损坏：{error}"))?;
    if record.version != LIFECYCLE_VERSION {
        return Err(format!(
            "维护记录版本 {} 不受支持（当前支持 {}）",
            record.version, LIFECYCLE_VERSION
        ));
    }
    Ok(Some(record))
}

/// 扫描全部维护记录；目录不可读、条目枚举失败、解析失败一律进 problems。
pub fn scan_lifecycle(paths: &SessionPaths) -> LifecycleScan {
    let mut scan = LifecycleScan::default();
    for variant in WbVariant::ALL {
        let dir = lifecycle_root(paths, variant);
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == ErrorKind::NotFound => continue,
            Err(error) => {
                scan.problems
                    .push(format!("维护记录目录不可读（{}）：{error}", dir.display()));
                continue;
            }
        };
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    scan.problems.push(format!(
                        "维护记录目录枚举失败（{}）：{error}",
                        dir.display()
                    ));
                    continue;
                }
            };
            let path = entry.path();
            if path.extension().is_none_or(|ext| ext != "json") {
                continue;
            }
            let name = path
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_default();
            let text = match std::fs::read_to_string(&path) {
                Ok(text) => text,
                Err(error) => {
                    scan.problems
                        .push(format!("维护记录不可读（{name}）：{error}"));
                    continue;
                }
            };
            match serde_json::from_str::<BackupLifecycle>(&text) {
                Ok(record) if record.version == LIFECYCLE_VERSION => {
                    if let Err(reason) = validate_lifecycle_identity(&path, variant, &record) {
                        scan.problems.push(reason);
                    } else {
                        scan.records.push(record);
                    }
                }
                Ok(record) => scan.problems.push(format!(
                    "维护记录版本不受支持（{name}：{}）",
                    record.version
                )),
                Err(error) => scan
                    .problems
                    .push(format!("维护记录解析失败（{name}）：{error}")),
            }
        }
    }
    scan.records.sort_by_key(|record| record.created_at);
    scan
}

/// 仍被维护记录引用的操作日志：日志裁剪不得丢弃它们（否则补清理失去依据）。
///
/// 扫描不完整（损坏/未知版本/归属不一致）时返回 `Err`：调用方不得把部分结果
/// 当成「未引用」而去裁剪——设计要求状态不明就不裁剪。
pub fn referenced_operation_ids(
    paths: &SessionPaths,
) -> Result<std::collections::HashSet<String>, Vec<String>> {
    let scan = scan_lifecycle(paths);
    if !scan.problems.is_empty() {
        return Err(scan.problems);
    }
    Ok(scan
        .records
        .into_iter()
        .map(|record| record.operation_id)
        .collect())
}

/// 维护记录必须落在本档位目录、kind 受支持、文件名等于校验后的 UUID。
pub(super) fn validate_lifecycle_identity(
    path: &Path,
    dir_variant: WbVariant,
    record: &BackupLifecycle,
) -> Result<(), String> {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_default();
    if record.variant != dir_variant {
        return Err(format!("维护记录档位与目录不一致（{name}）"));
    }
    if record.kind != "copy" && record.kind != "sync" {
        return Err(format!("维护记录 kind 不受支持（{name}：{}）", record.kind));
    }
    let id = validated_operation_id(&record.operation_id).map_err(|_| {
        format!(
            "维护记录操作标识不是合法 UUID（{name}：{}）",
            record.operation_id
        )
    })?;
    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or_default();
    if stem != id {
        return Err(format!("维护记录文件名与操作标识不一致（{name}）"));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 生命周期推进
// ---------------------------------------------------------------------------

/// 开始一次操作：先可靠写 allocating 记录，再创建专属目录。
///
/// 返回的记录由调用方持有，后续阶段推进都基于它；目录创建失败时记录保留，
/// 由下次维护入口按「allocating 且无业务日志」回收（本版契约保证业务前 protected）。
pub fn begin_operation(
    paths: &SessionPaths,
    variant: WbVariant,
    kind: &str,
    session_id: Option<String>,
    title: Option<String>,
) -> Result<BackupLifecycle, String> {
    let operation_id = uuid::Uuid::new_v4().to_string();
    let record = BackupLifecycle {
        version: LIFECYCLE_VERSION,
        operation_id: operation_id.clone(),
        variant,
        kind: kind.to_string(),
        state: BackupState::Allocating,
        created_at: now_ms(),
        updated_at: now_ms(),
        last_error: None,
        session_id,
        title,
    };
    save_lifecycle(paths, &record)?;
    let dir = transaction_dir(paths, variant, &operation_id)?;
    if let Some(parent) = dir.parent() {
        std::fs::create_dir_all(parent).map_err(|error| format!("临时目录创建失败：{error}"))?;
        sync_dir(parent).map_err(|error| format!("临时目录持久化失败：{error}"))?;
    }
    match std::fs::create_dir(&dir) {
        Ok(()) => {}
        Err(error) => {
            return Err(format!(
                "操作专属目录创建失败（{error}），未写入任何业务内容"
            ));
        }
    }
    sync_dir(&dir).map_err(|error| format!("操作专属目录持久化失败：{error}"))?;
    Ok(record)
}

/// 记录状态推进：可靠落盘后才更新内存副本（失败时内存不变，不产生虚假状态）。
pub fn advance_record(
    paths: &SessionPaths,
    record: &mut BackupLifecycle,
    state: BackupState,
    last_error: Option<&str>,
) -> Result<(), String> {
    let mut candidate = record.clone();
    candidate.state = state;
    candidate.updated_at = now_ms();
    candidate.last_error = last_error.map(str::to_string);
    save_lifecycle(paths, &candidate)?;
    *record = candidate;
    Ok(())
}

/// 备份完备、即将或已经写业务：转 protected。
pub fn mark_protected(paths: &SessionPaths, record: &mut BackupLifecycle) -> Result<(), String> {
    let dir = transaction_dir(paths, record.variant, &record.operation_id)?;
    sync_tree(&dir).map_err(|error| format!("备份目录持久化失败：{error}"))?;
    advance_record(paths, record, BackupState::Protected, None)
}

/// 已可靠确认完成（或已验证安全终止）：允许回收；清理前若此步失败，业务成功不受影响。
pub fn mark_cleanup_pending(
    paths: &SessionPaths,
    record: &mut BackupLifecycle,
    reason: Option<&str>,
) -> Result<(), String> {
    advance_record(paths, record, BackupState::CleanupPending, reason)
}
