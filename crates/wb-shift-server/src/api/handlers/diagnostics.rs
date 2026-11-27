//! diagnostics 域的 HTTP handler。
//!
//! 与 `src-tauri/src/commands/diagnostics.rs` 一一对应：同一份
//! `wb_shift_core::modules::diagnostics::build_report_default`，两个宿主只负责「凑事实」
//! （账号条数 + 各客户端探测结果）。
//!
//! **网页版不落盘**：`GET /api/diagnostics` 只返回文本，前端自己复制。没有对应的
//! `POST /api/diagnostics/export`——服务端不该往用户的机器上写任意路径
//! （与前端 `canPersistErrorLog()` 的既有约定一致）。

use axum::extract::RawQuery;
use axum::http::StatusCode;
use axum::response::Response;
use serde_json::Value;

use crate::api::*;
use wb_shift_core::modules::{
    account, codebuddy_cli, codebuddy_cn_ide, codebuddy_ide, config, diagnostics, jetbrains,
    vscode_ext,
};

/// GET /api/diagnostics —— 脱敏诊断包全文（只读）。
///
/// `locale` 是可选查询参数（`navigator.language`），缺省写 `unknown`；没有别的参数，
/// 报告内容全部由服务端装配。
pub(crate) async fn api_diagnostics_report(RawQuery(query): RawQuery) -> Response {
    let locale = normalize_locale(query_param(query.as_deref(), "locale"));
    // 各客户端 status() 会探盘（目录存在性 / 进程查询），不能在 async 上下文里直接跑。
    let result = tokio::task::spawn_blocking(move || {
        let input = report_input(locale);
        let text = diagnostics::build_report_default(&input);
        serde_json::json!({
            "text": text,
            "generatedAt": config::now_ms(),
            "bytes": text.len(),
        })
    })
    .await;
    match result {
        Ok(value) => json_ok(value),
        Err(error) => json_err(error.to_string(), StatusCode::INTERNAL_SERVER_ERROR),
    }
}

/// 从客户端 `status()` 载荷里取「装没装」与版本（与桌面端同一套优先级）。
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

/// 凑齐报告需要的外部事实；探测列表与桌面端逐项一致。
fn report_input(locale: String) -> diagnostics::ReportInput {
    let clients = vec![
        probe("CodeBuddy CLI", &codebuddy_cli::status()),
        probe("CodeBuddy IDE", &codebuddy_ide::status()),
        probe("CodeBuddy IDE（国内版）", &codebuddy_cn_ide::status()),
        probe("VS Code 插件", &vscode_ext::status()),
        probe("JetBrains 插件", &jetbrains::status()),
    ];
    diagnostics::ReportInput {
        host_form: diagnostics::HostForm::WebUi,
        locale,
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
