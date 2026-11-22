//! HTTP 传输层抽象：协议层只依赖 [`Transport`]，真实实现走 reqwest。
//!
//! 为什么抽成 trait：五步流程是本模块最需要单测覆盖的部分，但它天生要打网络。
//! 抽出这一层后，单测注入脚本化桩，**一个真实请求都不发**（`AGENTS.md` 第 8 节：
//! 测试不得打真实域名）；生产路径只有 [`HttpTransport`]。
//!
//! # 安全约定
//!
//! [`HttpRequest`] / [`HttpResponse`] **故意不实现 `Debug`**：请求头里有
//! `Authorization: Bearer <token>`，响应体来自外部不可信。一旦实现 `Debug`，
//! 任何 `{:?}` / `{:#?}` 都会把 token 顺手带进日志与 panic 信息。

use std::future::Future;
use std::net::{IpAddr, SocketAddr};
use std::pin::Pin;
use std::sync::Mutex;
use std::time::Duration;

use serde_json::Value;

use crate::modules::config::error_chain;
use crate::modules::locks;

use super::dns;
use super::scrub;

/// 传输层返回的 future 类型别名（`trait` 方法里不能直接写 `async fn`，
/// 也不为此引入 `async-trait` 依赖）。
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// 控制台 / 沙箱请求共用的 UA（与官方 web app 一致，不带桌面端指纹）。
pub const WEB_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                                 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36";

/// 建连超时。
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// 控制台请求的客户端级兜底超时（协议层每步还有自己的超时）。
const CONSOLE_TIMEOUT: Duration = Duration::from_secs(30);
/// 沙箱客户端的总超时。SSE 通道用同一个上限：整轮本来就受 `turn_timeout` 约束。
const SANDBOX_TIMEOUT: Duration = Duration::from_secs(120);
/// SSE 空闲上限：这么久收不到任何数据（含注释型 ping）就主动收掉连接。
///
/// 「保活」= 后台持续读流：服务端的 SSE ping 只有被读走，连接才不会因发送缓冲
/// 堆积而被判空闲。空闲超时是为了**有界**——上游再也不发数据就断开，交给第 4 步
/// 的轮询去看会话到底成没成，而不是挂一个永远在等的任务。
const SSE_IDLE_TIMEOUT: Duration = Duration::from_secs(60);

/// 请求方法（只用到这两种）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    /// GET。
    Get,
    /// POST。
    Post,
}

impl Method {
    fn as_reqwest(self) -> reqwest::Method {
        match self {
            Self::Get => reqwest::Method::GET,
            Self::Post => reqwest::Method::POST,
        }
    }
}

/// 一次 HTTP 请求。**不实现 `Debug`**（理由见模块文档）。
#[derive(Clone)]
pub struct HttpRequest {
    /// 方法。
    pub method: Method,
    /// 完整地址（沙箱地址可能带 query，因此禁止原样进文案）。
    pub url: String,
    /// 请求头（名称原样，`Authorization` 在此）。
    pub headers: Vec<(String, String)>,
    /// JSON 请求体。
    pub body: Option<Value>,
}

impl HttpRequest {
    /// 构造 GET。
    pub fn get(url: impl Into<String>) -> Self {
        Self {
            method: Method::Get,
            url: url.into(),
            headers: Vec::new(),
            body: None,
        }
    }

    /// 构造带 JSON 体的 POST。
    pub fn post_json(url: impl Into<String>, body: Value) -> Self {
        Self {
            method: Method::Post,
            url: url.into(),
            headers: Vec::new(),
            body: Some(body),
        }
    }

    /// 追加一个请求头。
    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }

    /// 批量追加请求头。
    pub fn headers(mut self, pairs: &[(String, String)]) -> Self {
        self.headers.extend(pairs.iter().cloned());
        self
    }
}

/// 一次 HTTP 响应。**不实现 `Debug`**（响应体不可信）。
#[derive(Clone)]
pub struct HttpResponse {
    /// HTTP 状态码。
    pub status: u16,
    /// 响应头，名称统一小写，供大小写不敏感查找。
    pub headers: Vec<(String, String)>,
    /// 响应体原文。
    pub body: String,
}

impl HttpResponse {
    /// 构造响应（供实现方与单测桩使用）。
    pub fn new(status: u16, headers: Vec<(String, String)>, body: impl Into<String>) -> Self {
        Self {
            status,
            headers,
            body: body.into(),
        }
    }

    /// 按名称查响应头（大小写不敏感）。
    pub fn header(&self, name: &str) -> Option<&str> {
        let name = name.to_ascii_lowercase();
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(&name))
            .map(|(_, value)| value.as_str())
    }

    /// 解析 JSON 响应体；非 JSON 返回 `None`。
    pub fn json(&self) -> Option<Value> {
        serde_json::from_str(&self.body).ok()
    }

    /// 是否 2xx。
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

/// SSE 长连接的存活句柄。
///
/// 协议层必须在**整轮期间**持有它：`Drop` 时取消后台读流任务（连接随之关闭）。
/// 这样「ACP 失败 / 会话失败」的提前返回也不会留下一个还在读流的任务。
pub struct EventStream {
    connection_id: String,
    keepalive: Option<tokio::task::JoinHandle<()>>,
}

impl EventStream {
    /// 构造句柄；`keepalive` 为后台读流任务。
    pub fn new(
        connection_id: impl Into<String>,
        keepalive: Option<tokio::task::JoinHandle<()>>,
    ) -> Self {
        Self {
            connection_id: connection_id.into(),
            keepalive,
        }
    }

    /// `Acp-Connection-Id`，后续每条 JSON-RPC 请求都要带。
    pub fn connection_id(&self) -> &str {
        &self.connection_id
    }
}

impl Drop for EventStream {
    fn drop(&mut self) {
        if let Some(task) = self.keepalive.take() {
            task.abort();
        }
    }
}

/// 协议层依赖的最小 HTTP 能力：一次普通请求 + 一条 SSE 长连接。
///
/// 单测实现见 `tests/fixtures.rs` 的 `StubTransport`。
pub trait Transport: Send + Sync {
    /// 发一个请求并读完响应体。
    fn send<'a>(&'a self, request: HttpRequest) -> BoxFuture<'a, Result<HttpResponse, String>>;

    /// 建立 SSE 通道，返回连接 id 与后台保活任务。
    fn open_stream<'a>(
        &'a self,
        request: HttpRequest,
    ) -> BoxFuture<'a, Result<EventStream, String>>;
}

/// 真实传输层：reqwest + 沙箱域名 DoH 兜底。
///
/// 控制台（`*.workbuddy.ai`）走系统解析；沙箱（`*.sandbox.cloudstudio.club`）在
/// 直连解析失败或命中 Clash fake-ip 段时固定到 DoH 查到的真实 IP，
/// 见 [`dns`]。
pub struct HttpTransport {
    /// 控制台与 DoH 查询用的客户端（系统解析）。
    console: reqwest::Client,
    /// 沙箱 host → 需要固定到的 IP（`None` = 不需要兜底）。可重建缓存，见
    /// [`Self::remember_ip`]。
    resolved: Mutex<Vec<(String, Option<IpAddr>)>>,
}

impl HttpTransport {
    /// 构造传输层。
    pub fn new() -> Result<Self, String> {
        let console = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(CONSOLE_TIMEOUT)
            .user_agent(WEB_USER_AGENT)
            .build()
            .map_err(|e| format!("构造 HTTP 客户端失败: {}", scrub(&error_chain(&e))))?;
        Ok(Self {
            console,
            resolved: Mutex::new(Vec::new()),
        })
    }

    /// 按目标 host 选客户端：沙箱域名需要 DoH 兜底时现场构造一个固定解析的客户端。
    async fn client_for(&self, url: &reqwest::Url) -> Result<reqwest::Client, String> {
        let Some(host) = url.host_str() else {
            return Err("请求地址没有 host".to_string());
        };
        let host = host.to_ascii_lowercase();
        if !dns::is_sandbox_host(&host) {
            return Ok(self.console.clone());
        }
        let pinned = self.sandbox_ip(&host).await?;
        let mut builder = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(SANDBOX_TIMEOUT)
            .user_agent(WEB_USER_AGENT);
        if let Some(ip) = pinned {
            // `resolve` 的覆盖优先于系统解析器，因此 Clash 的 fake-ip 不再生效；
            // 证书校验与 SNI 仍按原 host 名进行。
            builder = builder.resolve(host.as_str(), SocketAddr::new(ip, 443));
        }
        builder
            .build()
            .map_err(|e| format!("构造沙箱客户端失败: {}", scrub(&error_chain(&e))))
    }

    /// 沙箱 host 需要固定到的 IP。先看缓存，再走「系统解析 → 需要时 DoH」。
    async fn sandbox_ip(&self, host: &str) -> Result<Option<IpAddr>, String> {
        if let Some(hit) = self.cached_ip(host) {
            return Ok(hit);
        }
        let system = dns::system_resolve(host).await;
        let ip = dns::resolve_override(self, host, system).await;
        self.remember_ip(host, ip);
        Ok(ip)
    }

    fn cached_ip(&self, host: &str) -> Option<Option<IpAddr>> {
        let cache = locks::recover(&self.resolved);
        cache
            .iter()
            .find(|(cached, _)| cached == host)
            .map(|(_, ip)| *ip)
    }

    /// 记住解析结论。
    ///
    /// 用 `locks::recover`：这是**可重建缓存**（进程内、下一次请求会整体重算），
    /// 中毒时取回半写状态也不影响正确性——最坏是多查一次 DoH。
    fn remember_ip(&self, host: &str, ip: Option<IpAddr>) {
        let mut cache = locks::recover(&self.resolved);
        if !cache.iter().any(|(cached, _)| cached == host) {
            cache.push((host.to_string(), ip));
        }
    }
}

impl Transport for HttpTransport {
    fn send<'a>(&'a self, request: HttpRequest) -> BoxFuture<'a, Result<HttpResponse, String>> {
        Box::pin(async move {
            let url = reqwest::Url::parse(&request.url).map_err(|_| "请求地址非法".to_string())?;
            let client = self.client_for(&url).await?;
            let mut builder = client.request(request.method.as_reqwest(), url);
            for (name, value) in &request.headers {
                builder = builder.header(name.as_str(), value.as_str());
            }
            if let Some(body) = &request.body {
                builder = builder.json(body);
            }
            let response = builder
                .send()
                .await
                .map_err(|e| format!("请求失败: {}", scrub(&error_chain(&e))))?;
            let status = response.status().as_u16();
            let headers = response
                .headers()
                .iter()
                .filter_map(|(name, value)| {
                    value
                        .to_str()
                        .ok()
                        .map(|text| (name.as_str().to_ascii_lowercase(), text.to_string()))
                })
                .collect();
            let body = response.text().await.unwrap_or_default();
            Ok(HttpResponse {
                status,
                headers,
                body,
            })
        })
    }

    fn open_stream<'a>(
        &'a self,
        request: HttpRequest,
    ) -> BoxFuture<'a, Result<EventStream, String>> {
        Box::pin(async move {
            let url = reqwest::Url::parse(&request.url).map_err(|_| "沙箱地址非法".to_string())?;
            let client = self.client_for(&url).await?;
            let mut builder = client.get(url);
            for (name, value) in &request.headers {
                builder = builder.header(name.as_str(), value.as_str());
            }
            let response = builder
                .send()
                .await
                .map_err(|e| format!("SSE 连接失败: {}", scrub(&error_chain(&e))))?;
            let status = response.status().as_u16();
            if status != 200 {
                return Err(format!("SSE 通道返回 HTTP {status}"));
            }
            let connection_id = response
                .headers()
                .get("Acp-Connection-Id")
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_string();
            if connection_id.is_empty() {
                return Err("SSE 通道没有返回 Acp-Connection-Id".to_string());
            }
            let mut response = response;
            // 后台读流：不解释事件，只把数据读走，避免连接被判空闲。
            // 三个退出条件都收敛到「不再有数据」：读完 / 读失败 / 空闲超时——
            // 绝不留下一个永远在等数据的任务。
            let keepalive = tokio::spawn(async move {
                while let Ok(Ok(Some(_))) =
                    tokio::time::timeout(SSE_IDLE_TIMEOUT, response.chunk()).await
                {}
            });
            Ok(EventStream::new(connection_id, Some(keepalive)))
        })
    }
}
