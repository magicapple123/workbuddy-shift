//! 清理：先把目标解析成「计划」，dry-run 只返回计划，执行时逐项进回收站。
//!
//! 三条铁律（AUDIT §3 T3）：
//! 1. 任何目标校验失败都在**写入之前**返回错误（失败零写入）；
//! 2. 会话类操作（含单文件勾选）要求客户端退出，运行中直接拒绝并给原因；
//! 3. 删 DB 行前整库备份 + 整行快照，结果提示「云端可能重新拉回」。

use super::catalog::{self, Category};
use super::roots::{ensure_within, StorageRoots, Usage};
use super::sessions::{scan_sessions, Bundle};
use super::trash::{self, RowSnapshot, TrashEntry};
use crate::modules::variant::WbVariant;
use serde_json::{json, Value};
mod db;

pub(crate) use db::{backup_db, delete_row, remove_group_members};

use std::path::{Path, PathBuf};

/// 一次清理的上下文：根、档位、数据根与客户端是否在运行。
///
/// `client_running` / `data_root` 显式传入而不是在 core 内部读取：宿主负责进程探测，
/// 单测与沙箱夹具因此不需要真实客户端，也不会误读真实数据根。
#[derive(Clone, Debug)]
pub struct CleanContext {
    pub roots: StorageRoots,
    pub variant: WbVariant,
    pub data_root: PathBuf,
    pub client_running: bool,
}

/// 沙箱验证开关：声明「客户端已退出」，用于 `WB_SHIFT_HOME` 沙箱里的端到端验证。
///
/// 只读环境变量，不写任何东西；**必须**同时设置 `WB_SHIFT_HOME` 才生效——此时所有
/// 数据根都在沙箱内，不可能碰到真实会话。真实用户环境不设这两个变量。
pub const SANDBOX_CLIENT_EXITED_ENV: &str = "WB_SHIFT_STORAGE_ASSUME_CLIENT_EXITED";

/// 客户端是否在运行（扫描 / 页面状态与清理判定共用同一口径）。
pub fn client_running(variant: WbVariant) -> bool {
    !sandbox_client_exited() && crate::modules::process::is_workbuddy_running(variant)
}

fn sandbox_client_exited() -> bool {
    std::env::var_os(crate::modules::config::HOME_ENV_VAR).is_some()
        && std::env::var(SANDBOX_CLIENT_EXITED_ENV).ok().as_deref() == Some("1")
}

impl CleanContext {
    /// 生产入口：数据根取档位，客户端运行状态由进程探测给出。
    pub fn for_variant(roots: StorageRoots, variant: WbVariant) -> Self {
        Self {
            roots,
            variant,
            data_root: variant.data_root(),
            client_running: client_running(variant),
        }
    }

    fn store(&self) -> &Path {
        &self.roots.store
    }
}

/// 计划里的一项（文件或目录）。
#[derive(Clone, Debug)]
struct Work {
    root_id: &'static str,
    root: PathBuf,
    path: PathBuf,
    usage: Usage,
    category: String,
    session_id: Option<String>,
}

/// 要删的 DB 行。
#[derive(Clone, Debug)]
struct RowWork {
    bundle: Bundle,
}

#[derive(Default)]
struct Plan {
    works: Vec<Work>,
    rows: Vec<RowWork>,
    warnings: Vec<String>,
    labels: Vec<String>,
}

impl Plan {
    fn totals(&self) -> Usage {
        self.works.iter().fold(Usage::default(), |mut acc, work| {
            acc.add(work.usage);
            acc
        })
    }
}

/// 主入口：`dry_run=true` 只算计划；`false` 执行（宿主默认传 true）。
pub fn clean(ctx: &CleanContext, dry_run: bool, targets: &Value) -> Result<Value, String> {
    let list = targets
        .as_array()
        .ok_or_else(|| "targets 必须是数组".to_string())?;
    if list.is_empty() {
        return Err("没有选择要清理的内容".to_string());
    }

    let mut plan = Plan::default();
    let mut categories: Option<Vec<Category>> = None;
    let mut bundles: Option<Vec<Bundle>> = None;

    for target in list {
        let kind = target
            .get("kind")
            .and_then(Value::as_str)
            .ok_or_else(|| "target 缺少 kind".to_string())?;
        match kind {
            "category" => {
                let id = target
                    .get("id")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "category 缺少 id".to_string())?;
                let all = categories.get_or_insert_with(|| catalog::catalog(&ctx.roots).categories);
                let category = catalog::find(all, id).ok_or_else(|| format!("未知分类：{id}"))?;
                if !category.deletable {
                    return Err(category
                        .protected
                        .clone()
                        .unwrap_or_else(|| "该分类不允许直接删除".to_string()));
                }
                if category.group == "client" && ctx.client_running {
                    return Err("请先退出 WorkBuddy 客户端，再清理客户端缓存".to_string());
                }
                if category.units.is_empty() {
                    return Err(format!("{}：没有可清理的内容", category.label));
                }
                let root_id = root_id_of(&ctx.roots, &category.path)
                    .ok_or_else(|| "分类路径不在白名单根内".to_string())?;
                for unit in &category.units {
                    plan.works.push(Work {
                        root_id,
                        root: ctx.roots.by_id(root_id).unwrap_or_default(),
                        path: unit.path.clone(),
                        usage: unit.usage,
                        category: category.id.clone(),
                        session_id: None,
                    });
                }
                plan.labels.push(category.label.clone());
            }
            "session" => {
                let id = target
                    .get("id")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "session 缺少 id".to_string())?;
                let all = bundles
                    .get_or_insert_with(|| scan_sessions(ctx.store(), &ctx.data_root, ctx.variant));
                let bundle = all
                    .iter()
                    .find(|bundle| bundle.id == id)
                    .ok_or_else(|| format!("会话不存在：{id}"))?;
                if ctx.client_running {
                    return Err("请先退出 WorkBuddy 客户端，再删除会话".to_string());
                }
                let delete_row = target
                    .get("deleteRow")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let requested: Vec<String> = target
                    .get("entries")
                    .and_then(Value::as_array)
                    .map(|items| {
                        items
                            .iter()
                            .filter_map(Value::as_str)
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_default();

                let selected: Vec<&super::sessions::Entry> = if delete_row {
                    bundle.entries.iter().filter(|entry| entry.exists).collect()
                } else {
                    if requested.is_empty() {
                        return Err("请选择要删除的子项，或勾选「删除整条会话」".to_string());
                    }
                    let mut picked = Vec::new();
                    for kind in &requested {
                        let entry = bundle
                            .entries
                            .iter()
                            .find(|entry| entry.kind == kind)
                            .ok_or_else(|| format!("未知子项：{kind}"))?;
                        if entry.exists {
                            picked.push(entry);
                        }
                    }
                    picked
                };
                if selected.is_empty() && !delete_row {
                    return Err("所选子项在磁盘上都不存在，无需删除".to_string());
                }
                let root_id = variant_root_id(ctx.variant);
                for entry in selected {
                    plan.works.push(Work {
                        root_id,
                        root: ctx.data_root.clone(),
                        path: entry.path.clone(),
                        usage: entry.usage,
                        category: format!("session.{}", entry.kind),
                        session_id: Some(bundle.id.clone()),
                    });
                }
                if delete_row {
                    plan.rows.push(RowWork {
                        bundle: bundle.clone(),
                    });
                    plan.warnings.push(format!(
                        "「{}」已从本机删除；云端同步可能重新拉回该会话",
                        bundle.title
                    ));
                }
                plan.labels.push(format!(
                    "会话「{}」{}",
                    bundle.title,
                    if delete_row {
                        "（含数据库行）"
                    } else {
                        ""
                    }
                ));
            }
            "redundant" => {
                let group_id = target
                    .get("groupId")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "redundant 缺少 groupId".to_string())?;
                let keep = target.get("keepSessionId").and_then(Value::as_str);
                let all = bundles
                    .get_or_insert_with(|| scan_sessions(ctx.store(), &ctx.data_root, ctx.variant));
                if ctx.client_running {
                    return Err("请先退出 WorkBuddy 客户端，再清理冗余副本".to_string());
                }
                let doomed: Vec<&Bundle> = all
                    .iter()
                    .filter(|bundle| bundle.group_id.as_deref() == Some(group_id))
                    .filter(|bundle| bundle.row_status == "live")
                    .filter(|bundle| Some(bundle.id.as_str()) != keep)
                    .collect();
                if doomed.is_empty() {
                    return Err("该关联组没有可清理的冗余副本".to_string());
                }
                let root_id = variant_root_id(ctx.variant);
                for bundle in doomed {
                    for entry in bundle.entries.iter().filter(|entry| entry.exists) {
                        plan.works.push(Work {
                            root_id,
                            root: ctx.data_root.clone(),
                            path: entry.path.clone(),
                            usage: entry.usage,
                            category: format!("session.{}", entry.kind),
                            session_id: Some(bundle.id.clone()),
                        });
                    }
                    plan.rows.push(RowWork {
                        bundle: bundle.clone(),
                    });
                }
                plan.labels.push(format!("关联组 {group_id} 的冗余副本"));
            }
            other => return Err(format!("未知的 target 类型：{other}")),
        }
    }

    // 写前最后一道校验：每个路径都必须在白名单根内。
    for work in &plan.works {
        ensure_within(&work.root, &work.path)?;
    }

    if dry_run {
        return Ok(json!({
            "dryRun": true,
            "items": plan.works.iter().map(|work| json!({
                "path": work.path.to_string_lossy(),
                "category": work.category,
                "sessionId": work.session_id,
                "bytes": work.usage.bytes,
                "files": work.usage.files,
                "exists": work.path.exists(),
            })).collect::<Vec<_>>(),
            "rows": plan.rows.iter().map(|row| json!({
                "sessionId": row.bundle.id,
                "title": row.bundle.title,
                "variant": row.bundle.variant,
                "rowStatus": row.bundle.row_status,
            })).collect::<Vec<_>>(),
            "labels": plan.labels,
            "bytes": plan.totals().bytes,
            "files": plan.totals().files,
            "warnings": plan.warnings,
        }));
    }

    execute(ctx, plan)
}

fn execute(ctx: &CleanContext, plan: Plan) -> Result<Value, String> {
    let stamp = trash::new_entry_id(&ctx.roots);
    // 删行前先做整库备份；备份失败 → 零写入。
    let db_backups = if plan.rows.is_empty() {
        Vec::new()
    } else {
        vec![backup_db(ctx, &stamp)?]
    };

    let mut entry = TrashEntry {
        id: stamp.clone(),
        created_at: trash::now_ms(),
        items: Vec::new(),
        rows: plan
            .rows
            .iter()
            .filter_map(|row| {
                row.bundle.row.as_ref().map(|snapshot| RowSnapshot {
                    variant: row.bundle.variant.clone(),
                    table: "sessions".to_string(),
                    db_path: ctx
                        .data_root
                        .join("workbuddy.db")
                        .to_string_lossy()
                        .to_string(),
                    row: snapshot.clone(),
                })
            })
            .collect(),
        db_backups,
    };

    let mut failures: Vec<Value> = Vec::new();
    for work in &plan.works {
        if !work.path.exists() {
            continue;
        }
        match trash::move_unit(
            &ctx.roots,
            &stamp,
            work.root_id,
            &work.root,
            &work.path,
            &work.category,
            work.session_id.as_deref(),
        ) {
            Ok(item) => entry.items.push(item),
            Err(error) => {
                failures.push(json!({
                    "path": work.path.to_string_lossy(),
                    "error": error,
                }));
                break;
            }
        }
    }

    // 文件已移动的项先落盘清单，保证中断也能恢复。
    trash::write_manifest(&ctx.roots, &entry)?;
    let mut deleted_rows = 0_usize;
    let mut deleted_ids: Vec<String> = Vec::new();
    if failures.is_empty() {
        for row in &plan.rows {
            match delete_row(&ctx.data_root, &row.bundle.id) {
                Ok(()) => {
                    deleted_rows += 1;
                    deleted_ids.push(row.bundle.id.clone());
                }
                Err(error) => failures.push(json!({
                    "sessionId": row.bundle.id,
                    "error": error,
                })),
            }
        }
        if !deleted_ids.is_empty() {
            let _ = remove_group_members(ctx.store(), ctx.variant, &deleted_ids);
        }
        trash::write_manifest(&ctx.roots, &entry)?;
    }

    let moved = entry.items.iter().fold(Usage::default(), |mut acc, item| {
        acc.add(item.usage);
        acc
    });
    super::invalidate_scan_cache();

    Ok(json!({
        "dryRun": false,
        "trashId": stamp,
        "moved": entry.items.len(),
        "deletedRows": deleted_rows,
        "bytes": moved.bytes,
        "files": moved.files,
        "failures": failures,
        "warnings": plan.warnings,
        "labels": plan.labels,
        "manifest": trash::entry_dir(&ctx.roots, &stamp)?.join(trash::MANIFEST_NAME).to_string_lossy(),
    }))
}

fn variant_root_id(variant: WbVariant) -> &'static str {
    if variant.as_str() == "ai" {
        "ai"
    } else {
        "cn"
    }
}

fn root_id_of(roots: &StorageRoots, path: &Path) -> Option<&'static str> {
    for (id, root) in [
        ("store", &roots.store),
        ("cn", &roots.client_cn),
        ("ai", &roots.client_ai),
    ] {
        if path.starts_with(root) {
            return Some(id);
        }
    }
    None
}
