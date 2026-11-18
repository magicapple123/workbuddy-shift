//! 本机 HTTP 边界：Host 防 DNS-rebinding + Origin 防跨站 CSRF。

use super::response::json_err;
use axum::extract::Request;
use axum::http::{header, StatusCode};
use axum::middleware::Next;
use axum::response::Response;

/// 请求体上限（字节）：与 axum 0.7 的默认值一致（2 MiB）。
pub(crate) const MAX_REQUEST_BODY_BYTES: usize = 2 * 1024 * 1024;

/// Host 头是否来自本机回环地址（可带端口）。
///
/// 允许 `127.0.0.1` / `localhost` / `[::1]`，大小写不敏感；端口存在时必须是纯数字。
/// 服务只绑定 127.0.0.1，IPv6 分支是为了 SSH 转发/手工输入的 `[::1]` 文案
/// （实际连接仍落在 IPv4 回环），不影响监听范围。
pub(crate) fn host_allowed(host: Option<&str>) -> bool {
    const ALLOWED: [&str; 3] = ["127.0.0.1", "localhost", "[::1]"];

    let Some(raw) = host.map(str::trim).filter(|raw| !raw.is_empty()) else {
        return false;
    };
    let hostname = if let Some(rest) = raw.strip_prefix('[') {
        // IPv6 字面量：[::1] 或 [::1]:57910。
        match rest.split_once(']') {
            Some((inner, tail)) if tail.is_empty() || tail.starts_with(':') => {
                format!("[{inner}]")
            }
            _ => return false,
        }
    } else if let Some((name, port)) = raw.rsplit_once(':') {
        let port_is_valid = !port.is_empty() && port.bytes().all(|byte| byte.is_ascii_digit());
        if name.is_empty() || !port_is_valid {
            return false;
        }
        name.to_string()
    } else {
        raw.to_string()
    };

    ALLOWED
        .iter()
        .any(|allowed| hostname.eq_ignore_ascii_case(allowed))
}

/// 中间件：拒绝 Host 不是本机回环地址的请求。
///
/// 说明：非浏览器客户端（curl / 本机进程）本来就能访问回环端口，这层不防它们；
/// 它防的是浏览器里被 DNS 重绑定到 127.0.0.1 的恶意来源——浏览器一定会带
/// 目标域名作为 Host，因此会被这里拒绝，而正常 webui（127.0.0.1/localhost）不受影响。
pub(crate) async fn local_host_only(request: Request, next: Next) -> Response {
    let host = request
        .headers()
        .get(header::HOST)
        .and_then(|value| value.to_str().ok());
    if host_allowed(host) {
        next.run(request).await
    } else {
        json_err(
            "仅接受本机（127.0.0.1 / localhost）访问".to_string(),
            StatusCode::FORBIDDEN,
        )
    }
}

/// 浏览器请求的 Origin 是否与本次请求的 Host 同源。
///
/// 允许无 Origin（curl / 本机进程；同源 GET 通常也不带），带 Origin 时必须等于
/// `http://<Host>`（大小写不敏感）。本地服务只提供 http，https 反代不在支持范围。
/// 这一层挡的是恶意网页对 127.0.0.1:<port> 发起的 simple request（CSRF）：
/// Host 校验对直接访问 127.0.0.1 的页面无效，Origin 校验才区分页面来源。
pub(crate) fn origin_allowed(origin: Option<&str>, host: Option<&str>) -> bool {
    let Some(origin) = origin.map(str::trim).filter(|value| !value.is_empty()) else {
        return true;
    };
    let Some(host) = host.map(str::trim).filter(|value| !value.is_empty()) else {
        return false;
    };
    let expected = format!("http://{}", host.to_ascii_lowercase());
    origin.trim_end_matches('/').eq_ignore_ascii_case(&expected)
}

/// 中间件：拒绝跨源浏览器请求。
pub(crate) async fn local_origin_only(request: Request, next: Next) -> Response {
    let origin = request
        .headers()
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok());
    let host = request
        .headers()
        .get(header::HOST)
        .and_then(|value| value.to_str().ok());
    if origin_allowed(origin, host) {
        next.run(request).await
    } else {
        json_err(
            "仅接受同源（本机页面）请求".to_string(),
            StatusCode::FORBIDDEN,
        )
    }
}
