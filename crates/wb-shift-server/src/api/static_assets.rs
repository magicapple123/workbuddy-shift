//! 内嵌前端静态资源与 SPA 回退。

use axum::body::Body;
use axum::http::{header, HeaderValue, StatusCode, Uri};
use axum::response::Response;
use rust_embed::RustEmbed;

use super::response::json_err;

#[derive(RustEmbed)]
#[folder = "../../dist/"]
struct Assets;

fn content_type(path: &str) -> &'static str {
    if path.ends_with(".js") || path.ends_with(".mjs") {
        "text/javascript"
    } else if path.ends_with(".css") {
        "text/css"
    } else if path.ends_with(".html") {
        "text/html; charset=utf-8"
    } else if path.ends_with(".json") {
        "application/json"
    } else if path.ends_with(".svg") {
        "image/svg+xml"
    } else if path.ends_with(".png") {
        "image/png"
    } else if path.ends_with(".ico") {
        "image/x-icon"
    } else if path.ends_with(".woff2") {
        "font/woff2"
    } else {
        "application/octet-stream"
    }
}

pub(crate) async fn static_handler(uri: Uri) -> Response {
    // /api/* 没有匹配路由时不能回退到 SPA：否则未知接口会得到 200 + index.html，
    // 前端把 HTML 当 JSON 解析并静默失败。
    if uri.path().starts_with("/api/") {
        return json_err("接口不存在".to_string(), StatusCode::NOT_FOUND);
    }
    let mut path = uri.path().trim_start_matches('/').to_string();
    if path.is_empty() || path == "index.html" {
        path = "index.html".to_string();
    }
    // 前端路由回退到 index.html。Content-Type 必须按**实际命中的文件**给：按请求路径算
    // 会把 `/sessions` 这类前端路由标成 application/octet-stream，浏览器直接当下载。
    let (data, served) = match Assets::get(&path) {
        Some(f) => (f, path),
        None => match Assets::get("index.html") {
            Some(f) => (f, "index.html".to_string()),
            None => {
                return plain(
                    StatusCode::NOT_FOUND,
                    "text/plain; charset=utf-8",
                    "not found",
                )
            }
        },
    };
    let mut response = Response::new(Body::from(data.data.into_owned()));
    *response.status_mut() = StatusCode::OK;
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(content_type(&served)),
    );
    // 静态资源都是编译进二进制的已知类型；禁止浏览器按内容嗅探改判，
    // 避免被拖进本机服务的任意文件被当成 HTML/JS 执行。
    response.headers_mut().insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    response
}

/// 固定头部的最小响应；头部值均为编译期常量，`from_static` 不会失败。
fn plain(status: StatusCode, content_type: &'static str, body_text: &'static str) -> Response {
    let mut response = Response::new(Body::from(body_text));
    *response.status_mut() = status;
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    response
}
