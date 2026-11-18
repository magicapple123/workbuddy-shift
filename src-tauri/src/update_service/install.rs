//! 更新包暂存与安装重启。

use super::*;

/// 已下载的更新包：bytes 交给 `Update::install`，`update` 提供安装上下文
/// （安装目标路径、安装器参数等，无法在外部重建）。
pub(crate) struct DownloadedPackage {
    pub(crate) version: String,
    pub(crate) bytes: Vec<u8>,
    pub(crate) update: tauri_plugin_updater::Update,
}

pub(crate) static PACKAGE: OnceLock<Mutex<Option<DownloadedPackage>>> = OnceLock::new();

pub(crate) fn package_cell() -> &'static Mutex<Option<DownloadedPackage>> {
    PACKAGE.get_or_init(|| Mutex::new(None))
}

/// 安装已下载的更新包并重启应用（用户点「重启以完成升级」时调用）。
///
/// 无包时返回可读错误（不 panic）；安装失败置 Error + 通知，并释放已暂存的包。
pub async fn restart<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    if crate::is_screenshot_demo() {
        return Err(DEMO_RESTART_MESSAGE.to_string());
    }
    // 无包时返回可读错误（不 panic）。
    let package = take_package_for_install()?;
    let Some(_busy) = BusyGuard::acquire() else {
        store_package(package);
        return Err(BUSY_MESSAGE.to_string());
    };

    let DownloadedPackage {
        version,
        bytes,
        update,
    } = package;
    eprintln!("[更新] 开始安装 v{version}");
    // 安装会解压整包并替换应用（macOS 未授权时经 AppleScript 在主线程弹授权框），
    // 放 blocking 线程，避免占住 async worker。
    let installed = tauri::async_runtime::spawn_blocking(move || update.install(bytes)).await;
    match installed {
        Ok(Ok(())) => match crate::commands::relaunch_app_inner(app) {
            // macOS / Linux：安装已完成，由本进程拉起新版本。Windows 侧安装器会先让
            // 本进程退出（`install` 内部 `std::process::exit`），到不了这里。
            Ok(()) => Ok(()),
            Err(error) => {
                // 包已装好，重启失败只剩「手动重启」一条路：明确告知，避免用户以为要重下。
                let message = format!("v{version} 已安装，请手动重启应用完成升级：{error}");
                eprintln!("[更新] 重启失败：{message}");
                set_phase(app, fail(message.clone()));
                notify(app, "更新已安装", &message);
                Err(message)
            }
        },
        Ok(Err(error)) => {
            let message = format!("安装 v{version} 失败：{error}");
            eprintln!("[更新] {message}");
            set_phase(app, fail(message.clone()));
            notify(app, "更新安装失败", &message);
            Err(message)
        }
        Err(error) => {
            let message = format!("安装 v{version} 失败：{error}");
            eprintln!("[更新] {message}");
            set_phase(app, fail(message.clone()));
            notify(app, "更新安装失败", &message);
            Err(message)
        }
    }
}

pub(crate) fn store_package(package: DownloadedPackage) {
    *package_cell()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(package);
}

/// 取出待安装的包；无包时返回可读错误（不 panic）。
pub(crate) fn take_package_for_install() -> Result<DownloadedPackage, String> {
    take_package().ok_or_else(|| NO_PACKAGE_MESSAGE.to_string())
}

pub(crate) fn take_package() -> Option<DownloadedPackage> {
    package_cell()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take()
}

/// 公开更新页地址（按当前更新源配置拼装，与 core 的 `update_check` 同源）。
pub(crate) fn release_url() -> String {
    let config = update::load_github_config();
    let owner = config
        .get("owner")
        .and_then(Value::as_str)
        .filter(|owner| !owner.is_empty())
        .unwrap_or(update::GITHUB_OWNER);
    let repo = config
        .get("repo")
        .and_then(Value::as_str)
        .filter(|repo| !repo.is_empty())
        .unwrap_or(update::GITHUB_REPO);
    format!("https://github.com/{owner}/{repo}/releases/latest")
}
