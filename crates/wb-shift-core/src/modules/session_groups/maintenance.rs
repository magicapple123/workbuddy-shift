//! 失效关联组的批量清理：组内所有成员指向的会话行都已不存活时，删除整组的关联关系。
//!
//! 语义与「会话管理页隐藏掉的组」严格一致：只要有**任意一个**成员还活着就不许删
//! （部分成员缺失的组是可修复的，删了等于替用户放弃关联）。两侧共用同一份组级汇总
//! （[`super::liveness::GroupLiveness`]），因此**管理页隐藏的组与清理能删的组是同一集合**。
//!
//! 存活判定按 [`SessionClient`] 分派——两侧的会话「行」根本不在同一个地方：
//!
//! - `Workbuddy`：`workbuddy.db` 的 `sessions` 表（`group_any_alive`，按成员档位读库）；
//! - `VscodeExt` / `CodebuddyIde`：工作区索引条目**或**会话目录
//!   （`vscode_session_sync::liveness`，与管理页共用）。
//!
//! 索引有四态，写路径只接受「能证明全死」的结论：`Absent`（目录不存在）、`Unavailable`
//! （目录在但一份索引都没解析成功）与 `Partial`（部分工作区索引损坏）在**索引与目录都没
//! 命中**时都不足以说明会话已消失，此时**拒绝整次清理**并指出是哪个账号、为什么判不了，
//! 绝不「读不到就当全死」。会话目录存在则是正向证据：`Partial` 下「索引条目在损坏分片里、
//! 但目录还在」的组按存活处理，不再整次拒绝。
//! IDE 两档共用一份关联表，因此按 group/member 的 variant 过滤
//! （与 `links_preview_in_for_variant` 同口径），不同档位的组互不影响。
//!
//! 只删管理关系，不碰任何账号里的会话内容；写盘前留一份关联主表的时间戳备份
//! （关联存储没有操作日志，删组后无法凭主表复原）——文件名随命名空间走，
//! `session_links.json` / `vscode_session_links.json` / `codebuddy_ide_session_links.json`
//! 各自生成 `<文件名>.bak-<YYYYMMDD-HHMMSS>`。

use super::*;

/// 关联主表读不出来时的统一文案：`prune` 的两处 `Err`、列表载荷的 `pruneBlocked` 与
/// `view::detail` 的错误**共用**这一句，保证「`pruneBlocked` 非空 ⇔ `prune` 会拒绝」并且
/// 逐字相同，也避免同一句用户可见文案存在多份副本各自漂移。
///
/// 主表**不存在**不走这里：那是首次使用，`prune` 返回 0 组（不是拒绝），列表的
/// `pruneBlocked` 必须保持 `null`。
pub(super) fn store_unavailable_reason(reason: &str) -> String {
    format!("会话关联组暂不可用：{reason}")
}

/// 待清理的组：id 与成员数（成员数只用于回报，删组走 [`delete_group_in`]）。
pub(super) struct DeadGroup {
    pub(super) id: String,
    pub(super) members: usize,
}

/// 清理「组内所有成员指向的会话行都已不存活」的关联组。
///
/// `dry_run = true` 只统计不写盘；`dry_run = false` 先在关联存储锁内重新读表、重新判定，
/// 再逐个走 [`delete_group_in`]（保留 `pair_bases` 等不变量），最后统一落盘一次。
pub fn prune_dead_groups(
    client: SessionClient,
    scope: Option<WbVariant>,
    dry_run: bool,
) -> Result<Value, String> {
    client.validate_scope(scope)?;
    // 插件 / IDE 的会话行在工作区索引里：判定前先解析数据根，解析不到就没有判定依据，
    // 直接给出可读原因（不写盘、不备份）。
    let index_root = match client {
        SessionClient::Workbuddy => None,
        SessionClient::VscodeExt | SessionClient::CodebuddyIde => Some(
            client
                .data_root()
                .ok_or_else(|| prune_unsupported_reason(client))?,
        ),
    };
    prune_dead_groups_at(
        &client.store_paths(),
        client,
        scope,
        dry_run,
        SessionPaths::for_variant,
        index_root.as_deref(),
    )
}

/// [`prune_dead_groups`] 的可注入版本：显式传入关联存储路径、「档位 → 数据根」解析器与
/// 插件 / IDE 的会话数据根（`None` = 该类客户端没有可用数据根）。
pub(super) fn prune_dead_groups_at(
    paths: &SessionPaths,
    client: SessionClient,
    scope: Option<WbVariant>,
    dry_run: bool,
    resolve_member_paths: impl Fn(WbVariant) -> SessionPaths,
    index_root: Option<&Path>,
) -> Result<Value, String> {
    if dry_run {
        // 只读统计：不取存储锁、不写盘（用户可能只是点开确认框看一眼数量）。
        let store = match session_link::load_store(paths) {
            StoreState::Ready(store) => store,
            StoreState::Missing => return Ok(prune_report(true, Vec::new())),
            StoreState::Unavailable(reason) => {
                return Err(store_unavailable_reason(&reason));
            }
        };
        let dead = dead_groups(&store, client, scope, &resolve_member_paths, index_root)?;
        return Ok(prune_report(true, dead));
    }
    // 首次使用（Missing）没有可清理的对象，也不该被这次调用凭空建出主表；损坏/未知版本
    // （Unavailable）保留现场并拒绝写入。
    //
    // **没有失效组时必须真的不碰主表**：`with_link_store_write` 无论如何都会落盘并
    // `revision + 1`，把空转的清理也写进去等于凭空制造一次「存储被改动」（用户看不到任何
    // 变化，却会得到新的 revision 与新的文件 mtime）。所以先在锁外做一次只读预判，为空就
    // 直接返回。预判只用于**提前退出**：锁内仍会重新读表、重新判定，期间若有组变成失效，
    // 下一次调用会处理掉（最坏是多报一次「没有可清理的组」，绝不会多删）。
    match session_link::load_store(paths) {
        StoreState::Missing => return Ok(prune_report(false, Vec::new())),
        StoreState::Unavailable(reason) => {
            return Err(store_unavailable_reason(&reason));
        }
        StoreState::Ready(store) => {
            let dead = dead_groups(&store, client, scope, &resolve_member_paths, index_root)?;
            if dead.is_empty() {
                return Ok(prune_report(false, dead));
            }
        }
    }
    // 单次存储锁内完成「重读 → 判定 → 备份 → 删组 → 校验 → 落盘」：失败不写盘、不推进 revision。
    session_link::with_link_store_write(paths, |store| {
        let dead = dead_groups(store, client, scope, &resolve_member_paths, index_root)?;
        if dead.is_empty() {
            return Ok(prune_report(false, dead));
        }
        // 备份必须在任何内存改动之前完成（此刻磁盘上还是原始内容），失败即中止。
        backup_links_file(paths)?;
        let mut removed_members = 0_usize;
        for group in &dead {
            removed_members =
                removed_members.saturating_add(delete_group_in(store, client, scope, &group.id)?);
        }
        let mut report = prune_report(false, dead);
        report["removedMembers"] = json!(removed_members);
        Ok(report)
    })
}

/// 清理结果载荷（`dryRun` 区分「将要删除」与「已经删除」）。
fn prune_report(dry_run: bool, dead: Vec<DeadGroup>) -> Value {
    let removed_members: usize = dead.iter().map(|group| group.members).sum();
    json!({
        "dryRun": dry_run,
        "removedGroups": dead.len(),
        "removedMembers": removed_members,
        "groupIds": dead.iter().map(|group| group.id.clone()).collect::<Vec<_>>(),
    })
}

/// 判定 scope 内哪些组的所有成员都已失效（按客户端分派存活判定依据）。
///
/// 判定本身与会话管理页共用同一趟扫描（[`scan_groups`]）：这里只把「阻塞原因」翻成 `Err`
/// ——`blocked` 非空即管理页禁用清理入口的同一个原因，两边不可能不一致。
pub(super) fn dead_groups(
    store: &session_link::LinkStore,
    client: SessionClient,
    scope: Option<WbVariant>,
    resolve_member_paths: &dyn Fn(WbVariant) -> SessionPaths,
    index_root: Option<&Path>,
) -> Result<Vec<DeadGroup>, String> {
    // 成员档位决定数据根：跨档组源读源档、目标读目标档。
    let member_paths = |group: &LinkGroup, member: &LinkMember| {
        resolve_member_paths(session_link::member_variant(group, member))
    };
    let scan = scan_groups(store, client, scope, &member_paths, index_root);
    // 破坏性操作不许 fail-open：任一组判不了就整次拒绝（绝不「读不到就当全死」）。
    match scan.blocked {
        Some(reason) => Err(reason),
        None => Ok(scan
            .dead
            .into_iter()
            .map(|group| DeadGroup {
                id: group.id.clone(),
                members: group.members.len(),
            })
            .collect()),
    }
}

/// 写盘前把关联主表复制成 `<主表文件名>.bak-<YYYYMMDD-HHMMSS>`（同目录）。
///
/// 备份不可信时返回 Err：调用方（`with_link_store_write`）据此不写盘，清理整体中止。
fn backup_links_file(paths: &SessionPaths) -> Result<PathBuf, String> {
    let source = paths.session_links_file();
    if !source.is_file() {
        return Err("关联记录主文件不存在，已取消清理".to_string());
    }
    let name = source
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .ok_or_else(|| "关联记录主文件名不可用，已取消清理".to_string())?;
    let stamp = chrono::Utc::now().format("%Y%m%d-%H%M%S");
    let backup = source.with_file_name(format!("{name}.bak-{stamp}"));
    std::fs::copy(&source, &backup)
        .map_err(|error| format!("关联记录备份失败，已取消清理：{error}"))?;
    let (source_len, backup_len) = (
        std::fs::metadata(&source)
            .map(|meta| meta.len())
            .unwrap_or(0),
        std::fs::metadata(&backup)
            .map(|meta| meta.len())
            .unwrap_or(0),
    );
    if source_len == 0 || source_len != backup_len {
        // 备份内容不可信时不能继续删组：原地保留主表，让用户先排查磁盘问题。
        let _ = std::fs::remove_file(&backup);
        return Err("关联记录备份校验失败（大小不一致），已取消清理".to_string());
    }
    Ok(backup)
}
