//! 执行切换：switch_account 生命周期编排。

use super::plan::*;
use super::*;
use crate::modules::automation_sync;

/// 切换账号。
///
/// `copy_session_ids` 非空时按路径 B 复制勾选会话（新 id，云端可同步）；
/// `sync_selections` 非空时把来源账号的新增内容同步到目标账号的关联会话（保留目标
/// sessionId、标题与自定义标题）。两者共用同一把档位操作锁，顺序执行、不重复写入。
pub fn switch_account(
    progress_fn: Option<&ProgressFn>,
    account_id: &str,
    restart: bool,
    share_sessions: bool,
    copy_session_ids: &[String],
    sync_selections: &[SyncSelection],
) -> Result<Value, String> {
    let progress = |message: &str| {
        eprintln!("[switch] progress: {message}");
        if let Some(p) = progress_fn {
            p(message);
        }
    };

    progress("开始切换账号…");
    let acc =
        account::find_account(account_id).ok_or_else(|| format!("账号不存在: {account_id}"))?;
    // 档位以账号自身为准：签名里的参数无法表达「用 A 档位操作 B 档位账号」。
    let variant = account::variant_of(&acc);
    let backup = auth_file::backup_auth_file(variant);

    let mut copy_report: Option<Value> = None;
    let mut session_report: Option<Value> = None;
    let mut sync_report: Option<Value> = None;
    let mut recovery_report: Option<Value> = None;
    if restart {
        progress("正在关闭 WorkBuddy…");
        close_workbuddy(variant, 20)?;
        // 关进程后先恢复未完成的会话写入：恢复成功或复制侧可安全延后重试的失败
        // 不阻断切换；同步仍未完成、拿不到锁或中间产物异常则暂停启动（design §4 / §5）。
        let recovery = match session::recover_pending_session_operations(variant) {
            Ok(report) => report,
            Err(error) => {
                return Err(format!(
                    "无法恢复未完成的会话写入（{error}），已暂停切换与启动 WorkBuddy；请稍后重试"
                ));
            }
        };
        let blocking = recovery_blocks_startup(&recovery);
        if !recovery.is_empty() {
            recovery_report = Some(recovery_report_json(&recovery));
        }
        if blocking {
            let detail = recovery_blocking_detail(&recovery);
            return Err(format!(
                "检测到无法安全恢复的会话写入（{detail}），已暂停切换与启动 WorkBuddy；请先处理该会话后再试"
            ));
        }
        // 本次是否请求了会话写入；同时记下写入前已存在的未完成记录，供写入后比对。
        let ops_paths = SessionPaths::for_variant(variant);
        let attempts_session_writes = !copy_session_ids.is_empty() || !sync_selections.is_empty();
        let pending_before = if attempts_session_writes {
            pending_operation_ids(&ops_paths, variant)
        } else {
            BTreeSet::new()
        };
        if !copy_session_ids.is_empty() {
            progress("正在复制会话到目标账号…");
            // 复制失败本身只记进报告、不阻断切换；但若本次留下了未完成的写入，
            // 会在复制与同步都跑完后统一暂停切换与启动（见下方 pending 差集检查）。
            copy_report = Some(
                match session::copy_sessions_for_switch(&acc, copy_session_ids, false) {
                    Ok(report) => report,
                    // 档位锁被占用或无法建立互斥：此时可能另有会话写入正在进行，
                    // 不能当成普通复制失败后继续写认证并启动 App。锁失败文案前缀
                    // 由 session_link 提供，不在这里嗅探整句错误文案。
                    Err(error)
                        if error.starts_with(LOCK_BUSY_MESSAGE_PREFIX)
                            || error.starts_with(LOCK_UNAVAILABLE_MESSAGE_PREFIX) =>
                    {
                        return Err(format!(
                            "无法独占会话操作（{error}），已暂停切换与启动 WorkBuddy；请稍后重试"
                        ));
                    }
                    Err(error) => json!({"error": error}),
                },
            );
        }
        if !sync_selections.is_empty() {
            // 同步与复制顺序执行（同一把档位锁），不重复写入。
            progress("正在同步会话到目标账号…");
            sync_report = Some(
                match session::sync_sessions_for_switch(&acc, sync_selections) {
                    Ok(report) => report,
                    Err(error)
                        if error.starts_with(LOCK_BUSY_MESSAGE_PREFIX)
                            || error.starts_with(LOCK_UNAVAILABLE_MESSAGE_PREFIX) =>
                    {
                        return Err(format!(
                            "无法独占会话操作（{error}），已暂停切换与启动 WorkBuddy；请稍后重试"
                        ));
                    }
                    // 同步失败不阻断切换：契约与成功路径同形，错误挂在 errors 里。
                    Err(error) => json!({
                        "synced": [],
                        "skipped": [],
                        "errors": [{ "error": error }],
                    }),
                },
            );
        }
        if share_sessions {
            // 旧的「全体转移」兼容路径（默认关闭），Rust 版暂未实现
            session_report = Some(json!({"error": "share_sessions 兼容路径暂未在 Rust 版实现"}));
        }
        // 本次复制/同步新留下的未完成操作：不带着不一致的会话内容写认证并启动 App
        // （design §5 / R5）。未完成写入也不在这里被报告成成功——错误已在各自报告里，
        // 这里只决定「暂停」。未开始写入的失败（预览过期、源会话被删等）不产生操作记录，
        // 因此仍然只是跳过该项、继续切号。
        if attempts_session_writes {
            let created = newly_unfinished_writes(
                &pending_before,
                session_link::pending_operations(&ops_paths, variant),
            );
            if !created.is_empty() {
                return Err(format!(
                    "本次会话写入未完成（{}），已暂停切换与启动 WorkBuddy；请重试，下次切号会先完成恢复",
                    unfinished_writes_detail(&created)
                ));
            }
        }
    } else {
        // restart=false 表示本次不做任何会话写入；携带写入意图时显式拒绝该会话操作，
        // 不能静默丢弃（design §4.1）。
        let (copy, sync) = reject_session_writes_without_restart(
            !copy_session_ids.is_empty(),
            !sync_selections.is_empty(),
        );
        copy_report = copy;
        sync_report = sync;
    }
    // 界面设置跨账号同步：必须在关闭客户端之后、写入认证之前完成——此时源账号仍是
    // 「当前登录」，目标账号的 preferences 还没有被客户端重写；restart=false 时只回报跳过。
    progress("正在同步界面设置…");
    let source_uid = auth_file::read_auth_file(variant)
        .and_then(|auth| {
            auth.get("account")
                .and_then(|account| account.get("uid"))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_default();
    let target_uid = account::get_str(&acc, "uid").unwrap_or_default();
    let settings_report = settings_sync::sync_on_switch(
        &settings_sync::SettingsSyncContext::production(),
        variant,
        &source_uid,
        &target_uid,
        restart,
    );

    // 自动化定时任务（R5-11）：只在客户端已关闭（restart）且用户开启同步时执行。
    // 失败不阻断认证切换——与会话复制同口径，结论进报告让用户自己判断。
    let automation_report = if restart
        && automation_sync::is_enabled(&crate::modules::session::SessionPaths::for_variant(variant))
    {
        progress("正在同步自动化任务…");
        Some(automation_sync::sync(&source_uid, &target_uid, variant))
    } else {
        None
    };

    progress("正在写入认证文件…");
    auth_file::write_account_to_auth_file(&acc, variant)?;
    if restart {
        progress("正在启动 WorkBuddy…");
        launch_workbuddy(variant, Some(&progress))?;
    }
    progress("切换完成");

    let mut result = json!({
        "ok": true,
        "account": account::account_display_name(&acc),
        "variant": variant.as_str(),
        "backup": backup.map(|p| p.to_string_lossy().to_string()),
    });
    if let Some(c) = copy_report {
        result["sessionCopy"] = c;
    }
    if let Some(s) = session_report {
        result["sessionShare"] = s;
    }
    if let Some(s) = sync_report {
        result["sessionSync"] = s;
    }
    if let Some(r) = recovery_report {
        result["sessionRecovery"] = r;
    }
    result["settingsSync"] = settings_report;
    if let Some(a) = automation_report {
        result["automationSync"] = a;
    }
    Ok(result)
}
