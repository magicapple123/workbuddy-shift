//! api 层单元测试。

use super::router::router as app_router;
use super::shared_state;
use super::{
    checkin_status_item, host_allowed, origin_allowed, query_param, query_variant, static_handler,
    submitted_config,
};
use axum::body::{to_bytes, Body};
use axum::http::{header, Request, StatusCode, Uri};
use serde_json::json;
use tower::ServiceExt;
use wb_shift_core::modules::variant::WbVariant;

/// SPA 回退必须按**命中的文件**给 Content-Type：`/sessions` 这类前端路由曾按请求路径
/// 被标成 application/octet-stream，浏览器直接当文件下载、页面打不开（真机验收发现）。
#[tokio::test]
async fn spa_fallback_serves_html_content_type() {
    for path in ["/", "/sessions", "/accounts"] {
        let resp = static_handler(Uri::from_static(path)).await;
        assert_eq!(resp.status(), StatusCode::OK, "{path}");
        let content_type = resp
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default();
        assert!(
            content_type.starts_with("text/html"),
            "{path} 的 Content-Type 应为 text/html，实际 {content_type}"
        );
    }
}

/// 缺省档位必须与改造前一致（不传 variant 即国内版）。
#[test]
fn variant_query_defaults_to_cn() {
    assert_eq!(query_variant(None), WbVariant::Cn);
    assert_eq!(query_variant(Some("")), WbVariant::Cn);
    assert_eq!(query_variant(Some("refresh=true")), WbVariant::Cn);
    assert_eq!(
        query_variant(Some("refresh=true&variant=cn")),
        WbVariant::Cn
    );
    assert_eq!(query_variant(Some("variant=ai")), WbVariant::Ai);
    assert_eq!(
        query_variant(Some("variant=ai&refresh=true")),
        WbVariant::Ai
    );
    assert_eq!(query_variant(Some("variant=unknown")), WbVariant::Cn);
}

/// 档位请求体缺省为国内版（与 `VariantRequest` 的默认一致）。
#[test]
fn variant_request_defaults_to_cn() {
    let empty: super::VariantRequest = serde_json::from_value(json!({})).expect("空请求体");
    assert_eq!(WbVariant::parse(empty.variant.as_deref()), WbVariant::Cn);

    let ai: super::VariantRequest =
        serde_json::from_value(json!({"variant": "ai"})).expect("国际版请求体");
    assert_eq!(WbVariant::parse(ai.variant.as_deref()), WbVariant::Ai);
}

#[test]
fn web_checkin_status_keeps_account_identity() {
    let item = checkin_status_item(
        &json!({"id": "account-1", "email": "user@example.com"}),
        json!({"ok": true, "todayCheckedIn": true}),
    );

    assert_eq!(item["accountId"], "account-1");
    assert_eq!(item["email"], "user@example.com");
    assert_eq!(item["todayCheckedIn"], true);
    assert_eq!(item["variant"], "cn");
}

#[test]
fn web_checkin_status_preserves_failure_state() {
    let item = checkin_status_item(
        &json!({"id": "account-2"}),
        json!({"ok": false, "todayCheckedIn": false, "error": "status failed"}),
    );

    assert_eq!(item["accountId"], "account-2");
    assert_eq!(item["ok"], false);
    assert_eq!(item["error"], "status failed");
}

#[test]
fn web_checkin_status_preserves_exclusion_without_inventing_today_status() {
    let item = checkin_status_item(
        &json!({"id": "excluded"}),
        json!({"ok": false, "result": "skipped", "reason": "auto_checkin_disabled"}),
    );
    assert_eq!(item["accountId"], "excluded");
    assert_eq!(item["reason"], "auto_checkin_disabled");
    assert_eq!(item["result"], "skipped");
    assert!(item.get("todayCheckedIn").is_none());
}

#[test]
fn web_checkin_status_row_carries_variant() {
    let item = checkin_status_item(
        &json!({"id": "ai-1", "variant": "ai"}),
        json!({"ok": false, "statusUnsupported": true}),
    );

    assert_eq!(item["variant"], "ai");
    assert_eq!(item["statusUnsupported"], true);
}

/// Host 校验：只放行本机回环地址（含端口），其它域名一律拒绝。
///
/// 服务只监听 127.0.0.1，但浏览器里被 DNS 重绑定到 127.0.0.1 的恶意页面仍会
/// 发出同源请求；Host 头校验把它们挡在路由之前，正常 webui 不受影响。
#[test]
fn host_guard_accepts_only_loopback_hosts() {
    for allowed in [
        "127.0.0.1:57910",
        "127.0.0.1",
        "localhost:57910",
        "LOCALHOST:57910",
        "[::1]:57910",
        "[::1]",
    ] {
        assert!(host_allowed(Some(allowed)), "{allowed} 应被放行");
    }
    for rejected in [
        "",
        "evil.example:57910",
        "127.0.0.1.evil.example",
        "localhost.evil.example:57910",
        "127.0.0.1:abc",
        "[::1]evil",
        "::1",
    ] {
        assert!(!host_allowed(Some(rejected)), "{rejected} 应被拒绝");
    }
    assert!(!host_allowed(None), "缺失 Host 头应被拒绝");
}

/// 前端资源来自 rust-embed 的编译期资源表：路径穿越串在表里查不到，回退
/// index.html，不存在按请求路径读取磁盘文件的代码路径。
#[tokio::test]
async fn static_handler_never_serves_files_outside_embedded_assets() {
    for path in [
        "/../Cargo.toml",
        "/%2e%2e%2fCargo.toml",
        "/..%2f..%2fCargo.toml",
        "/assets/../../Cargo.toml",
    ] {
        let response = static_handler(Uri::from_static(path)).await;
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        let content_type = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default();
        assert!(
            content_type.starts_with("text/html"),
            "{path} 应回退 index.html，实际 Content-Type {content_type}"
        );
        let body = to_bytes(response.into_body(), 4 * 1024 * 1024)
            .await
            .expect("读取响应体");
        let text = String::from_utf8_lossy(&body);
        assert!(
            !text.contains("wb-shift-server"),
            "{path} 泄露了 Cargo.toml"
        );
    }
}

/// 静态响应带 X-Content-Type-Options: nosniff。
#[tokio::test]
async fn static_handler_sets_nosniff() {
    let response = static_handler(Uri::from_static("/")).await;
    assert_eq!(
        response
            .headers()
            .get(header::X_CONTENT_TYPE_OPTIONS)
            .and_then(|value| value.to_str().ok()),
        Some("nosniff")
    );
}
/// Origin 校验：无 Origin 放行（curl/本机进程），同源 Origin 放行，跨源拒绝。
#[test]
fn origin_guard_rejects_cross_site_browser_requests() {
    for allowed in [
        (None, Some("127.0.0.1:57910")),
        (Some("http://127.0.0.1:57910"), Some("127.0.0.1:57910")),
        (Some("HTTP://LOCALHOST:57910"), Some("localhost:57910")),
        (Some("http://127.0.0.1:57910/"), Some("127.0.0.1:57910")),
    ] {
        assert!(origin_allowed(allowed.0, allowed.1), "{allowed:?} 应被放行");
    }
    for rejected in [
        (Some("https://evil.example"), Some("127.0.0.1:57910")),
        (Some("http://localhost:57910"), Some("127.0.0.1:57910")),
        (Some("http://127.0.0.1:1"), Some("127.0.0.1:57910")),
        (Some("null"), Some("127.0.0.1:57910")),
        (Some("http://127.0.0.1:57910"), None),
    ] {
        assert!(
            !origin_allowed(rejected.0, rejected.1),
            "{rejected:?} 应被拒绝"
        );
    }
}

/// save_* handler 的 {config} 信封拆包：桌面端与 WebUI 的参数形状不同，但落盘对象必须相同。
#[test]
fn submitted_config_unwraps_webui_envelope() {
    let inside = json!({"enabled": true, "intervalMinutes": 30});
    assert_eq!(
        submitted_config(&json!({"config": inside.clone()})),
        &inside
    );
    assert_eq!(submitted_config(&inside), &inside, "裸配置保持原样");
}

/// query 参数必须百分号解码（代理地址含 ://，不解码会把编码串当代理）。
#[test]
fn query_param_decodes_escaped_values() {
    assert_eq!(
        query_param(
            Some("accountId=a%2Fb&proxy=http%3A%2F%2F127.0.0.1%3A7897"),
            "proxy"
        )
        .as_deref(),
        Some("http://127.0.0.1:7897")
    );
    assert_eq!(
        query_param(Some("q=a+b%20c"), "q").as_deref(),
        Some("a b c")
    );
    assert_eq!(query_param(Some("accountId=x"), "missing"), None);
}

// ---------------------------------------------------------------------------
// router 级测试：用 tower::ServiceExt::oneshot 跑完整栈（路由 + Host/Origin 边界层），
// 不监听端口、不写任何真实用户目录。拒绝类请求用 GET /api/status（中间件在 handler
// 之前拦截，即便断言失败也不会写盘）；放行类请求用只读进程内缓存的 /api/switch/progress。
// ---------------------------------------------------------------------------

/// 构造 router 请求：GET + 空 body；host/origin 为 None 时不带对应头。
///
/// 真实浏览器一定会带 Host；测试里显式传入，避免依赖 http 的默认头行为。
fn router_request(path: &str, host: Option<&str>, origin: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder().method("GET").uri(path);
    if let Some(host) = host {
        builder = builder.header(header::HOST, host);
    }
    if let Some(origin) = origin {
        builder = builder.header(header::ORIGIN, origin);
    }
    builder.body(Body::empty()).expect("构造测试请求")
}

/// 构造 JSON POST 请求：WebUI 的 `httpCall` 固定带 `Content-Type: application/json`。
fn post_json(path: &str, body: serde_json::Value) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(path)
        .header(header::HOST, "127.0.0.1:57910")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .expect("构造测试请求")
}

/// 驱动 api::router() 跑完整栈，返回状态码。
async fn router_status(request: Request<Body>) -> StatusCode {
    app_router(shared_state())
        .oneshot(request)
        .await
        .expect("Router 作为 Service 应始终返回响应")
        .status()
}

/// 未知 /api/* 必须是 404，不能回退 SPA 200：否则前端把 index.html 当 JSON 解析、
/// 静默失败（真机验收问题）。用合法本机 Host 穿过边界层，专测 fallback 分支。
#[tokio::test]
async fn unknown_api_route_returns_404_not_spa_fallback() {
    for path in ["/api/does-not-exist", "/api/unknown/nested"] {
        assert_eq!(
            router_status(router_request(path, Some("127.0.0.1:57910"), None)).await,
            StatusCode::NOT_FOUND,
            "{path} 应返回 404（而不是 SPA 回退 200）"
        );
    }
}

/// Host 边界：非回环 Host / 缺失 Host 在路由前即 403（防 DNS rebinding）。
#[tokio::test]
async fn router_rejects_non_loopback_host() {
    for host in [
        Some("evil.example:57910"),
        Some("127.0.0.1.evil.example"),
        Some("127.0.0.1:abc"),
        None,
    ] {
        assert_eq!(
            router_status(router_request("/api/status", host, None)).await,
            StatusCode::FORBIDDEN,
            "Host {host:?} 应被拒绝"
        );
    }
}

/// Origin 边界：浏览器跨源请求（含 null）一律 403（CSRF）。
#[tokio::test]
async fn router_rejects_cross_site_origin() {
    for origin in ["https://evil.example", "http://localhost:57910", "null"] {
        assert_eq!(
            router_status(router_request(
                "/api/status",
                Some("127.0.0.1:57910"),
                Some(origin)
            ))
            .await,
            StatusCode::FORBIDDEN,
            "Origin {origin} 应被拒绝"
        );
    }
}

/// 边界层放行：无 Origin（curl/本机进程）与同源 Origin 都能到达 handler。
/// /api/switch/progress 只读进程内缓存、不读写磁盘，适合做穿透断言。
#[tokio::test]
async fn router_allows_loopback_requests_without_or_same_origin() {
    for (host, origin) in [
        ("127.0.0.1:57910", None),
        ("localhost:57910", Some("http://localhost:57910")),
        ("[::1]:57910", Some("HTTP://[::1]:57910")),
    ] {
        assert_eq!(
            router_status(router_request("/api/switch/progress", Some(host), origin)).await,
            StatusCode::OK,
            "Host {host} / Origin {origin:?} 应被放行"
        );
    }
}

/// 前端路由走完整栈（带合法 Host）仍回退 index.html：200 + text/html。
#[tokio::test]
async fn router_spa_fallback_serves_html_for_frontend_routes() {
    let response = app_router(shared_state())
        .oneshot(router_request("/sessions", Some("127.0.0.1:57910"), None))
        .await
        .expect("Router 应返回响应");
    assert_eq!(response.status(), StatusCode::OK);
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    assert!(
        content_type.starts_with("text/html"),
        "/sessions 的 Content-Type 应为 text/html，实际 {content_type}"
    );
}

// ---------------------------------------------------------------------------
// 切换任务状态：请求被取消时不能遗留「永久运行中」，也不能提前开放第二次切换。
// 状态语义由 state.rs 的单元测试覆盖；这里验证 HTTP 边界的行为。
// ---------------------------------------------------------------------------

/// 缺少 accountId：400，且不得占用运行标记（否则用户改完参数也切不了）。
#[tokio::test]
async fn switch_rejects_missing_account_id_without_occupying_state() {
    let state = shared_state();
    let response = app_router(state.clone())
        .oneshot(post_json("/api/switch", json!({})))
        .await
        .expect("Router 应返回响应");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = to_bytes(response.into_body(), 64 * 1024)
        .await
        .expect("读取响应体");
    let value: serde_json::Value = serde_json::from_slice(&body).expect("响应是 JSON");
    assert_eq!(value["error"], "缺少 accountId");
    assert!(!state.switch.is_running());
}

/// 字段类型错误必须 400：`restart: "false"` 曾被静默当成 true 并重启客户端。
#[tokio::test]
async fn switch_rejects_wrong_field_types() {
    for payload in [
        json!({"accountId": 1}),
        json!({"accountId": "acc", "restart": "false"}),
        json!({"accountId": "acc", "copySessionIds": "s1"}),
    ] {
        let response = app_router(shared_state())
            .oneshot(post_json("/api/switch", payload.clone()))
            .await
            .expect("Router 应返回响应");
        assert_eq!(
            response.status(),
            StatusCode::BAD_REQUEST,
            "payload {payload} 应被拒绝"
        );
        let body = to_bytes(response.into_body(), 64 * 1024)
            .await
            .expect("读取响应体");
        let value: serde_json::Value = serde_json::from_slice(&body).expect("响应是 JSON");
        let error = value["error"].as_str().unwrap_or_default();
        assert!(
            error.starts_with("请求参数无效"),
            "payload {payload} 的错误文案应说明参数无效，实际 {error}"
        );
    }
}

/// 已有任务在跑：第二个请求 409，且不得清掉原任务的运行标记。
#[tokio::test]
async fn switch_returns_conflict_while_task_is_running() {
    let state = shared_state();
    assert!(state.switch.try_begin(), "测试先占用运行标记");
    let status = app_router(state.clone())
        .oneshot(post_json("/api/switch", json!({"accountId": "acc"})))
        .await
        .expect("Router 应返回响应")
        .status();
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(state.switch.is_running(), "冲突响应不得清掉运行标记");
}

// ---------------------------------------------------------------------------
// 请求体契约：DTO 必须接受前端实际下发的形状，缺省值与改造前一致。
// 前端参数名见 src/lib/api/accounts.ts 的 switchAccount()。
// ---------------------------------------------------------------------------

#[test]
fn switch_dto_accepts_webui_payload() {
    let payload = json!({
        "accountId": "acc-1",
        "restart": false,
        "shareSessions": true,
        "copySessionIds": ["s1", "s2"],
        "syncSelections": [{"groupId": "g", "previewToken": "t", "mode": "safe"}]
    });
    let request: super::SwitchRequest =
        serde_json::from_value(payload).expect("前端请求体应可解析");
    assert_eq!(request.account_id, "acc-1");
    assert!(!request.restart);
    assert!(request.share_sessions);
    assert_eq!(request.copy_session_ids, vec!["s1", "s2"]);
    assert!(request.sync_selections.is_some());
}

#[test]
fn switch_dto_defaults_match_legacy_behavior() {
    let request: super::SwitchRequest =
        serde_json::from_value(json!({"accountId": "acc"})).expect("最小请求体应可解析");
    assert!(request.restart, "未传 restart 时保持改造前的默认 true");
    assert!(!request.share_sessions);
    assert!(request.copy_session_ids.is_empty());
    assert!(request.sync_selections.is_none());
}

/// 备注补丁的三态语义（缺省 = 不改 / null = 清空 / 字符串 = 设置）不能被 DTO 抹平。
#[test]
fn display_patch_keeps_tri_state_semantics() {
    let absent: super::DisplayPatch = serde_json::from_value(json!({})).expect("空补丁");
    assert_eq!(absent.to_value(), json!({}), "缺省字段不得下发给 core");

    let cleared: super::DisplayPatch = serde_json::from_value(json!({"note": null})).expect("清空");
    assert_eq!(cleared.to_value(), json!({"note": null}));

    let set: super::DisplayPatch =
        serde_json::from_value(json!({"note": "备注", "displayField": "phone"})).expect("设置");
    assert_eq!(
        set.to_value(),
        json!({"note": "备注", "displayField": "phone"})
    );
}

// ---------------------------------------------------------------------------
// 清理失效关联组：`dryRun` 必须是显式开关，缺字段只能 400。
//
// 这条 DTO 曾经带 `default`，于是漏传 `dryRun` 的 POST 会被静默降级成 `false` = 真删
// （Tauri command 的 `dry_run: bool` 是必填，两侧语义不一致）。破坏性操作不允许有
// 「缺参数按最危险取值」的默认值，所以这里同时锁 serde 层与 HTTP 层。
// ---------------------------------------------------------------------------

/// 缺 `dryRun` / 类型不是布尔 → 解析失败；显式 true/false → 原样保留。
#[test]
fn prune_dto_requires_an_explicit_dry_run() {
    let missing =
        serde_json::from_value::<super::SessionGroupPruneRequest>(json!({"client": "workbuddy"}))
            .expect_err("缺 dryRun 必须解析失败（不得默认成 false = 真删）");
    assert!(
        missing.to_string().contains("dryRun"),
        "错误文案应指明缺的字段是 dryRun：{missing}"
    );

    // 类型错误同样不能被静默降级（这正是 DTO 取代 `Json<Value>` 的原因）。
    assert!(
        serde_json::from_value::<super::SessionGroupPruneRequest>(
            json!({"client": "workbuddy", "dryRun": "false"})
        )
        .is_err(),
        "字符串 \"false\" 不得被当成布尔"
    );

    let dry: super::SessionGroupPruneRequest =
        serde_json::from_value(json!({"client": "workbuddy", "dryRun": true}))
            .expect("显式 dryRun=true 应可解析");
    assert!(dry.dry_run);
    assert_eq!(dry.variant_scope, None, "variantScope 仍可缺省");

    let real: super::SessionGroupPruneRequest = serde_json::from_value(json!({
        "client": "workbuddy",
        "variantScope": "cn",
        "dryRun": false,
    }))
    .expect("显式 dryRun=false 应可解析");
    assert!(!real.dry_run);
    assert_eq!(real.variant_scope.as_deref(), Some("cn"));
}

/// HTTP 边界：缺 `dryRun` 的请求在解析阶段就被拒，不会进入 core（因此不可能写盘）。
///
/// 这里不做「临时 store 的 sha 不变」断言：server 进程里的 store 目录由
/// `config::store_dir()` 决定（真实用户目录），而给单测临时改 `WB_SHIFT_HOME` 是进程级
/// 全局状态、会与其他并行用例竞争，本仓库也没有这个先例。改为断言错误来自边界解析：
/// core 的错误文案是「会话关联组暂不可用…」「当前客户端暂不支持…」，解析失败只可能是
/// serde 的 `missing field`，即请求从未到达 `prune_dead_groups`。
#[tokio::test]
async fn prune_without_dry_run_is_rejected_at_the_boundary() {
    let response = app_router(shared_state())
        .oneshot(post_json(
            "/api/session-groups/prune-dead",
            json!({"client": "workbuddy"}),
        ))
        .await
        .expect("Router 应返回响应");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = to_bytes(response.into_body(), 64 * 1024)
        .await
        .expect("读取响应体");
    let value: serde_json::Value = serde_json::from_slice(&body).expect("响应是 JSON");
    assert_eq!(value["ok"], json!(false));
    let error = value["error"].as_str().unwrap_or_default();
    assert!(
        error.contains("dryRun"),
        "缺 dryRun 的请求必须在边界被拒（而不是进 core 后按 false 真删）：{error}"
    );
    assert!(
        !error.contains("会话关联组暂不可用") && !error.contains("暂不支持"),
        "错误必须来自请求体解析，不能是 core 的运行时错误：{error}"
    );
}
