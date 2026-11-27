//! 诊断包：桌面端命令（生成报告 + 落盘）。
//!
//! # 为什么这一层只做「凑事实」
//!
//! `wb_shift_core::modules::diagnostics` 刻意不碰账号 / 令牌 / 会话（见该模块文档），
//! 所以「账号有几条」「各客户端装没装」这两组事实必须由**已经持有账号库的宿主层**取好，
//! 以纯值（`usize` / 布尔 / 版本串）传进 core。这也让 core 侧的脱敏单测可以完全不依赖
//! 真实数据目录。
//!
//! 探测列表与设置页「支持工具」的五个入口对齐（`src/lib/supported-tools.ts`）：
//! WorkBuddy 本体的状态由账号页的 `get_status` 覆盖，这里不重复探测（它也拿不到
//! 「装没装」这种布尔，只有路径与运行态）。
//!
//! # 与 HTTP 侧的关系
//!
//! 报告生成两个宿主同源（同一份 `build_report_default`）；**落盘只在这里**：
//! 网页版拿到的只是文本，由前端复制，服务端不会往用户机器上写任意路径
//! （与 `canPersistErrorLog()` 的既有约定一致）。

use serde_json::{json, Value};
use wb_shift_core::modules::{
    account, codebuddy_cli, codebuddy_cn_ide, codebuddy_ide, config, diagnostics, jetbrains,
    vscode_ext,
};

/// 从客户端 `status()` 载荷里取「装没装」与版本。
///
/// 各客户端 `status()` 的形状并不统一（`codebuddy_cli` 用 `configured`，其余用
/// `installed`，VS Code 插件另有 `extensionInstalled`），这里按固定优先级取第一个存在的
/// 布尔值；取不到一律按「未安装」处理——诊断包里宁可保守，也不要凭猜测说「装了」。
/// **只取布尔与版本串**，载荷里的路径 / 账号字段一个都不进入报告。
fn probe(name: &str, status: &Value) -> diagnostics::ClientProbe {
    let installed = ["installed", "extensionInstalled", "configured"]
        .iter()
        .find_map(|key| status.get(*key).and_then(Value::as_bool))
        .unwrap_or(false);
    let version = status
        .get("version")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    diagnostics::ClientProbe::new(name, installed, version)
}

/// 凑齐报告需要的外部事实（账号条数 + 五个客户端的探测结果）。
fn report_input(locale: Option<String>) -> diagnostics::ReportInput {
    let clients = vec![
        probe("CodeBuddy CLI", &codebuddy_cli::status()),
        probe("CodeBuddy IDE", &codebuddy_ide::status()),
        probe("CodeBuddy IDE（国内版）", &codebuddy_cn_ide::status()),
        probe("VS Code 插件", &vscode_ext::status()),
        probe("JetBrains 插件", &jetbrains::status()),
    ];
    diagnostics::ReportInput {
        host_form: diagnostics::HostForm::Desktop,
        locale: normalize_locale(locale),
        account_count: account::load_accounts().len(),
        clients,
    }
}

/// 语言标签归一化：空串 / 纯空白按 `unknown` 处理（报告里不留空值）。
fn normalize_locale(locale: Option<String>) -> String {
    locale
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "unknown".to_string())
}

/// 报告响应形状（与 `crates/wb-shift-server/src/api/handlers/diagnostics.rs` 逐字一致）。
fn report_payload(input: &diagnostics::ReportInput) -> Value {
    let text = diagnostics::build_report_default(input);
    json!({
        "text": text,
        "generatedAt": config::now_ms(),
        "bytes": text.len(),
    })
}

/// 生成脱敏诊断包（只读，不落盘）。前端「预览」与「复制」都调它。
#[tauri::command]
pub fn get_diagnostics_report(locale: Option<String>) -> Value {
    report_payload(&report_input(locale))
}

/// 把诊断包写到用户选定的路径（`.txt` / `.md`）。
///
/// 内容在**宿主侧重新生成**而不是由前端回传：这样「预览里看到的」和「写进文件的」
/// 走同一条装配路径，前端也无法通过这条命令写入任意文本。
#[tauri::command]
pub fn export_diagnostics(path: String, locale: Option<String>) -> Result<Value, String> {
    let input = report_input(locale);
    let text = diagnostics::build_report_default(&input);
    diagnostics::save_report(&path, &text)
}
