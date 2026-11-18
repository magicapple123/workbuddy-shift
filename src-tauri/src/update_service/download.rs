//! 更新包下载、进度与代理。

use super::*;

/// 启动下载：立即返回，进度走 `update-state` 事件与托盘 tooltip。
///
/// 下载在后台任务里进行（含重新 check 一次以拿到 `Update` 句柄），因此托盘与前端
/// 都不需要等网络；重复调用由 `BUSY` 拒绝。
pub fn start_download<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    if crate::is_screenshot_demo() {
        return Err(DEMO_DOWNLOAD_MESSAGE.to_string());
    }
    let Some(busy) = BusyGuard::acquire() else {
        eprintln!("[更新] 已有任务在跑，跳过本次下载");
        return Err(BUSY_MESSAGE.to_string());
    };
    eprintln!("[更新] 启动下载任务");
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let _busy = busy;
        match download_package(&app).await {
            // 下载完成：包先暂存，等用户点「重启以完成升级」时才安装。
            Ok(Some(package)) => {
                let version = package.version.clone();
                eprintln!("[更新] 下载完成 v{version}（{} 字节）", package.bytes.len());
                store_package(package);
                set_phase(&app, mark_ready_to_restart(&version));
                reset_tooltip(&app);
                notify(
                    &app,
                    "更新已就绪",
                    &format!("v{version} 已下载完成，点「重启以完成升级」后生效"),
                );
            }
            // 更新源已无可用包（版本已追平）。
            Ok(None) => {
                eprintln!("[更新] 更新源无可用更新包，转为已是最新");
                set_phase(
                    &app,
                    apply_check_result(&json!({"ok": true, "hasUpdate": false})),
                );
                reset_tooltip(&app);
            }
            Err(message) => {
                eprintln!("[更新] 下载失败：{message}");
                set_phase(&app, fail(message.clone()));
                reset_tooltip(&app);
                notify(&app, "更新下载失败", &message);
            }
        }
    });
    Ok(())
}

/// 下载更新包：`Ok(None)` 表示更新源当前没有可用包（版本已追平）。
pub(crate) async fn download_package<R: Runtime>(
    app: &AppHandle<R>,
) -> Result<Option<DownloadedPackage>, String> {
    let builder = app.updater_builder();
    let builder = match configured_proxy() {
        Some(proxy) => builder.proxy(
            tauri::Url::parse(&proxy).map_err(|error| format!("更新代理地址无效：{error}"))?,
        ),
        None => builder,
    };
    let updater = builder
        .build()
        .map_err(|error| format!("初始化更新器失败：{error}"))?;
    let update = updater
        .check()
        .await
        .map_err(|error| format!("检查更新包失败：{error}"))?;
    let Some(update) = update else {
        return Ok(None);
    };

    let version = update.version.clone();
    set_phase(app, begin_download(&version));
    let progress_app = app.clone();
    let mut downloaded: u64 = 0;
    let bytes = update
        .download(
            move |chunk_length, total| {
                downloaded = downloaded.saturating_add(chunk_length as u64);
                on_download_chunk(&progress_app, downloaded, total);
            },
            || {},
        )
        .await
        .map_err(|error| format!("下载更新包失败：{error}"))?;
    Ok(Some(DownloadedPackage {
        version,
        bytes,
        update,
    }))
}

/// 单个下载分包的回调：tooltip 实时更新；快照与事件按整数百分比；菜单跨 10% 才重建。
pub(crate) fn on_download_chunk<R: Runtime>(
    app: &AppHandle<R>,
    downloaded: u64,
    total: Option<u64>,
) {
    set_tooltip(app, &download_tooltip(percent_of(downloaded, total)));
    let Some(percent) = advance_progress(downloaded, total) else {
        return;
    };
    if !PROGRESS_REPORTED.swap(true, Ordering::AcqRel) {
        eprintln!("[更新] 进度开始上报：{percent}%（total={total:?}）");
    }
    let _ = app.emit("update-state", snapshot());
    if should_rebuild_menu(percent) {
        eprintln!("[更新] 下载进度 {percent}%（重建托盘菜单）");
        refresh_tray(app);
    }
}

pub(crate) fn download_tooltip(percent: Option<u8>) -> String {
    match percent {
        Some(percent) => format!("正在下载更新 {percent}%"),
        None => "正在下载更新…".to_string(),
    }
}

/// 更新代理：`update::load_github_config()` 的 `proxy` 注入 updater builder；留空表示不设置。
pub(crate) fn configured_proxy() -> Option<String> {
    update::load_github_config()
        .get("proxy")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|proxy| !proxy.is_empty())
        .map(str::to_string)
}
