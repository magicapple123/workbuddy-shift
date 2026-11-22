//! 沙箱域名的 DoH 兜底。
//!
//! # 为什么需要
//!
//! Clash / Surge 一类代理的 fake-ip 模式会把 `*.sandbox.cloudstudio.club` 解析成保留段
//! `198.18.0.0/15`（实测 `198.18.x.x`）。这类地址 TCP 能连上，但 TLS 握手会被直接
//! 掐断（`UNEXPECTED_EOF_WHILE_READING`）——用户看到的是「沙箱地址无法访问」，不是
//! 「DNS 解析失败」，排查成本极高。
//!
//! # 策略（与上游一致）
//!
//! **只在直连解析失败或结果全是 fake-ip 时**才查阿里 DoH（`dns.alidns.com` 的 A
//! 记录），拿到真实 IP 后由 [`super::transport::HttpTransport`] 用
//! `reqwest::ClientBuilder::resolve` 把 host 固定过去。解析正常时**一次 DoH 都不发**：
//! 这一层是兜底，不是默认链路。
//!
//! 决策与解析被拆成两个纯函数（[`needs_doh`] / [`pick_a_record`]）加一个注入系统
//! 解析结果的异步入口（[`resolve_override`]），单测因此可以覆盖两个分支而**不做任何
//! 真实解析**。

use std::net::IpAddr;
use std::time::Duration;

use serde_json::Value;

use super::transport::{HttpRequest, Transport};

/// DoH 端点主机（阿里公共 DNS，支持 `application/dns-json`）。
pub const DOH_HOST: &str = "dns.alidns.com";
/// 沙箱域名后缀（E2B on 腾讯云 CloudStudio）。
pub const SANDBOX_SUFFIX: &str = ".sandbox.cloudstudio.club";
/// DoH 查询超时：兜底路径，不能拖慢整轮。
const DOH_TIMEOUT: Duration = Duration::from_secs(8);

/// 是否沙箱域名（只有这类域名才考虑 DoH 兜底）。
pub fn is_sandbox_host(host: &str) -> bool {
    host.trim().to_ascii_lowercase().ends_with(SANDBOX_SUFFIX)
}

/// 是否落在 fake-ip 保留段 `198.18.0.0/15`。
pub fn is_fake_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let octets = v4.octets();
            octets[0] == 198 && (octets[1] == 18 || octets[1] == 19)
        }
        IpAddr::V6(_) => false,
    }
}

/// 系统解析结果是否可信：非空，且没有任何一个地址落在 fake-ip 段。
pub fn address_trusted(addrs: &[IpAddr]) -> bool {
    !addrs.is_empty() && addrs.iter().all(|ip| !is_fake_ip(*ip))
}

/// 纯决策：这个 host 的**系统解析结果**需不需要 DoH 兜底。
///
/// - 非沙箱域名：不需要（其它链路解析正常，不去碰 DoH）；
/// - 解析失败：需要；
/// - 结果为空或含 fake-ip：需要。
pub fn needs_doh(host: &str, system: &Result<Vec<IpAddr>, String>) -> bool {
    if !is_sandbox_host(host) {
        return false;
    }
    match system {
        Ok(addrs) => !address_trusted(addrs),
        Err(_) => true,
    }
}

/// DoH 查询地址（`name` 是沙箱域名，本身不是秘密；query 仍会被错误文案脱敏掉）。
pub fn doh_url(host: &str) -> String {
    format!("https://{DOH_HOST}/resolve?name={host}&type=A")
}

/// 从 DoH 的 JSON 响应里挑第一个可用 A 记录。
///
/// `Answer` 里混着 CNAME（type=5）等条目，只看 `type == 1`；同时**丢掉 fake-ip
/// 结果**——被代理污染的 DoH 应答会把兜底变成循环。
pub fn pick_a_record(body: &Value) -> Option<IpAddr> {
    body.get("Answer")?
        .as_array()?
        .iter()
        .filter(|item| item.get("type").and_then(Value::as_i64) == Some(1))
        .filter_map(|item| item.get("data").and_then(Value::as_str))
        .filter_map(|text| text.trim().parse::<IpAddr>().ok())
        .find(|ip| !is_fake_ip(*ip))
}

/// 查一次 DoH，返回第一个可信 A 记录；任何失败都返回 `None`（兜底失败不改判整轮）。
pub async fn doh_lookup(transport: &dyn Transport, host: &str) -> Option<IpAddr> {
    let request = HttpRequest::get(doh_url(host)).header("Accept", "application/dns-json");
    let response = tokio::time::timeout(DOH_TIMEOUT, transport.send(request))
        .await
        .ok()?
        .ok()?;
    if !response.is_success() {
        return None;
    }
    pick_a_record(&response.json()?)
}

/// 完整兜底：需要时查 DoH，返回要固定到的 IP。
///
/// `system` 是调用方已经拿到的系统解析结果（注入而非内部调用，单测才能覆盖两个
/// 分支而不碰真实 DNS）。
pub async fn resolve_override(
    transport: &dyn Transport,
    host: &str,
    system: Result<Vec<IpAddr>, String>,
) -> Option<IpAddr> {
    if !needs_doh(host, &system) {
        return None;
    }
    doh_lookup(transport, host).await
}

/// 走系统解析拿 A/AAAA 地址。
///
/// `ToSocketAddrs` 是阻塞 API，放进 `spawn_blocking`，避免占住 async 线程。
/// 返回 `Err` 时由 [`needs_doh`] 决定是否兜底。
pub async fn system_resolve(host: &str) -> Result<Vec<IpAddr>, String> {
    use std::net::ToSocketAddrs;

    let host = host.to_string();
    let joined = tokio::task::spawn_blocking(move || {
        (host.as_str(), 443_u16)
            .to_socket_addrs()
            .map(|addrs| addrs.map(|addr| addr.ip()).collect::<Vec<IpAddr>>())
            .map_err(|e| crate::modules::config::error_chain(&e))
    })
    .await;
    match joined {
        Ok(inner) => inner,
        Err(error) => Err(format!("解析任务异常: {error}")),
    }
}
