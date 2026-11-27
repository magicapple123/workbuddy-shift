//! 诊断报告：把白名单事实拼成一段可读文本，并提供 `.txt` 落盘。
//!
//! 两个入口：
//!
//! - [`build_report`]（纯函数式装配，事实全部来自 [`ReportSources`]）——单测用它喂临时
//!   `error.log` 与手写设置对象，全程不碰用户目录；
//! - [`build_report_default`]（生产入口）——用 [`default_sources`] 读真实路径。
//!
//! 装配完之后**整体再过一遍 [`redact`]**：动态内容（日志正文、权限原因）先在装配阶段
//! 脱敏一次，整段再兜一次，这样即使将来有人在某个分段里塞了新字段，也不会因为忘了
//! 脱敏而漏出去。替换次数取自这最后一遍，写进报告头让用户确认「确实脱敏过」。

use serde_json::Value;
use std::path::{Path, PathBuf};

use crate::modules::config::{atomic_write, now_ms, store_dir};
use crate::modules::error_log::{error_log_path, ERROR_LOG_MAX_LINES};

use super::redact::redact;

/// 允许落盘的扩展名（小写比较）。
///
/// 只认纯文本：诊断包必须是「能直接打开、能直接粘贴」的东西，不接受任意扩展名，
/// 避免这条命令被当成通用写文件通道。
pub const REPORT_EXTENSIONS: [&str; 2] = ["txt", "md"];

/// 诊断包体积上限（4 MiB）：日志最多 200 行、每行 detail 上限 4KB，正常远低于此。
pub const MAX_REPORT_BYTES: usize = 4 * 1024 * 1024;

/// 报告里默认回看的日志条数。
pub const DEFAULT_LOG_LINES: usize = 20;

/// 宿主形态：影响报告里「宿主」那一行，也决定用户该去看哪一套操作路径。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostForm {
    /// 桌面 App（Tauri）。
    Desktop,
    /// 浏览器里的 WebUI。
    WebUi,
    /// 只跑后端的命令行形态。
    Cli,
}

impl HostForm {
    /// 写进报告的稳定取值（snake_case，便于作者 grep）。
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Desktop => "desktop",
            Self::WebUi => "webui",
            Self::Cli => "cli",
        }
    }

    /// 报告里给人看的那一行。
    pub fn label(self) -> &'static str {
        match self {
            Self::Desktop => "桌面版（Tauri）",
            Self::WebUi => "网页版（浏览器访问本机服务）",
            Self::Cli => "命令行 / 仅服务端",
        }
    }
}

/// 单个客户端的探测结果：**只有**「装没装」与版本桶，不含路径与账号。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientProbe {
    /// 客户端显示名（固定文案，来自宿主）。
    pub name: String,
    /// 是否检测到该客户端。
    pub installed: bool,
    /// 客户端版本（取不到时 `None`）。值会再被脱敏一遍。
    pub version: Option<String>,
}

impl ClientProbe {
    /// 新建一条探测结果。
    pub fn new(name: impl Into<String>, installed: bool, version: Option<String>) -> Self {
        Self {
            name: name.into(),
            installed,
            version,
        }
    }
}

/// 限额 hook 的安装状态（只保留布尔与计数，不保留任何路径）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HookState {
    /// 脚本与至少一处配置都就绪。
    pub installed: bool,
    /// 已注册该 hook 的客户端配置处数。
    pub registered_targets: usize,
    /// 探测到的客户端配置总处数。
    pub total_targets: usize,
}

/// 报告需要的外部事实（宿主层填）。**没有任何机密字段**。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReportInput {
    /// 宿主形态。
    pub host_form: HostForm,
    /// 界面语言（`zh-CN` / `en-US` …）。
    pub locale: String,
    /// 账号库里的账号条数（这里只做区间分桶，不落明细）。
    pub account_count: usize,
    /// 各客户端探测结果。
    pub clients: Vec<ClientProbe>,
}

impl ReportInput {
    /// 只带必填项的最小构造（客户端列表为空）。
    pub fn new(host_form: HostForm, locale: impl Into<String>, account_count: usize) -> Self {
        Self {
            host_form,
            locale: locale.into(),
            account_count,
            clients: Vec::new(),
        }
    }
}

/// 报告的数据来源（可注入，单测用临时目录）。
#[derive(Debug, Clone, PartialEq)]
pub struct ReportSources {
    /// `error.log` 的路径。
    pub error_log: PathBuf,
    /// 回看多少条日志。
    pub error_log_limit: usize,
    /// 设置包（`export_settings_bundle()` 的形状）；`None` 表示读不到。
    pub settings: Option<Value>,
    /// 限额 hook 状态；`None` 表示读不到。
    pub hook: Option<HookState>,
    /// 工具数据目录是否**疑似**可写（只读判断，见 `probe_store_writable`）；`None` = 未探测。
    pub store_writable: Option<bool>,
    /// 生成时间（毫秒）。
    pub generated_at_ms: i64,
}

/// 生产来源：真实数据目录 + 真实设置 + 真实 hook 状态。
pub fn default_sources() -> ReportSources {
    ReportSources {
        error_log: error_log_path(),
        error_log_limit: DEFAULT_LOG_LINES,
        settings: Some(crate::modules::config::export_settings_bundle()),
        hook: read_hook_state(),
        store_writable: probe_store_writable(),
        generated_at_ms: now_ms(),
    }
}

/// 限额 hook 状态 → [`HookState`]（只留布尔与计数，路径一律丢在解析这一步）。
fn read_hook_state() -> Option<HookState> {
    let status = crate::modules::rate_limit_hook::hook_status();
    let installed = status.get("installed").and_then(Value::as_bool)?;
    let targets = status.get("targets").and_then(Value::as_array);
    let total_targets = targets.map(Vec::len).unwrap_or(0);
    let registered_targets = targets
        .map(|items| {
            items
                .iter()
                .filter(|item| {
                    item.get("installed")
                        .and_then(Value::as_bool)
                        .unwrap_or(false)
                })
                .count()
        })
        .unwrap_or(0);
    Some(HookState {
        installed,
        registered_targets,
        total_targets,
    })
}

/// 用生产来源生成报告。
pub fn build_report_default(input: &ReportInput) -> String {
    build_report(input, &default_sources())
}

/// 把白名单事实装配成一段可读文本（纯函数，不做 IO 之外的决定）。
pub fn build_report(input: &ReportInput, sources: &ReportSources) -> String {
    let mut body: Vec<String> = Vec::new();

    body.push("[环境]".to_string());
    body.push(format!(
        "  应用版本: {}",
        crate::modules::update::APP_VERSION
    ));
    body.push(format!(
        "  操作系统: {} / {}",
        std::env::consts::OS,
        std::env::consts::ARCH
    ));
    body.push(format!("  宿主形态: {}", input.host_form.label()));
    body.push(format!("  宿主标识: {}", input.host_form.as_wire()));
    body.push(format!("  界面语言: {}", input.locale));
    body.push(format!(
        "  数据目录名: {}",
        store_dir()
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| "-".to_string())
    ));

    body.push(String::new());
    body.push("[账号]".to_string());
    body.push(format!(
        "  账号数量: {} 个（区间 {}）",
        input.account_count,
        account_bucket(input.account_count)
    ));

    body.push(String::new());
    body.push("[客户端]".to_string());
    if input.clients.is_empty() {
        body.push("  （未提供探测结果）".to_string());
    } else {
        for probe in &input.clients {
            let state = if probe.installed {
                "已安装"
            } else {
                "未安装"
            };
            match probe.version.as_deref().filter(|v| !v.trim().is_empty()) {
                Some(version) => {
                    body.push(format!("  {}: {}（版本 {}）", probe.name, state, version))
                }
                None => body.push(format!("  {}: {}", probe.name, state)),
            }
        }
    }

    body.push(String::new());
    body.push("[权限]".to_string());
    body.push(match sources.store_writable {
        // 正向分支同样不能下断言：判据是 metadata 的只读位 + 只读 read_dir，
        // 在 Windows 上 readonly 属性不拦写文件、POSIX 上判不了 ACL 与 root，
        // 所以只能说「没看到只读标记」，不能说「可写」。
        Some(true) => "  工具数据目录: 存在（未检测到只读标记）".to_string(),
        // 只读判断本质上只能给「疑似」：POSIX 上 mode 位判不了 root 与 ACL，
        // Windows 上目录的 readonly 属性基本无意义。所以保留「可能」，不做断言。
        Some(false) => "  工具数据目录: 不可写（可能是权限 / 占用 / 安全软件拦截）".to_string(),
        None => "  工具数据目录: 未探测".to_string(),
    });

    body.push(String::new());
    body.push("[限额 hook]".to_string());
    match sources.hook {
        Some(hook) => {
            body.push(format!(
                "  已安装: {}",
                if hook.installed { "是" } else { "否" }
            ));
            body.push(format!(
                "  已注册配置: {}/{} 处",
                hook.registered_targets, hook.total_targets
            ));
        }
        None => body.push("  状态: 读取失败".to_string()),
    }

    body.push(String::new());
    body.push("[设置项（只有键名与类型，不含取值）]".to_string());
    match &sources.settings {
        Some(settings) => push_settings_keys(&mut body, settings),
        None => body.push("  （读取失败）".to_string()),
    }

    body.push(String::new());
    body.push(format!(
        "[最近错误日志]（最多 {} 条，已脱敏）",
        sources.error_log_limit
    ));
    push_error_log(&mut body, &sources.error_log, sources.error_log_limit);

    let generated = format_timestamp(sources.generated_at_ms);
    let redacted = redact(&body.join("\n"));
    let mut header = String::new();
    header.push_str("WorkBuddy Shift 诊断包\n");
    header.push_str(&format!("生成时间: {generated}\n"));
    header.push_str(&format!(
        "脱敏: 已替换 {} 处敏感片段\n",
        redacted.replacements
    ));
    header.push_str(
        "说明: 本文件不含账号、令牌、会话内容、项目名与原始路径；路径 / UUID / URL 路径 /\n\
         邮箱 / 令牌 / 用户名 / 长数字已替换为占位符。可整段粘贴到 GitHub issue。\n",
    );
    header.push('\n');
    format!("{header}{}\n", redacted.text)
}

/// 账号数量的区间分桶：报告里**不写精确条数**之外的任何账号信息。
pub fn account_bucket(count: usize) -> &'static str {
    match count {
        0 => "0",
        1..=5 => "1-5",
        6..=10 => "6-10",
        11..=20 => "11-20",
        21..=50 => "21-50",
        _ => "50+",
    }
}

/// 把设置包投影成「分组 → 键名: 类型」。
///
/// 只走 `configs` 一层（`export_settings_bundle()` 的白名单就是那四份配置），
/// **取值一个都不读**：这里唯一的产物是字符串 `"boolean"` / `"string"` / …。
fn push_settings_keys(body: &mut Vec<String>, settings: &Value) {
    let Some(configs) = settings.get("configs").and_then(Value::as_object) else {
        body.push("  （设置包形状不符，已跳过）".to_string());
        return;
    };
    if configs.is_empty() {
        body.push("  （没有可导出的配置）".to_string());
        return;
    }
    for (group, value) in configs {
        body.push(format!("  {group}"));
        match value.as_object() {
            Some(fields) => {
                for (key, field) in fields {
                    body.push(format!("    {key}: {}", type_name(field)));
                }
            }
            None => body.push(format!("    （形状异常：{}）", type_name(value))),
        }
    }
}

/// JSON 值的类型名（只用于「值类型」这一列，绝不输出值本身）。
fn type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(number) if number.is_i64() || number.is_u64() => "integer",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// 追加最近若干条错误日志；读不到就写一行说明，不让整份报告失败。
fn push_error_log(body: &mut Vec<String>, path: &Path, limit: usize) {
    let Ok(text) = std::fs::read_to_string(path) else {
        body.push("  （没有日志文件，或读不出来）".to_string());
        return;
    };
    let lines: Vec<&str> = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    if lines.is_empty() {
        body.push("  （日志为空）".to_string());
        return;
    }
    let limit = limit.clamp(1, ERROR_LOG_MAX_LINES);
    let start = lines.len().saturating_sub(limit);
    for (offset, line) in lines[start..].iter().enumerate() {
        let index = start + offset + 1;
        match serde_json::from_str::<Value>(line) {
            Ok(entry) => {
                let kind = entry.get("kind").and_then(Value::as_str).unwrap_or("-");
                let stamp = entry
                    .get("ts")
                    .and_then(Value::as_i64)
                    .map(format_timestamp)
                    .unwrap_or_else(|| "-".to_string());
                let message = entry.get("message").and_then(Value::as_str).unwrap_or("");
                body.push(format!("  #{index} {stamp} kind={kind} {message}"));
                let detail = entry.get("detail").and_then(Value::as_str).unwrap_or("");
                for detail_line in detail.lines().take(6) {
                    if detail_line.trim().is_empty() {
                        continue;
                    }
                    body.push(format!("      {detail_line}"));
                }
            }
            // 非 JSON 行（历史格式 / 被外部追加）：原样收进报告，脱敏仍会兜一遍。
            Err(_) => body.push(format!("  #{index} {line}")),
        }
    }
}

/// 毫秒时间戳 → `YYYY-MM-DD HH:MM:SS UTC`（人类可读，也避免被长数字规则抹掉）。
fn format_timestamp(ms: i64) -> String {
    match chrono::DateTime::from_timestamp_millis(ms) {
        Some(stamp) => stamp.format("%Y-%m-%d %H:%M:%S UTC").to_string(),
        None => format!("{ms}ms"),
    }
}

/// 工具数据目录状态：**纯只读判断，任何情况下都不创建 / 不修改 / 不删除文件**。
///
/// 早先的版本会写一个 `.diagnostics-probe-<pid>` 再删掉来「证明可写」。改掉的理由：
/// 诊断工具本身写盘，进程若在探测中途被杀就会在用户目录里留下垃圾文件；而「当前是否
/// 可写」对排障的价值有限（真写不进去时，`error.log` 与保存失败会自己说话）。
///
/// 现在的判据只有三条只读操作：`metadata`（存在 / 是不是目录 / 只读标记）、
/// `read_dir`（当前用户是否**可访问**该目录——POSIX 上比 mode 位可靠，Windows 上也不
/// 会因为「不能对目录 `File::open`」而误判）。
///
/// 结论因此是**疑似**而非事实：POSIX 的 mode 位判不了 root 与 ACL，Windows 上目录的
/// readonly 属性基本无意义。所以报告里的文案保留「可能」，不写成断言句。
fn probe_store_writable() -> Option<bool> {
    let dir = store_dir();
    let meta = std::fs::metadata(&dir).ok()?;
    if !meta.is_dir() || meta.permissions().readonly() {
        return Some(false);
    }
    // 能以只读方式列目录 = 目录存在且当前用户可访问；不写入任何东西。
    Some(std::fs::read_dir(&dir).is_ok())
}

/// 保存诊断文本到用户选定的路径（桌面宿主专用）。
///
/// 与 `export_file::save_text_file` 同一套约束（空路径 / 扩展名白名单 / 体积上限 /
/// 父目录必须已存在 / 原子写），但扩展名是 `.txt`/`.md` 而不是 `.csv`：诊断包不是
/// 表格，混进 CSV 白名单会让「导出 CSV」这条既有契约变得含糊。
pub fn save_report(path: &str, content: &str) -> Result<Value, String> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err("缺少保存路径".to_string());
    }
    let target = Path::new(trimmed);
    let extension = target
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_lowercase)
        .unwrap_or_default();
    if !REPORT_EXTENSIONS.contains(&extension.as_str()) {
        return Err(format!("只允许保存 .txt / .md 文件（收到 .{extension}）"));
    }
    let bytes = content.len();
    if bytes == 0 {
        return Err("诊断内容为空".to_string());
    }
    if bytes > MAX_REPORT_BYTES {
        return Err(format!(
            "内容超过 {} MiB 上限（{} 字节）",
            MAX_REPORT_BYTES / 1024 / 1024,
            bytes
        ));
    }
    if let Some(parent) = target.parent() {
        if !parent.as_os_str().is_empty() && !parent.is_dir() {
            return Err(format!("目标目录不存在：{}", parent.display()));
        }
    }
    atomic_write(target, content).map_err(|error| format!("写入失败：{error}"))?;
    Ok(serde_json::json!({
        "ok": true,
        "path": target.to_string_lossy(),
        "bytes": bytes,
        "savedAt": now_ms(),
    }))
}
