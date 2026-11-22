// 单测桩：脚本化的 `Transport` 实现 + 账号 / 临时目录夹具。
//
// **不发任何真实请求**：所有 URL 都在这里被就地应答，`dns.alidns.com` 与沙箱域名
// 都不会真的被解析或连接（见 `AGENTS.md` 第 8 节）。

use std::collections::VecDeque;
use std::sync::Mutex;

/// 桩收到的请求（**只记路径**，不记 query / 请求头，避免把 token 带进断言输出）。
struct Seen {
    path: String,
    rpc: Option<String>,
    connection_id: Option<String>,
    has_auth: bool,
}

/// 控制台路由。
#[derive(Debug, PartialEq, Eq)]
enum ConsoleRoute {
    Create,
    Session,
    Status,
    Stop,
    Delete,
    List,
    Other,
}

struct StubState {
    seen: Vec<Seen>,
    /// 会话状态序列，依次弹出；空则一直返回 `last_status`。
    statuses: VecDeque<String>,
    last_status: String,
    created_models: Vec<String>,
    fail_create_for: Vec<String>,
    /// 强制某阶段失败：`"session"` / `"turn"`。
    fail_phase: Option<&'static str>,
    reject_set_model: bool,
    /// 让 `initialize` 返回 `200 + {"error":…}`：用于固定「200 带错误必须快速失败」。
    reject_initialize: bool,
    /// 活跃名额满：建会话在腾位成功前一律 429 / 10105（`pruned` 变 true 后放行）。
    active_limit: bool,
    /// 已经腾出一个名额（`active_limit` 模式下由被停掉的会话触发）。
    pruned: bool,
    /// 会话列表端点返回的 `(id, status)`，按时间倒序（第一项最新）。
    listed: Vec<(String, String)>,
    /// 会话列表端点被请求过几次（与建会话同路径，只能在这里单独计数）。
    list_requests: usize,
    connection_id: String,
    doh_answers: Vec<serde_json::Value>,
}

impl Default for StubState {
    fn default() -> Self {
        Self {
            seen: Vec::new(),
            statuses: VecDeque::new(),
            last_status: "working".to_string(),
            created_models: Vec::new(),
            fail_create_for: Vec::new(),
            fail_phase: None,
            reject_set_model: false,
            reject_initialize: false,
            active_limit: false,
            pruned: false,
            listed: vec![("conv-old".to_string(), "working".to_string())],
            list_requests: 0,
            connection_id: "conn-1".to_string(),
            doh_answers: vec![serde_json::json!({
                "Status": 0,
                "Answer": [
                    {"name": "abc.sandbox.cloudstudio.club", "type": 5, "data": "alias.example.com"},
                    {"name": "abc.sandbox.cloudstudio.club", "type": 1, "data": "203.0.113.7"}
                ]
            })],
        }
    }
}

/// 脚本化传输层。
struct Stub {
    state: Mutex<StubState>,
}

impl Stub {
    /// 五步全成功的默认脚本。
    fn happy() -> Self {
        let stub = Self {
            state: Mutex::new(StubState::default()),
        };
        stub.with(|state| state.statuses.push_back("completed".to_string()));
        stub
    }

    /// 预设会话状态序列（按顺序弹出）。
    fn with_statuses(self, statuses: &[&str]) -> Self {
        self.with(|state| {
            state.statuses.clear();
            for status in statuses {
                state.statuses.push_back((*status).to_string());
            }
        });
        self
    }

    /// 对指定模型建会话一律 500（模拟限流 / 模型下线）。
    fn fail_create_for(self, model: &str) -> Self {
        self.with(|state| state.fail_create_for.push(model.to_string()));
        self
    }

    /// 强制某阶段失败。
    fn fail_phase(self, phase: &'static str) -> Self {
        self.with(|state| state.fail_phase = Some(phase));
        self
    }

    /// `session/set_model` 返回 JSON-RPC error（其余步骤正常）。
    fn reject_set_model(self) -> Self {
        self.with(|state| state.reject_set_model = true);
        self
    }

    /// 活跃名额满：建会话先 429 / 10105，直到 `prune_oldest` 停掉一条旧会话（只放行一次腾位）。
    fn with_active_limit(self) -> Self {
        self.with(|state| state.active_limit = true);
        self
    }

    /// `initialize` 返回 `200 + {"error":…}`（致命步骤的 JSON-RPC 错误）。
    fn reject_initialize(self) -> Self {
        self.with(|state| state.reject_initialize = true);
        self
    }

    /// 会话列表端点返回的 `(id, status)`（第一项最新）：用于「没有可停的旧会话」等场景。
    fn with_listed(self, listed: &[(&str, &str)]) -> Self {
        self.with(|state| {
            state.listed = listed
                .iter()
                .map(|(id, status)| ((*id).to_string(), (*status).to_string()))
                .collect();
        });
        self
    }

    /// 会话列表端点被请求过几次。
    fn list_requests(&self) -> usize {
        self.with(|state| state.list_requests)
    }

    /// 某个控制台路径被请求过几次（按 `classify` 的路由名：`Status` / `Stop` …）。
    fn console_calls(&self, route: ConsoleRoute) -> usize {
        self.with(|state| {
            state
                .seen
                .iter()
                .filter(|seen| {
                    if seen.rpc.is_some() || seen.path == "/resolve" {
                        return false;
                    }
                    if seen.path.contains("pageSize=") {
                        return route == ConsoleRoute::List;
                    }
                    classify(&seen.path) == route
                })
                .count()
        })
    }

    fn with<R>(&self, mutate: impl FnOnce(&mut StubState) -> R) -> R {
        let mut state = self.state.lock().expect("桩状态锁未中毒");
        mutate(&mut state)
    }

    /// 收到的 JSON-RPC 方法序列。
    fn rpc_methods(&self) -> Vec<String> {
        self.with(|state| {
            state
                .seen
                .iter()
                .filter_map(|seen| seen.rpc.clone())
                .collect()
        })
    }

    /// 是否调过某个收尾动作（`stop` / `delete`）。
    fn called_action(&self, action: &str) -> bool {
        let suffix = format!("/{action}");
        self.with(|state| state.seen.iter().any(|seen| seen.path.ends_with(&suffix)))
    }

    /// 建会话请求里带的模型序列（用于验证顺延顺序）。
    fn created_models(&self) -> Vec<String> {
        self.with(|state| state.created_models.clone())
    }

    /// 查过几次 DoH。
    fn doh_queries(&self) -> usize {
        self.with(|state| {
            state
                .seen
                .iter()
                .filter(|seen| seen.path == "/resolve")
                .count()
        })
    }

    /// 一次请求都没发。
    fn is_empty(&self) -> bool {
        self.with(|state| state.seen.is_empty())
    }

    /// 所有 ACP 请求是否都带了连接 id。
    fn acp_connection_ids(&self) -> Vec<Option<String>> {
        self.with(|state| {
            state
                .seen
                .iter()
                .filter(|seen| seen.rpc.is_some())
                .map(|seen| seen.connection_id.clone())
                .collect()
        })
    }

    /// ACP 请求是否都带了 Authorization。
    fn acp_all_authorized(&self) -> bool {
        self.with(|state| {
            state
                .seen
                .iter()
                .filter(|seen| seen.rpc.is_some())
                .all(|seen| seen.has_auth)
        })
    }

    fn respond(&self, request: &HttpRequest) -> Result<HttpResponse, String> {
        let path = url_path(&request.url);
        let rpc = request
            .body
            .as_ref()
            .and_then(|body| body.get("method"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        let model = request
            .body
            .as_ref()
            .and_then(|body| body.get("model"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        let connection_id = request
            .headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("Acp-Connection-Id"))
            .map(|(_, value)| value.clone());
        let has_auth = request
            .headers
            .iter()
            .any(|(name, value)| name.eq_ignore_ascii_case("Authorization") && !value.is_empty());

        let mut state = self.state.lock().expect("桩状态锁未中毒");
        let model_for_create = model.clone();
        state.seen.push(Seen {
            path: path.clone(),
            rpc: rpc.clone(),
            connection_id,
            has_auth,
        });

        if path == "/resolve" {
            let answer = state
                .doh_answers
                .first()
                .cloned()
                .unwrap_or_else(|| serde_json::json!({"Answer": []}));
            return Ok(HttpResponse::new(200, Vec::new(), answer.to_string()));
        }
        if request.url.contains(SANDBOX_SUFFIX) {
            return Ok(sandbox_response(&mut state, rpc));
        }

        let route = if request.url.contains("pageSize=") {
            ConsoleRoute::List
        } else {
            classify(&path)
        };
        match route {
            ConsoleRoute::Create => {
                let model = model_for_create.unwrap_or_default();
                state.created_models.push(model.clone());
                // 活跃名额满：真实上游回 429 + 10105「maximum number of active」。
                // 腾位前一直拒，用来固定「先停最旧、再重试一次」这条路径。
                if state.active_limit && !state.pruned {
                    return Ok(HttpResponse::new(
                        429,
                        Vec::new(),
                        serde_json::json!({
                            "code": 10105,
                            "msg": "reached the maximum number of active conversations"
                        })
                        .to_string(),
                    ));
                }
                if state.fail_create_for.iter().any(|bad| bad == &model) {
                    return Ok(HttpResponse::new(
                        500,
                        Vec::new(),
                        serde_json::json!({"msg": "upstream boom"}).to_string(),
                    ));
                }
                Ok(HttpResponse::new(
                    200,
                    Vec::new(),
                    serde_json::json!({"code": 0, "data": {"id": "conv-1"}}).to_string(),
                ))
            }
            ConsoleRoute::Session => {
                if state.fail_phase == Some("session") {
                    return Ok(HttpResponse::new(
                        500,
                        Vec::new(),
                        serde_json::json!({"msg": "sandbox unavailable"}).to_string(),
                    ));
                }
                Ok(HttpResponse::new(
                    200,
                    Vec::new(),
                    serde_json::json!({
                        "code": 0,
                        "data": {
                            // query 里塞一个哨兵：任何把它带进错误文案的实现都会被抓到。
                            "link": "https://sbx-1.sandbox.cloudstudio.club/acp?token=SENTINEL-QUERY",
                            "token": "tok-SANDBOX-SENTINEL",
                            "sessionId": "sess-1",
                            "cwd": "/workspace",
                        }
                    })
                    .to_string(),
                ))
            }
            ConsoleRoute::Status => {
                let next = state
                    .statuses
                    .pop_front()
                    .unwrap_or_else(|| state.last_status.clone());
                state.last_status = next.clone();
                Ok(HttpResponse::new(
                    200,
                    Vec::new(),
                    serde_json::json!({"code": 0, "data": {"status": next}}).to_string(),
                ))
            }
            ConsoleRoute::Stop => {
                // 停掉任何一条会话都算腾出一个活跃名额。
                state.pruned = true;
                Ok(HttpResponse::new(
                    200,
                    Vec::new(),
                    serde_json::json!({"code": 0}).to_string(),
                ))
            }
            ConsoleRoute::Delete => Ok(HttpResponse::new(
                200,
                Vec::new(),
                serde_json::json!({"code": 0}).to_string(),
            )),
            ConsoleRoute::List => {
                state.list_requests += 1;
                let conversations: Vec<serde_json::Value> = state
                    .listed
                    .iter()
                    .map(|(id, status)| serde_json::json!({"id": id, "status": status}))
                    .collect();
                Ok(HttpResponse::new(
                    200,
                    Vec::new(),
                    serde_json::json!({"code": 0, "data": {"conversations": conversations}})
                        .to_string(),
                ))
            }
            ConsoleRoute::Other => Ok(HttpResponse::new(404, Vec::new(), "{}")),
        }
    }
}

/// 沙箱（ACP）应答。
fn sandbox_response(state: &mut StubState, rpc: Option<String>) -> HttpResponse {
    let Some(method) = rpc else {
        return HttpResponse::new(400, Vec::new(), "{}");
    };
    if state.fail_phase == Some("turn") && method == "initialize" {
        return HttpResponse::new(500, Vec::new(), "{}");
    }
    if state.reject_initialize && method == "initialize" {
        return HttpResponse::new(
            200,
            Vec::new(),
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "error": {"code": -32602, "message": "initialize rejected"}
            })
            .to_string(),
        );
    }
    if state.reject_set_model && method == "session/set_model" {
        return HttpResponse::new(
            200,
            Vec::new(),
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 3,
                "error": {"code": -32601, "message": "model not available"}
            })
            .to_string(),
        );
    }
    HttpResponse::new(
        200,
        Vec::new(),
        serde_json::json!({"jsonrpc": "2.0", "id": 1, "result": {}}).to_string(),
    )
}

impl Transport for Stub {
    fn send<'a>(&'a self, request: HttpRequest) -> BoxFuture<'a, Result<HttpResponse, String>> {
        Box::pin(async move { self.respond(&request) })
    }

    fn open_stream<'a>(
        &'a self,
        request: HttpRequest,
    ) -> BoxFuture<'a, Result<EventStream, String>> {
        Box::pin(async move {
            self.respond(&request)?;
            let connection_id = self.with(|state| state.connection_id.clone());
            Ok(EventStream::new(connection_id, None))
        })
    }
}

/// 取 URL 的路径部分（丢掉 scheme / host / query / fragment）。
fn url_path(url: &str) -> String {
    let without_fragment = url.split('#').next().unwrap_or(url);
    let without_query = without_fragment
        .split('?')
        .next()
        .unwrap_or(without_fragment);
    match without_query.find("://") {
        Some(index) => {
            let rest = &without_query[index + 3..];
            match rest.find('/') {
                Some(slash) => rest[slash..].to_string(),
                None => "/".to_string(),
            }
        }
        None => without_query.to_string(),
    }
}

/// 控制台路径归类。
fn classify(path: &str) -> ConsoleRoute {
    let Some(rest) = path.strip_prefix("/console/as/conversations") else {
        return ConsoleRoute::Other;
    };
    if rest.is_empty() || rest == "/" {
        return ConsoleRoute::Create;
    }
    let Some(tail) = rest.strip_prefix('/') else {
        return ConsoleRoute::Other;
    };
    if tail.ends_with("/session") {
        return ConsoleRoute::Session;
    }
    if tail.ends_with("/stop") {
        return ConsoleRoute::Stop;
    }
    if tail.ends_with("/delete") {
        return ConsoleRoute::Delete;
    }
    if !tail.contains('/') {
        return ConsoleRoute::Status;
    }
    ConsoleRoute::Other
}

/// 国际版账号夹具（token 用哨兵值，便于断言「没有泄漏」）。
fn ai_account() -> serde_json::Value {
    serde_json::json!({
        "id": "acc-1",
        "uid": "u-1",
        "variant": "ai",
        "domain": "www.workbuddy.ai",
        "access_token": "tok-ACCESS-SENTINEL",
    })
}
