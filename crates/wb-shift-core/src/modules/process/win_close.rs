//! Windows 优雅关闭助手：向目标进程的可见顶层窗口投递 `WM_SYSCOMMAND / SC_CLOSE`。
//!
//! **为什么不用 `taskkill /PID <pid> /T`**：不带 `/F` 时它只对「有窗口」的进程有效——对
//! 没有窗口的进程（VS Code 的语言服务 / GPU / 扩展宿主）它只回一行「只能强制终止」，
//! 进程原样存活，真正要关的主窗口反而没被命中，调用方只能一路等到超时；带 `/F` 则是
//! 强杀，会丢掉用户未保存的编辑，与本仓库「绝不强杀」的契约冲突。`SC_CLOSE` 等价于点
//! 窗口标题栏的关闭按钮：由目标进程自己决定要不要弹保存提示。
//!
//! **为什么先复制而不与 `jetbrains::win_close` 共用**：JetBrains 的关闭路径在生产运行，
//! 把内部模块提出来再让两条路径共用等于同时改两处、回归面翻倍；本次只做「复制给 VS Code
//! 用」。等两条路径确实需要同一份逻辑（例如都要自动确认弹窗）时再合并到本模块。
//!
//! 平台：只在 Windows 编译（`process::mod` 里按 `#[cfg(target_os = "windows")]` 声明）。
//! 直接 `extern "system"` 声明所需的最小 API 面，不引入新的 crate / feature。

use std::collections::HashSet;

type WndEnumProc = Option<unsafe extern "system" fn(isize, isize) -> i32>;

#[link(name = "user32")]
extern "system" {
    fn EnumWindows(lpEnumFunc: WndEnumProc, lParam: isize) -> i32;
    fn GetWindowThreadProcessId(hwnd: isize, lpdwProcessId: *mut u32) -> u32;
    fn IsWindowVisible(hwnd: isize) -> i32;
    fn PostMessageW(hwnd: isize, msg: u32, wparam: usize, lparam: isize) -> i32;
    fn GetWindowTextLengthW(hwnd: isize) -> i32;
    fn GetWindowTextW(hwnd: isize, lpString: *mut u16, nMaxCount: i32) -> i32;
}

const WM_SYSCOMMAND: u32 = 0x0112;
const SC_CLOSE: usize = 0xF060;

/// 一条「可见顶层窗口」记录：句柄 + 归属进程。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct WindowRow {
    hwnd: isize,
    pid: u32,
}

unsafe extern "system" fn enum_cb(hwnd: isize, lparam: isize) -> i32 {
    // 只看可见窗口：隐藏窗口（托盘 / 后台辅助窗口）发 SC_CLOSE 既关不掉主窗口，
    // 也可能命中不该碰的内部窗口。
    if IsWindowVisible(hwnd) == 0 {
        return 1;
    }
    let mut pid = 0u32;
    GetWindowThreadProcessId(hwnd, &mut pid);
    if pid == 0 {
        return 1;
    }
    let out = &mut *(lparam as *mut Vec<WindowRow>);
    out.push(WindowRow { hwnd, pid });
    1
}

/// 当前全部可见顶层窗口（`EnumWindows` 只枚举顶层窗口，不含控件等子窗口）。
fn visible_windows() -> Vec<WindowRow> {
    let mut rows: Vec<WindowRow> = Vec::new();
    unsafe {
        EnumWindows(Some(enum_cb), &mut rows as *mut _ as isize);
    }
    rows
}

/// 纯逻辑：从 `pids` 里挑出「仍持有可见顶层窗口」的，保持入参顺序并去重。
///
/// 抽成纯函数是为了能在单测里覆盖「给定 pid 与可见窗口集合选出子集」，
/// Win32 调用本身无法在 CI 里断言。
fn pids_with_visible_windows(pids: &[u32], windows: &[WindowRow]) -> Vec<u32> {
    let visible: HashSet<u32> = windows.iter().map(|w| w.pid).collect();
    let mut out: Vec<u32> = Vec::new();
    for pid in pids {
        if visible.contains(pid) && !out.contains(pid) {
            out.push(*pid);
        }
    }
    out
}

/// 对该 pid 的可见顶层窗口逐个投递 `SC_CLOSE`；返回是否命中至少一个窗口。
///
/// 命中多个窗口时逐个投递（VS Code 每个打开的窗口都是独立顶层窗口，「完全退出」意味着
/// 全部关掉）。每个窗口只投一次：重复投递只会叠加多个确认对话框。
pub(crate) fn post_close_to_visible_windows(pid: u32) -> bool {
    let mut hit = false;
    for w in visible_windows().iter().filter(|w| w.pid == pid) {
        unsafe {
            PostMessageW(w.hwnd, WM_SYSCOMMAND, SC_CLOSE, 0);
        }
        hit = true;
    }
    hit
}

/// 这批 pid 中仍持有可见顶层窗口的（保持入参顺序）。
pub(crate) fn visible_window_pids(pids: &[u32]) -> Vec<u32> {
    pids_with_visible_windows(pids, &visible_windows())
}

/// 取这批 pid 的可见顶层窗口标题（去重、去空）。
///
/// 仅用于错误文案：让用户认出「卡住的是哪个窗口」。拿不到标题就返回空集合，
/// 调用方必须能在没有标题时退化到只报 PID。
pub(crate) fn window_titles(pids: &[u32]) -> Vec<String> {
    let mut titles: Vec<String> = Vec::new();
    for w in visible_windows().iter().filter(|w| pids.contains(&w.pid)) {
        let Some(title) = window_title(w.hwnd) else {
            continue;
        };
        if !titles.iter().any(|existing| existing == &title) {
            titles.push(title);
        }
    }
    titles
}

/// 读单个窗口的标题；空标题 / 调用失败一律 `None`。
fn window_title(hwnd: isize) -> Option<String> {
    unsafe {
        let len = GetWindowTextLengthW(hwnd);
        if len <= 0 {
            return None;
        }
        // `+1` 留给结尾的 NUL；GetWindowTextW 返回实际写入的字符数（不含 NUL）。
        let mut buf = vec![0u16; len as usize + 1];
        let written = GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32);
        if written <= 0 {
            return None;
        }
        let title = String::from_utf16_lossy(&buf[..written as usize])
            .trim()
            .to_string();
        (!title.is_empty()).then_some(title)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(hwnd: isize, pid: u32) -> WindowRow {
        WindowRow { hwnd, pid }
    }

    /// 给定 pid 列表与可见窗口集合，选出「仍有窗口」的子集：顺序保持、重复 pid 去重、
    /// 不属于入参的 pid 忽略。
    #[test]
    fn pids_with_visible_windows_keeps_order_and_dedups() {
        let windows = vec![row(0x10, 3003), row(0x11, 2002), row(0x12, 2002)];
        assert_eq!(
            pids_with_visible_windows(&[2001, 2002, 3003], &windows),
            vec![2002, 3003]
        );
        // 入参里有重复 pid 时只保留一次（一个 PID 有多个窗口也只算一次）。
        assert_eq!(
            pids_with_visible_windows(&[2002, 2002, 2001], &windows),
            vec![2002]
        );
        // 一个都没有 → 空集合（调用方据此退回「等待全部 pid」）。
        assert!(pids_with_visible_windows(&[4004, 5005], &windows).is_empty());
        // 没有任何窗口 → 空集合。
        assert!(pids_with_visible_windows(&[2002], &[]).is_empty());
        // 窗口集合里的 pid 不在入参中 → 不返回（不越权关闭别的进程）。
        assert!(pids_with_visible_windows(&[2001], &windows).is_empty());
    }
}
