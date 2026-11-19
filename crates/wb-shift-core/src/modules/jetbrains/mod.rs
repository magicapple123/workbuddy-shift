//! JetBrains IDE（IntelliJ IDEA / PyCharm）内 CodeBuddy 插件账号切换。
//!
//! 插件（marketplace id 24379，安装目录 `coding-copilot-jetbrains`）把登录态存在
//! **应用级配置目录** `options/secret-storage.xml`：插件自定义的 `SecretStorage`
//! PersistentStateComponent（`@State(storages = @Storage("secret-storage.xml"))`）
//! 以 `<MapStorage><Scores><Entry key=.. value=.. /></Scores></MapStorage>` 保存
//! 全部 secret。内嵌 Node 扩展宿主的 `context.secrets.store/get/delete` 直接透传
//! 到这张表（无 `secret://` 前缀），因此会话密钥与 VS Code 扩展完全同名：
//! `Tencent-Cloud.coding-copilot.new.accessToken`，值为完整会话 JSON——载荷构造
//! 直接复用 [`crate::modules::vscode_ext::build_ext_session_json`]。
//!
//! 支持的产品（按配置目录名前缀）：`IntelliJIdea`（IDEA 旗舰）、`IdeaIC`（社区）、
//! `PyCharm`（专业）、`PyCharmCE`（社区）。扫描所有已安装产品，一次切换写入全部
//! 装有插件的 IDE（多产品共用同一份账号）。
//!
//! 与 VS Code 端同一时序（运行中写入会被 IDE 内存态覆盖）：优雅退出 → 写 XML →
//! 重新打开。IDE 进程与配置目录的对应关系优先用安装目录 `product-info.json` 的
//! `dataDirectoryName` 精确解析，读不到时按 exe 名与安装目录名兜底推断
//! （`mappingSource: "fallback"`）。**仍映射失败却仍在运行的实例会让整次切换被拒绝**
//! （见 [`switch::switch_account`]）——写入后只给警告等于让用户「切了没变」，且会
//! 静默覆盖失败现场。

use serde_json::{json, Value};
use std::path::{Path, PathBuf};
#[cfg(not(target_os = "macos"))]
use std::process::Stdio;
use std::time::Duration;
#[cfg(any(target_os = "macos", target_os = "linux"))]
use std::time::Instant;

use crate::modules::account::{self, get_str};
use crate::modules::codebuddy_cn_ide::{match_account_for_token, parse_token_from_secret};
use crate::modules::config::{atomic_write, now_ms, store_dir};
use crate::modules::process;
use crate::modules::vscode_ext::build_ext_session_json;

const STATE_FILE: &str = "jetbrains.json";
/// CodeBuddy 插件写入 `SecretStorage` 的会话密钥（与 VS Code 扩展同 key）。
const SECRET_KEY: &str = "Tencent-Cloud.coding-copilot.new.accessToken";
/// 插件在 `<配置目录>/plugins/` 下的安装目录名标记（marketplace id 或带前后缀的版本目录）。
///
/// 匹配方式为**包含**（大小写不敏感），不是前缀：2026.3 起插件目录可能被命名为
/// `Tencent-Cloud.coding-copilot` 这类 marketplace id，前缀判定会误判「未安装」。
const PLUGIN_DIR_MARKER: &str = "coding-copilot";
/// secret 存储文件相对配置目录的路径。
const SECRET_RELATIVE: &str = "options/secret-storage.xml";

/// 支持的产品：配置目录 / `product-info.json#dataDirectoryName` 前缀。
const PRODUCT_PREFIXES: [&str; 4] = ["IntelliJIdea", "IdeaIC", "PyCharm", "PyCharmCE"];

/// Windows 主进程映像名（`<product>64.exe`；zip / exe 安装通用）。
#[cfg(target_os = "windows")]
const WINDOWS_IMAGE_NAMES: [&str; 4] = ["idea64.exe", "pycharm64.exe", "idea.exe", "pycharm.exe"];

fn state_path() -> PathBuf {
    store_dir().join(STATE_FILE)
}

fn load_state() -> Value {
    std::fs::read_to_string(state_path())
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_else(|| json!({}))
}

fn save_state(state: &Value) -> Result<(), String> {
    let path = state_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let content = serde_json::to_string_pretty(state).map_err(|e| e.to_string())?;
    atomic_write(&path, &content).map_err(|e| e.to_string())
}

fn set_active_account_id(account_id: &str) -> Result<(), String> {
    let mut state = load_state();
    if let Some(map) = state.as_object_mut() {
        map.insert("activeAccountId".to_string(), json!(account_id));
        map.insert("updatedAt".to_string(), json!(now_ms()));
    }
    save_state(&state)
}

fn active_account_id_from_state() -> Option<String> {
    load_state()
        .get("activeAccountId")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// secret-storage.xml 路径。
fn secret_path_for(config_dir: &Path) -> PathBuf {
    config_dir.join(SECRET_RELATIVE)
}

// ---------------------------------------------------------------------------
// secret-storage.xml 读写（JDOM 序列化兼容，无第三方依赖）
// ---------------------------------------------------------------------------

/// XML 属性值转义：覆盖 JDOM 输出的全部敏感字符（含换行类控制字符），
/// 多余转义（如 `'`）对 JDOM 解析无害。
fn xml_escape_attr(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 16);
    for c in value.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            '\n' => out.push_str("&#10;"),
            '\r' => out.push_str("&#13;"),
            '\t' => out.push_str("&#9;"),
            c => out.push(c),
        }
    }
    out
}

/// 反转义 XML 属性值（数字实体 + 五个命名实体）。
fn xml_unescape_attr(value: &str) -> String {
    if !value.contains('&') {
        return value.to_string();
    }
    let mut out = String::with_capacity(value.len());
    let chars: Vec<char> = value.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] != '&' {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        // 从 i 起找实体结束分号（上限 12 字符足够覆盖最长数字实体）。
        let limit = (i + 12).min(chars.len());
        let Some(semi) = chars[i + 1..limit].iter().position(|&c| c == ';') else {
            out.push('&');
            i += 1;
            continue;
        };
        let entity: String = chars[i + 1..i + 1 + semi].iter().collect();
        let decoded = match entity.as_str() {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            hex if hex.starts_with("#x") || hex.starts_with("#X") => {
                u32::from_str_radix(&hex[2..], 16)
                    .ok()
                    .and_then(char::from_u32)
            }
            dec if dec.starts_with('#') => dec[1..].parse::<u32>().ok().and_then(char::from_u32),
            _ => None,
        };
        match decoded {
            Some(c) => {
                out.push(c);
                i += semi + 2;
            }
            // 不是可识别实体：原样输出 `&`，继续正常解析。
            None => {
                out.push('&');
                i += 1;
            }
        }
    }
    out
}

/// 在 Entry 片段（`<Entry key=".." value=".." />`）中提取单个属性值。
fn entry_attr(fragment: &str, attr: &str) -> Option<String> {
    let needle = format!("{attr}=\"");
    let start = fragment.find(&needle)? + needle.len();
    let rest = &fragment[start..];
    let end = rest.find('"')?;
    Some(xml_unescape_attr(&rest[..end]))
}

/// 解析 secret-storage.xml 的全部 Entry（key → value），保持文件顺序。
///
/// 只认 `<Entry ... />` 行内属性；解析器针对 JDOM 固定输出形态，非目标结构
/// （手工编辑过的异形 XML）返回空表并让上层按「文件不存在」处理。
fn parse_secret_entries(content: &str) -> Vec<(String, String)> {
    let mut entries = Vec::new();
    let mut rest = content;
    while let Some(pos) = rest.find("<Entry ") {
        rest = &rest[pos..];
        let end = match rest.find("/>") {
            Some(end) => end + 2,
            None => break,
        };
        let fragment = &rest[..end];
        if let (Some(key), Some(value)) =
            (entry_attr(fragment, "key"), entry_attr(fragment, "value"))
        {
            entries.push((key, value));
        }
        rest = &rest[end..];
    }
    entries
}

/// 序列化 Entry 列表为 JetBrains 平台持久化格式。
///
/// **必须与平台真实写盘格式一致**（实测样本）：`<application><component
/// name="SecretStorage"><Scores>…`。SecretStorage 的 `getState()` 返回的
/// `MapStorage` 根会被平台展平——组件文件里没有这一层；若照反编译代码写成
/// 裸 `<MapStorage>` 顶层，平台读回时整体丢弃（表现为写入的 key 消失、
/// 插件显示未登录）。
fn serialize_secret_entries(entries: &[(String, String)]) -> String {
    let mut out = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<application>\n  <component name=\"SecretStorage\">\n    <Scores>\n",
    );
    for (key, value) in entries {
        out.push_str(&format!(
            "      <Entry key=\"{}\" value=\"{}\" />\n",
            xml_escape_attr(key),
            xml_escape_attr(value)
        ));
    }
    out.push_str("    </Scores>\n  </component>\n</application>\n");
    out
}

/// 读取指定配置目录的会话 secret；无文件 / 无该 key 返回 `None`。
fn read_secret_for(config_dir: &Path) -> Result<Option<String>, String> {
    let path = secret_path_for(config_dir);
    if !path.exists() {
        return Ok(None);
    }
    let content =
        std::fs::read_to_string(&path).map_err(|e| format!("读取 {} 失败: {e}", path.display()))?;
    Ok(parse_secret_entries(&content)
        .into_iter()
        .find(|(key, _)| key == SECRET_KEY)
        .map(|(_, value)| value))
}

/// upsert 会话 secret：目标 key 已存在则原位替换，不存在则追加；其余 Entry 原样保留。
fn write_secret_for(config_dir: &Path, value: &str) -> Result<PathBuf, String> {
    let path = secret_path_for(config_dir);
    let mut entries: Vec<(String, String)> = if path.exists() {
        let content = std::fs::read_to_string(&path)
            .map_err(|e| format!("读取 {} 失败: {e}", path.display()))?;
        // 写前备份（单份覆盖式）：secret 丢了大不了重新登录，但零成本留个还原点。
        let backup = path.with_extension("xml.workbuddy-shift-bak");
        std::fs::copy(&path, &backup).map_err(|e| format!("备份 {} 失败: {e}", path.display()))?;
        parse_secret_entries(&content)
    } else {
        Vec::new()
    };
    match entries.iter_mut().find(|(key, _)| key == SECRET_KEY) {
        Some(slot) => slot.1 = value.to_string(),
        None => entries.push((SECRET_KEY.to_string(), value.to_string())),
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    atomic_write(&path, &serialize_secret_entries(&entries))
        .map_err(|e| format!("写入 {} 失败: {e}", path.display()))?;
    Ok(path)
}

/// 回滚：把某个配置目录的会话 secret 恢复到写入前的状态。
///
/// `previous = Some(value)` → 写回原值（upsert，保留其它 Entry）；
/// `previous = None` → 写入前该目录没有登录态，删掉我们写进去的 key。
fn restore_secret_for(config_dir: &Path, previous: Option<&str>) -> Result<(), String> {
    match previous {
        Some(value) => write_secret_for(config_dir, value).map(|_| ()),
        None => remove_secret_for(config_dir),
    }
}

/// 删除会话 secret，保留同一文件里的其它 Entry。
///
/// 删完只剩空表时连文件一起删：那个文件是我们刚创建的空壳，留着会让插件
/// 读到「有 secret-storage.xml 但没有登录项」的中间态。
fn remove_secret_for(config_dir: &Path) -> Result<(), String> {
    let path = secret_path_for(config_dir);
    if !path.exists() {
        return Ok(());
    }
    let content =
        std::fs::read_to_string(&path).map_err(|e| format!("读取 {} 失败: {e}", path.display()))?;
    let mut entries = parse_secret_entries(&content);
    let before = entries.len();
    entries.retain(|(key, _)| key != SECRET_KEY);
    if entries.len() == before {
        // 本来就没有我们的 key：回滚无事可做。
        return Ok(());
    }
    if entries.is_empty() {
        return std::fs::remove_file(&path)
            .map_err(|e| format!("回滚失败（删除 {}）: {e}", path.display()));
    }
    atomic_write(&path, &serialize_secret_entries(&entries))
        .map_err(|e| format!("回滚失败（写入 {}）: {e}", path.display()))
}

/// Windows 侧的 JetBrains IDE 优雅关闭（user32 消息流程）。
///
/// 不走 `windows` crate 的 feature，直接 `extern "system"` 声明所需的最小 API 面，
/// 避免为一条消息路径扩大依赖图。
#[cfg(target_os = "windows")]
mod win_close {
    use std::collections::HashSet;
    use std::time::{Duration, Instant};

    #[link(name = "user32")]
    extern "system" {
        fn EnumWindows(lpEnumFunc: WndEnumProc, lParam: isize) -> i32;
        fn GetWindowThreadProcessId(hwnd: isize, lpdwProcessId: *mut u32) -> u32;
        fn IsWindowVisible(hwnd: isize) -> i32;
        fn PostMessageW(hwnd: isize, msg: u32, wparam: usize, lparam: isize) -> i32;
        fn SetForegroundWindow(hwnd: isize) -> i32;
    }
    type WndEnumProc = Option<unsafe extern "system" fn(isize, isize) -> i32>;

    const WM_SYSCOMMAND: u32 = 0x0112;
    const SC_CLOSE: usize = 0xF060;
    const WM_KEYDOWN: u32 = 0x0100;
    const WM_KEYUP: u32 = 0x0101;
    const VK_RETURN: usize = 0x000D;

    struct WindowRow {
        hwnd: isize,
        pid: u32,
    }

    unsafe extern "system" fn enum_cb(hwnd: isize, lparam: isize) -> i32 {
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

    /// 当前全部可见顶层窗口（hwnd + 归属 PID）。
    fn visible_windows() -> Vec<WindowRow> {
        let mut rows: Vec<WindowRow> = Vec::new();
        unsafe {
            EnumWindows(Some(enum_cb), &mut rows as *mut _ as isize);
        }
        rows
    }

    /// 对目标 PID 发出 `SC_CLOSE`；返回是否命中了至少一个窗口。
    fn post_sc_close(windows: &[WindowRow], pid: u32) -> bool {
        let Some(w) = windows.iter().find(|w| w.pid == pid) else {
            return false;
        };
        unsafe {
            PostMessageW(w.hwnd, WM_SYSCOMMAND, SC_CLOSE, 0);
        }
        // 一个实例发一次就够：确认框由 IDE 自己弹，多发可能叠加多个对话框。
        true
    }

    /// 向确认对话框发送 Enter（默认按钮 = 退出）。焦点切换是 best-effort，
    /// 消息直达窗口句柄，不依赖前台状态。
    fn confirm_dialog(hwnd: isize) {
        unsafe {
            SetForegroundWindow(hwnd);
            PostMessageW(hwnd, WM_KEYDOWN, VK_RETURN, 0);
        }
        std::thread::sleep(Duration::from_millis(50));
        unsafe {
            PostMessageW(hwnd, WM_KEYUP, VK_RETURN, 0);
        }
    }

    /// 进程存活探测：`PROCESS_QUERY_LIMITED_INFORMATION` 读退出码，
    /// 避免每次轮询都起 tasklist 子进程。
    fn pid_alive(pid: u32) -> bool {
        unsafe {
            let handle = open_process(0x1000, 0, pid); // PROCESS_QUERY_LIMITED_INFORMATION
            if handle == 0 {
                return false;
            }
            let mut code: u32 = 0;
            let ok = get_exit_code_process(handle, &mut code);
            close_handle(handle);
            ok != 0 && code == 259 // STILL_ACTIVE
        }
    }

    #[link(name = "kernel32")]
    extern "system" {
        #[link_name = "OpenProcess"]
        fn open_process(desired: u32, inherit: i32, pid: u32) -> isize;
        #[link_name = "GetExitCodeProcess"]
        fn get_exit_code_process(handle: isize, code: *mut u32) -> i32;
        #[link_name = "CloseHandle"]
        fn close_handle(handle: isize) -> i32;
    }

    /// 优雅关闭目标 PID（发 SC_CLOSE → 新窗口弹现即发 Enter → 等待退出），
    /// 返回超时后仍存活的 PID。
    pub fn close_gracefully(pids: &[u32], timeout: Duration) -> Vec<u32> {
        let before: HashSet<isize> = visible_windows().into_iter().map(|w| w.hwnd).collect();
        let mut pending: Vec<u32> = pids.to_vec();
        let mut confirmed_dialogs: HashSet<isize> = HashSet::new();

        let mut windows = visible_windows();
        for pid in &pending {
            post_sc_close(&windows, *pid);
        }

        let deadline = Instant::now() + timeout;
        loop {
            pending.retain(|pid| pid_alive(*pid));
            if pending.is_empty() {
                return Vec::new();
            }
            if Instant::now() >= deadline {
                return pending;
            }
            std::thread::sleep(Duration::from_millis(500));

            windows = visible_windows();
            for w in &windows {
                if pending.contains(&w.pid)
                    && !before.contains(&w.hwnd)
                    && !confirmed_dialogs.contains(&w.hwnd)
                {
                    // SC_CLOSE 后新出现的可见窗口 = IDE 自己弹的退出确认（标题随
                    // IDE 语言变化，故不匹配文本）。只确认一次：若默认按钮不是
                    // 退出（如「保存/不保存/取消」场景），重发 Enter 只会误触发。
                    confirmed_dialogs.insert(w.hwnd);
                    confirm_dialog(w.hwnd);
                }
            }
        }
    }
}

mod detect;
mod diagnose;
mod status;
mod switch;

pub use detect::{detect_current_account, jetbrains_config_root};
pub use status::{
    close_running_plugin_ides, jetbrains_restart_precheck, relaunch_closed_ides,
    running_plugin_ide_labels, status, ClosedIdes,
};
pub use switch::switch_account;

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(unused_imports)]
    use super::detect::*;
    use super::diagnose::*;
    #[allow(unused_imports)]
    use super::status::*;
    #[allow(unused_imports)]
    use super::switch::*;
    include!("tests/switch.rs");
}
