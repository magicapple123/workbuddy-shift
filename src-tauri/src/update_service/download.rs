//! 更新包下载、进度与代理。

use super::*;

/// 下载失败的可分类错误：签名失败要引导用户手动安装，而不是反复点重试。
pub(crate) struct DownloadError {
    pub(crate) message: String,
    /// 是否属于「当前版本无法验证新包签名」（密钥轮换 / 旧客户端）。
    pub(crate) signature: bool,
}

impl From<String> for DownloadError {
    fn from(message: String) -> Self {
        Self {
            message,
            signature: false,
        }
    }
}

/// 把 updater 的错误映射成中文可诊断文案，并标出签名类失败。
///
/// 旧客户端遇到密钥轮换时，签名校验必然失败；继续给「重试」只会让用户死循环，
/// 必须改成「手动下载安装一次」（第三轮第 1 条 P0-4）。
fn describe_updater_error(error: &tauri_plugin_updater::Error) -> DownloadError {
    use tauri_plugin_updater::Error as UpdaterError;
    match error {
        UpdaterError::Minisign(_) | UpdaterError::Base64(_) | UpdaterError::SignatureUtf8(_) => {
            DownloadError {
                message: "更新包签名校验失败：当前版本无法自动验证这个更新包。\
                          请点「打开 Releases 手动下载」，安装一次新版本后即可恢复自动更新。"
                    .to_string(),
                signature: true,
            }
        }
        UpdaterError::TargetNotFound(platform) => DownloadError::from(format!(
            "更新清单不包含当前平台（{platform}），请到 Releases 手动下载对应安装包"
        )),
        UpdaterError::TargetsNotFound(platforms) => DownloadError::from(format!(
            "更新清单不包含当前平台（候选 {platforms:?}），请到 Releases 手动下载对应安装包"
        )),
        UpdaterError::Reqwest(error) => DownloadError::from(format!(
            "无法连接 GitHub 更新服务器：{}。请在设置中填写并保存更新代理后重试",
            wb_shift_core::modules::config::error_chain(error)
        )),
        other => DownloadError::from(format!("更新失败：{other}")),
    }
}

/// 检查成功后预热、供下载复用的 updater 句柄。
///
/// `tauri_plugin_updater::Update` 只能由 `updater.check()` 得到，且不能从外部重建；
/// 检查阶段拿到后缓存，下载时直接 `update.download(..)`，避免第二次 check 因网络抖动
/// 单独失败（第三版第 2.9-11 条：检查通过、下载失败的结构性根因）。
pub(crate) struct PendingUpdate {
    pub(crate) version: String,
    pub(crate) update: tauri_plugin_updater::Update,
}

static PENDING: OnceLock<Mutex<Option<PendingUpdate>>> = OnceLock::new();

fn pending_cell() -> &'static Mutex<Option<PendingUpdate>> {
    PENDING.get_or_init(|| Mutex::new(None))
}

pub(crate) fn take_pending_update() -> Option<PendingUpdate> {
    pending_cell()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take()
}

pub(crate) fn store_pending_update(pending: PendingUpdate) {
    *pending_cell()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(pending);
}

/// 构造带统一超时/代理的 updater。
///
/// - `timeout(20s)`：`updater.check()` 单次请求上限，挂死时不会永久占用任务；
/// - `connect_timeout(10s)`：建连失败快速返回；
/// - `read_timeout(60s)`：下载阶段没有总超时，靠读超时兜底（大包慢速下载仍可完成）。
fn build_updater<R: Runtime>(
    app: &AppHandle<R>,
    proxy: Option<&str>,
) -> Result<tauri_plugin_updater::Updater, String> {
    let builder = app
        .updater_builder()
        .timeout(Duration::from_secs(20))
        .configure_client(|client| {
            client
                .connect_timeout(Duration::from_secs(10))
                .read_timeout(Duration::from_secs(60))
        });
    let builder = match proxy
        .map(str::trim)
        .filter(|proxy| !proxy.is_empty())
        .map(str::to_string)
        .or_else(configured_proxy)
    {
        Some(proxy) => builder.proxy(
            tauri::Url::parse(&proxy).map_err(|error| format!("更新代理地址无效：{error}"))?,
        ),
        None => builder,
    };
    builder
        .build()
        .map_err(|error| format!("初始化更新器失败：{error}"))
}

/// 检查成功后的预热：把 `Update` 句柄缓存给下载用。失败只记日志，不影响检查结果。
pub(crate) async fn prefetch_update_handle<R: Runtime>(
    app: &AppHandle<R>,
    proxy: Option<&str>,
) -> Result<(), String> {
    let updater = build_updater(app, proxy)?;
    let checked = tokio::time::timeout(Duration::from_secs(20), updater.check())
        .await
        .map_err(|_| "预热更新句柄超时（20 秒）".to_string())?
        .map_err(|error| format!("预热更新句柄失败：{error}"))?;
    match checked {
        Some(update) => {
            let version = update.version.clone();
            store_pending_update(PendingUpdate { version, update });
            Ok(())
        }
        None => Ok(()),
    }
}

/// 启动下载：立即返回，进度走 `update-state` 事件与托盘 tooltip。
///
/// 下载在后台任务里进行；重复调用由下载互斥拒绝（检查不占这把锁）。
/// `proxy` 为设置页输入框的当前值（可能尚未保存），为空时回落到已保存配置。
pub fn start_download<R: Runtime>(app: &AppHandle<R>, proxy: Option<String>) -> Result<(), String> {
    if crate::is_screenshot_demo() {
        return Err(DEMO_DOWNLOAD_MESSAGE.to_string());
    }
    let Some(busy) = BusyGuard::acquire(BusyKind::Download) else {
        eprintln!("[更新] 已有下载/安装任务在跑，跳过本次下载");
        return Err(busy_message());
    };
    eprintln!("[更新] 启动下载任务");
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let _busy = busy;
        match download_package(&app, proxy.as_deref()).await {
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
            Err(error) => {
                let message = error.message;
                eprintln!("[更新] 下载失败：{message}");
                error_log::record("backend", "更新下载失败", &message);
                set_phase(
                    &app,
                    if error.signature {
                        fail_signature(message.clone())
                    } else {
                        fail(message.clone())
                    },
                );
                reset_tooltip(&app);
                notify(&app, "更新下载失败", &message);
            }
        }
    });
    Ok(())
}

/// 下载更新包：`Ok(None)` 表示更新源当前没有可用包（版本已追平）。
///
/// 优先复用检查阶段缓存的 `Update` 句柄；只有句柄不存在（例如应用重启过）时才重新
/// check 一次，并用 20s 超时兜底。
pub(crate) async fn download_package<R: Runtime>(
    app: &AppHandle<R>,
    proxy: Option<&str>,
) -> Result<Option<DownloadedPackage>, DownloadError> {
    let update = match take_pending_update() {
        Some(pending) => {
            eprintln!("[更新] 复用检查阶段缓存的更新句柄 v{}", pending.version);
            pending.update
        }
        None => {
            let updater = build_updater(app, proxy)?;
            tokio::time::timeout(Duration::from_secs(20), updater.check())
                .await
                .map_err(|_| {
                    DownloadError::from(
                        "检查更新包超时（20 秒），请检查网络或更新代理".to_string(),
                    )
                })?
                .map_err(|error| {
                    let described = describe_updater_error(&error);
                    error_log::record("backend", "更新包检查失败", &described.message);
                    described
                })?
                .ok_or_else(|| {
                    DownloadError::from("更新源没有可用的更新包".to_string())
                })?
        }
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
        .map_err(|error| {
            let described = describe_updater_error(&error);
            error_log::record("backend", "更新包下载失败", &described.message);
            described
        })?;
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
