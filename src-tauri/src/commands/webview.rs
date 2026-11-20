//! 旧 WebKit 兜底的第二道拦截：原生对话框 + 退出。
//!
//! 为什么不在前端弹：`blocked` 档位的机器上 Tailwind 的 `@layer` 整块被丢弃、`color-mix()`
//! 也不支持，DOM 提示的排版与配色都可能极难看。原生对话框不依赖任何 CSS，至少可读。
//!
//! 这是 **desktop-only** 命令：WebUI 里根本不存在「进程」可退出，也不会有老 WebKit 问题，
//! 因此不配 HTTP 路由（见 `scripts/check-command-routes.mjs` 的 desktop-only 清单）。

use tauri_plugin_dialog::{DialogExt, MessageDialogKind};

/// 默认文案（前端没带 detail 时用）。
const DEFAULT_MESSAGE: &str = "当前系统版本过低，界面无法正常显示。\n\n需要 Safari / WebKit 16.4 及以上（macOS 13.3+，或已更新 Safari 的 11.7.5+ / 12.6.4+）。\n请升级系统后重新打开。";

/// 弹原生「系统版本过低」对话框；用户确认后退出进程（退出码 1）。
///
/// `detail` 由 `index.html` 的内联检测脚本算出（含实测到的 WebKit 版本），前端原样带过来；
/// 弹出失败不影响已经渲染的内联提示页，因此这里只把错误字符串化后返回。
#[tauri::command(rename_all = "camelCase")]
pub fn report_unsupported_webview(
    app: tauri::AppHandle,
    detail: Option<String>,
) -> Result<(), String> {
    let message = detail
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| DEFAULT_MESSAGE.to_string());
    app.dialog()
        .message(message)
        .kind(MessageDialogKind::Error)
        .title("系统版本过低")
        .blocking_show();
    // 提示已经看过：让进程退出比留在一个没有样式的窗口里更清楚。
    app.exit(1);
    Ok(())
}
