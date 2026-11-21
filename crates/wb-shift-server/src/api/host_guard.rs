//! 本机 HTTP 边界：Host 防 DNS-rebinding + Origin 防跨站 CSRF。
//!
//! 默认口径与改造前一致：只认回环 Host、只认同源 Origin。Docker 形态需要从容器外访问，
//! 才由 [`ALLOWED_HOSTS_ENV_VAR`] 显式追加域名；**没有任何办法**通过环境变量关闭这两层
//! 校验（故意的：这个服务没有登录鉴权，边界一破就是账号库裸奔）。

use std::sync::OnceLock;

use super::response::json_err;
use axum::extract::Request;
use axum::http::{header, StatusCode};
use axum::middleware::Next;
use axum::response::Response;

/// 请求体上限（字节）：与 axum 0.7 的默认值一致（2 MiB）。
pub(crate) const MAX_REQUEST_BODY_BYTES: usize = 2 * 1024 * 1024;

/// 额外放行的主机名白名单（逗号分隔）。
///
/// 只比主机名（大小写不敏感、忽略端口与空白），与回环白名单是**并集**而不是替换：
/// 放开域名访问后 `127.0.0.1` / `localhost` 仍必须能用，否则反代和本机排障会一起失灵。
/// 不支持通配（`*`）：域名是安全边界的一部分，一个 `*` 就等于把边界拆了。
pub(crate) const ALLOWED_HOSTS_ENV_VAR: &str = "WB_SHIFT_ALLOWED_HOSTS";

/// 进程内缓存的解析结果。环境变量在进程生命周期内不会变，每个请求都读一次既慢又会让
/// 告警重复打印；这里读一次并连同告警一起缓存。
struct AllowedHosts {
    hosts: Vec<String>,
    warning: Option<String>,
}

static ALLOWED_HOSTS: OnceLock<AllowedHosts> = OnceLock::new();

fn allowed_hosts_config() -> &'static AllowedHosts {
    ALLOWED_HOSTS.get_or_init(|| {
        let raw = std::env::var(ALLOWED_HOSTS_ENV_VAR).ok();
        let (hosts, warning) = parse_allowed_hosts(raw.as_deref());
        AllowedHosts { hosts, warning }
    })
}

/// 额外白名单（缓存）。
pub(crate) fn allowed_hosts() -> &'static [String] {
    &allowed_hosts_config().hosts
}

/// 解析告警（缓存）：由 `serve()` 在启动横幅里打印一次，不随每个请求刷屏。
pub(crate) fn allowed_hosts_warning() -> Option<&'static str> {
    allowed_hosts_config().warning.as_deref()
}

/// 解析 `WB_SHIFT_ALLOWED_HOSTS`：逗号分隔，返回（归一化后的主机名，告警）。
///
/// 归一化口径与 [`host_allowed`] 的请求侧完全一致：小写、丢掉合法端口、IPv6 保留方括号。
/// 于是 `nas.local` 与 `NAS.local:57910` 是同一项，用户改了发布端口不会静默失配。
///
/// 非法项与通配项**跳过并汇总成一条告警**，不整体失败：白名单写错一个条目，
/// 最优结果是其余条目照常生效 + 日志说清哪类条目被忽略。
pub(crate) fn parse_allowed_hosts(raw: Option<&str>) -> (Vec<String>, Option<String>) {
    let Some(text) = raw else {
        return (Vec::new(), None);
    };
    let mut hosts: Vec<String> = Vec::new();
    let mut wildcards = 0usize;
    let mut invalid = 0usize;
    for entry in text.split(',') {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        if entry.contains('*') {
            wildcards += 1;
            continue;
        }
        let Some(hostname) = normalize_host(entry) else {
            invalid += 1;
            continue;
        };
        // 去重：重复项不改变语义，但会让日志和排查变吵。
        if hosts.iter().any(|kept| kept == &hostname) {
            continue;
        }
        hosts.push(hostname);
    }
    let mut reasons: Vec<String> = Vec::new();
    if wildcards > 0 {
        reasons.push(format!(
            "忽略 {wildcards} 个含 * 的通配条目（不支持通配，请逐个写显式域名）"
        ));
    }
    if invalid > 0 {
        reasons.push(format!(
            "忽略 {invalid} 个无法解析的条目（只接受主机名，可带端口）"
        ));
    }
    let warning = (!reasons.is_empty()).then(|| {
        format!(
            "[Host] {ALLOWED_HOSTS_ENV_VAR} 有无效内容：{}。",
            reasons.join("；")
        )
    });
    (hosts, warning)
}

/// 把 `Host` 头 / 白名单条目归一成主机名（小写；IPv6 保留方括号）。
///
/// 端口只做「是不是纯数字」的校验后丢弃：比对口径是主机名。返回 `None` 表示这串东西
/// 根本不是合法 Host，调用方按拒绝处理（宁可拒一个奇怪的 Host，也不要放过畸形头）。
fn normalize_host(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    let hostname = if let Some(rest) = raw.strip_prefix('[') {
        // IPv6 字面量：[::1] 或 [::1]:57910。
        let (inner, tail) = rest.split_once(']')?;
        if inner.is_empty() {
            return None;
        }
        if !(tail.is_empty() || tail.strip_prefix(':').is_some_and(is_port_number)) {
            return None;
        }
        format!("[{inner}]")
    } else if let Some((name, port)) = raw.rsplit_once(':') {
        if name.is_empty() || !is_port_number(port) {
            return None;
        }
        name.to_string()
    } else {
        raw.to_string()
    };
    Some(hostname.to_ascii_lowercase())
}

fn is_port_number(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit())
}

/// Host 头是否被放行：要么是回环地址，要么命中额外白名单。
///
/// 允许 `127.0.0.1` / `localhost` / `[::1]`，大小写不敏感；端口存在时必须是纯数字。
/// IPv6 分支是为了 SSH 转发/手工输入的 `[::1]` 文案，不影响监听范围。
///
/// `extra_hosts` 由调用方传入（中间件读缓存、单测直接构造），保证函数仍是纯函数。
pub(crate) fn host_allowed(host: Option<&str>, extra_hosts: &[String]) -> bool {
    const ALLOWED: [&str; 3] = ["127.0.0.1", "localhost", "[::1]"];

    let Some(hostname) = host.and_then(normalize_host) else {
        return false;
    };
    ALLOWED.contains(&hostname.as_str())
        || extra_hosts
            .iter()
            .any(|allowed| allowed.eq_ignore_ascii_case(&hostname))
}

/// 中间件：拒绝 Host 不在白名单内的请求。
///
/// 说明：非浏览器客户端（curl / 本机进程）本来就能访问端口，这层不防它们；
/// 它防的是浏览器里被 DNS 重绑定到 127.0.0.1 的恶意来源——浏览器一定会带
/// 目标域名作为 Host，因此会被这里拒绝，而正常 webui（127.0.0.1/localhost）不受影响。
/// Docker 形态下访问域名必须显式写进 [`ALLOWED_HOSTS_ENV_VAR`]，否则这里返回 403。
pub(crate) async fn local_host_only(request: Request, next: Next) -> Response {
    let host = request
        .headers()
        .get(header::HOST)
        .and_then(|value| value.to_str().ok());
    if host_allowed(host, allowed_hosts()) {
        next.run(request).await
    } else {
        json_err(
            format!(
                "仅接受本机（127.0.0.1 / localhost）或 {ALLOWED_HOSTS_ENV_VAR} 列出的 Host 访问"
            ),
            StatusCode::FORBIDDEN,
        )
    }
}

/// 浏览器请求的 Origin 是否与本次请求的 Host 同源（只认 http）。
///
/// 允许无 Origin（curl / 本机进程；同源 GET 通常也不带），带 Origin 时必须等于
/// `http://<Host>`（大小写不敏感）。这一层挡的是恶意网页对 127.0.0.1:<port> 发起的
/// simple request（CSRF）：Host 校验对直接访问 127.0.0.1 的页面无效，Origin 校验才区分
/// 页面来源。
pub(crate) fn origin_allowed(origin: Option<&str>, host: Option<&str>) -> bool {
    origin_matches(origin, host, false)
}

/// 同 [`origin_allowed`]，但可选接受 `https://<Host>`。
///
/// 为什么需要：TLS 常常在反向代理终止，浏览器页面是 `https://…`，`Origin` 就带
/// `https://`，而本服务只出 http。此时 `Host` 仍是同一个域名，请求依旧是「同源页面发起的」，
/// 拦掉它只会让反代部署里所有写操作 403（页面能打开、点任何按钮都失败，极难排查）。
///
/// 边界没有放宽：仍然要求 **Origin 的主机与 Host 完全一致**，跨站页面（不同主机）照旧拒绝；
/// 而且只有显式配置了 [`ALLOWED_HOSTS_ENV_VAR`]（部署者已承认对外暴露）才会打开这个分支，
/// 默认部署的判定口径逐字不变。
pub(crate) fn origin_matches(origin: Option<&str>, host: Option<&str>, allow_https: bool) -> bool {
    let Some(origin) = origin.map(str::trim).filter(|value| !value.is_empty()) else {
        return true;
    };
    let Some(host) = host.map(str::trim).filter(|value| !value.is_empty()) else {
        return false;
    };
    let host = host.to_ascii_lowercase();
    let origin = origin.trim_end_matches('/');
    if origin.eq_ignore_ascii_case(&format!("http://{host}")) {
        return true;
    }
    allow_https && origin.eq_ignore_ascii_case(&format!("https://{host}"))
}

/// 选定 Origin 判定口径的唯一入口。
///
/// 把「有没有配置额外白名单」这个进程级状态收敛成一个纯函数：中间件与单测走**同一条**
/// 代码路径。否则白名单来自 `OnceLock`（进程内读一次环境变量），任何用例都驱动不到
/// 「已配置白名单」那一支，判断写反了 CI 也不会响。
pub(crate) fn origin_policy(
    origin: Option<&str>,
    host: Option<&str>,
    extra_hosts: &[String],
) -> bool {
    if extra_hosts.is_empty() {
        origin_allowed(origin, host)
    } else {
        origin_matches(origin, host, true)
    }
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
    // 分成两支是刻意的：没配置额外白名单 = 纯本机形态，走改造前的判定（只认 http）；
    // 配置了 = 部署者已经显式放开域名访问，才允许反代终止 TLS 后的 https Origin。
    if origin_policy(origin, host, allowed_hosts()) {
        next.run(request).await
    } else {
        json_err(
            "仅接受同源（本机页面）请求".to_string(),
            StatusCode::FORBIDDEN,
        )
    }
}
