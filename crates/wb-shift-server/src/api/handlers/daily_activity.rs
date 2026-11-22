//! daily_activity（国际版每日活跃会话）域的 HTTP handler。
//!
//! 只有「手动跑一轮」这一个能力：没有定时器、没有启动钩子，也没有 `GET` 状态查询
//! （每日去重状态的暴露要等前端真的需要展示时才做，避免多一条弱类型的对外契约）。

use axum::extract::rejection::JsonRejection;
use axum::http::StatusCode;
use axum::response::Response;
use axum::Json;

use crate::api::*;
use serde_json::Value;
use wb_shift_core::modules::*;

/// POST /api/daily-activity/run —— 手动触发一次每日活跃会话（逐账号返回结果）。
///
/// 请求体是**空结构**：账号由后端从账号库读取，调用方不能指定子集（否则能绕过
/// 每日去重重复消耗额度）。与 Tauri `run_daily_activity` 同形，因此 DTO 没有字段。
///
/// 用明确 DTO 而不是 `Json<Value>` 的作用边界：`scripts/check-api-contract.mjs` 只比对
/// **两侧字段集合**（DTO 字段 ↔ Tauri 参数），能拦两侧任意一向的签名漂移（命令加参数而 DTO
/// 没加、或 DTO 加字段而命令没加），但**拦不住**「前端开始多传 `accountIds`」这类只改请求体、
/// 不改命令签名的漂移（serde 默认忽略未知字段）。
///
/// 注意：本 DTO 目前零字段，两侧集合都是空集，这条比对**当前不提供任何保护**；它只在将来
/// 某一侧先加字段时才生效。真要收口得给 DTO 加 `deny_unknown_fields`，但那会把多传字段的
/// 第三方脚本/手搓请求变成 400（桌面端与 WebUI 均不受影响：前者 invoke 无 body，后者固定发
/// `{}`），属对外契约收紧，必须单独评估迁移与通知，因此这里保持宽松、由调用方自觉。
pub(crate) async fn api_run_daily_activity(
    payload: Result<Json<DailyActivityRunRequest>, JsonRejection>,
) -> Response {
    if let Err(rejection) = parse_body(payload) {
        return invalid_body(rejection);
    }
    run_daily_activity_response(account::load_accounts).await
}

/// 账号来源可注入的同一套流程（单测传空集：既不读真实账号库，也不发任何请求）。
///
/// # 为什么账号库读取走 `spawn_blocking`，会话流程不走
///
/// - 账号库读取是**阻塞 fs**（账号多时 `accounts.json` 可能几十 MB），放 blocking 池，
///   避免占住 async 工作线程；
/// - 会话流程是 `async fn`（建会话 → 取沙箱 → ACP → 轮询 → 收尾），等待期间只有网络
///   在途、不占线程，直接 `.await` 即可。把它整体塞进 `spawn_blocking` 反而要在里面
///   调 `block_on`（对当前运行时的反模式，单线程运行时下会直接死锁），故不做。
///
/// 这一轮可能持续数分钟（单账号上限 120s，逐账号串行），只影响发起方自己的请求：
/// 服务端其余接口在等待期间照常响应。
pub(crate) async fn run_daily_activity_response(load: fn() -> Vec<Value>) -> Response {
    let accounts = match tokio::task::spawn_blocking(load).await {
        Ok(accounts) => accounts,
        // join 失败 = blocking 池里 panic 了；宿主层不许 panic 外泄，按相邻 handler
        // （`api_save_text_file`）的做法映射成 500 + 可读文案。
        Err(error) => {
            return json_err(
                format!("读取账号库失败: {error}"),
                StatusCode::INTERNAL_SERVER_ERROR,
            )
        }
    };
    match daily_activity::run_daily_activity_once(&accounts).await {
        Ok(value) => json_ok(value),
        // core 只把「整体性失败」（例如 HTTP 客户端构造不出来）放进 Err；
        // 逐账号失败一律走结果行，不进这个分支。
        Err(error) => json_err(error, StatusCode::INTERNAL_SERVER_ERROR),
    }
}

// ---------------------------------------------------------------------------
// 单测：这一域会**发真实网络请求**，所以一条真实请求都不发。
//
// 用例放在本文件而不是 `api/tests.rs`：那一份已经登记在
// `docs/ARCHITECTURE.md` 的长文件豁免表里（按行数精确校验），再往里加用例会让两道
// 门禁互相打架；handler 自带测试模块也让「会联网的能力」与它的测试待在一起。
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{to_bytes, Body};
    use axum::http::{header, Request, StatusCode};
    use serde_json::json;
    use tower::ServiceExt;

    /// 空账号库（注入缝用：不读真实账号库，因此不可能为真实账号建会话）。
    fn no_accounts() -> Vec<Value> {
        Vec::new()
    }

    /// 空请求体必须可解析（桌面端 invoke 无参数、WebUI 空 body），多传字段被忽略：
    /// DTO 没有字段，语义永远是「账号库里的全部账号」。
    #[test]
    fn dto_accepts_empty_body_and_ignores_extra_fields() {
        let _: DailyActivityRunRequest =
            serde_json::from_value(json!({})).expect("空请求体应可解析（DTO 无字段）");
        let _: DailyActivityRunRequest = serde_json::from_value(json!({"accountIds": ["a"]}))
            .expect("DTO 无字段：多传的字段应被忽略，不能让调用方指定子集绕过每日去重");
    }

    /// 请求体不是对象 → 400 + 既有错误信封，且在**边界**被拒（请求根本不进 core，
    /// 因此不可能发出任何会话请求）。
    #[tokio::test]
    async fn run_rejects_untyped_body_at_the_boundary() {
        for body in [json!(42), json!("account-1")] {
            let request = Request::builder()
                .method("POST")
                .uri("/api/daily-activity/run")
                .header(header::HOST, "127.0.0.1:57910")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .expect("构造测试请求");
            let response = crate::api::router(crate::api::shared_state())
                .oneshot(request)
                .await
                .expect("Router 作为 Service 应始终返回响应");
            assert_eq!(
                response.status(),
                StatusCode::BAD_REQUEST,
                "请求体 {body} 必须被边界拒绝"
            );
            let bytes = to_bytes(response.into_body(), 64 * 1024)
                .await
                .expect("读取响应体");
            let value: Value = serde_json::from_slice(&bytes).expect("响应是 JSON");
            assert_eq!(value["ok"], json!(false));
            let error = value["error"].as_str().unwrap_or_default();
            assert!(
                error.contains("请求参数无效"),
                "解析失败要复用既有的边界错误文案：{error}"
            );
        }
    }

    /// 空账号库：200 + `{date, accounts: []}`；`date` 是本地日期 `YYYY-MM-DD`。
    ///
    /// 这条用例同时锁住「零账号 = 零请求」：core 的循环体不执行，一个外部请求都不会发。
    #[tokio::test]
    async fn run_with_empty_accounts_returns_empty_rows() {
        let response = run_daily_activity_response(no_accounts).await;
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), 64 * 1024)
            .await
            .expect("读取响应体");
        let value: Value = serde_json::from_slice(&bytes).expect("响应是 JSON");
        assert_eq!(value["accounts"], json!([]), "空账号库不该产出结果行");
        let date = value["date"].as_str().unwrap_or_default();
        assert_eq!(date.len(), 10, "date 应是本地日期 YYYY-MM-DD，实际 {date}");
    }

    /// 带 `Content-Type: application/json` 但 body 为空串：axum 的 `Json` 反序列化会拿
    /// `EOF` 报错，必须和其它形状错误一样落在 400（而不是 500），且**不进 core**。
    ///
    /// 前端两条通道都不会发空 body（Tauri invoke 无 body、WebUI 固定 `{}`），所以这条
    /// 只防「脚本/第三方客户端手搓请求」这一种情况：报 400 而不是 500，排查时一眼能看出
    /// 是请求体问题，也不会因为 500 让人以为服务端炸了。
    #[tokio::test]
    async fn run_rejects_empty_body_with_400() {
        let request = Request::builder()
            .method("POST")
            .uri("/api/daily-activity/run")
            .header(header::HOST, "127.0.0.1:57910")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(""))
            .expect("构造测试请求");
        let response = crate::api::router(crate::api::shared_state())
            .oneshot(request)
            .await
            .expect("Router 作为 Service 应始终返回响应");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let bytes = to_bytes(response.into_body(), 64 * 1024)
            .await
            .expect("读取响应体");
        let value: Value = serde_json::from_slice(&bytes).expect("响应是 JSON");
        assert_eq!(value["ok"], json!(false));
        let error = value["error"].as_str().unwrap_or_default();
        assert!(
            error.contains("请求参数无效"),
            "空 body 也要复用边界错误文案：{error}"
        );
    }
}
