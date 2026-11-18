//! Webhook discovery plus read-only confirmation streams; no session scans.
//! Credentials at registration; keyed session metadata only after an event.
use super::*;
use std::time::{Duration, Instant};

mod child;
mod events;

use child::ChildState;
use events::codeg_title;

pub const CODEG_EVENTS: [&str; 5] = [
    "user_prompt_sent",
    "question_request",
    "permission_request",
    "turn_complete",
    "error",
];
/// The one-shot startup alignment enumerates at most this many connections.
const CODEG_ALIGN_LIMIT: usize = 64;
/// A delegated child never keeps its temporary card longer than this without
/// an answer; a cancelled child emits no signal at all, so this is the floor.
const CODEG_CHILD_TTL: i64 = 60 * 60 * 1000;
/// Answered request ids a child remembers against late duplicate webhooks.
const CODEG_CHILD_ANSWERED: usize = 8;
/// Child titles are raw prompt fragments; the rail gets one capped line.
const CODEG_CHILD_TITLE: usize = 80;

#[derive(Default)]
pub struct CodegHooks {
    pub url: String,
    pub registered: bool,
    pub reconciled: bool,
    pub next_attempt: Option<Instant>,
    auth: Option<(u16, String)>,
    connections: HashMap<String, String>,
    children: HashMap<String, ChildState>,
    sequence: u64,
    pub stream_sink: Option<super::codeg_stream::Sink>,
    streams: HashMap<String, super::codeg_stream::Stream>,
}
pub fn merge_codeg_webhooks(
    existing: &Value,
    owned: &[String],
    url: &str,
) -> Result<Value, String> {
    let list = existing
        .as_array()
        .ok_or("Codeg Webhook 配置无效，未覆盖")?;
    if list
        .iter()
        .any(|w| !w["url"].is_string() || !w["enabled"].is_boolean())
    {
        return Err("Codeg Webhook 配置无效，未覆盖".into());
    }
    let mut next: Vec<_> = list
        .iter()
        .filter(|w| w["url"] != url && !owned.contains(&text(&w["url"])))
        .cloned()
        .collect();
    if !url.is_empty() {
        next.push(json!({"url":url,"enabled":true}));
    }
    Ok(json!(next))
}
fn owned_urls(saved: &Value) -> Result<Vec<String>, String> {
    saved["owned"]
        .as_array()
        .ok_or("Codeg 注册记录无效")?
        .iter()
        .map(|v| {
            v.as_str()
                .map(str::to_owned)
                .ok_or_else(|| "Codeg 注册记录无效".to_string())
        })
        .collect()
}
fn post(auth: &(u16, String), method: &str, body: Value) -> Result<Value, String> {
    ureq::AgentBuilder::new()
        .timeout(Duration::from_millis(1500))
        .build()
        .post(&format!("http://127.0.0.1:{}/api/{method}", auth.0))
        .set("Authorization", &format!("Bearer {}", auth.1))
        .send_json(body)
        .map_err(|_| "Codeg Web Service 不可用")?
        .into_json()
        .map_err(|_| "Codeg API 返回无效".to_string())
}
impl Collector {
    fn codeg_credentials(&self) -> Result<(u16, String), String> {
        let db = open(&self.paths("codeg"))?;
        let rows = query(&db,"SELECT key,value FROM app_metadata WHERE key IN ('web_service_port','web_service_token')")?;
        let value = |k: &str| {
            rows.iter()
                .find(|r| r["key"] == k)
                .map(|r| text(&r["value"]))
                .unwrap_or_default()
        };
        let port = value("web_service_port")
            .parse::<u16>()
            .ok()
            .filter(|p| *p > 0)
            .unwrap_or(3080);
        let token = value("web_service_token");
        if token.is_empty() {
            return Err("请启用 Codeg Web Service".into());
        }
        Ok((port, token))
    }
    pub fn set_codeg_stream_sink(&mut self, sink: super::codeg_stream::Sink) {
        self.codeg.stream_sink = Some(sink);
    }
    pub fn configure_codeg_webhook(&mut self, url: String) {
        self.codeg.url = url;
        self.codeg.registered = false;
        self.codeg.reconciled = false;
        self.codeg.next_attempt = None;
    }
    pub fn maintain_codeg_webhook(&mut self) -> Result<(), String> {
        if self.settings["sources"]["codeg"]["enabled"] != true
            || !self.integration_automatic("codeg")
        {
            self.codeg.streams.clear();
            // Disabling releases every temporary child card with its stream.
            self.codeg_release_children(None);
        } else {
            // A cancelled or silently finished child emits no signal at all.
            self.codeg_release_children(Some(CODEG_CHILD_TTL));
        }
        if self.codeg.url.is_empty() {
            self.hub
                .health("codeg", "partial", "Webhook 接收入口尚未启动");
            return Ok(());
        }
        if self.codeg.registered
            || self
                .codeg
                .next_attempt
                .is_some_and(|at| Instant::now() < at)
        {
            return Ok(());
        }
        self.codeg.next_attempt = Some(Instant::now() + Duration::from_secs(60));
        let enabled = self.settings["sources"]["codeg"]["enabled"] == true
            && self.integration_automatic("codeg");
        let file = self
            .home
            .join(".workbuddy-shift-companion/codeg-webhook-native.json");
        let saved: Value = match std::fs::read(&file) {
            Ok(b) => serde_json::from_slice(&b).map_err(|_| "Codeg 注册记录无效，未覆盖")?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => json!({"owned":[]}),
            Err(_) => return Err("Codeg 注册记录不可读".into()),
        };
        let mut owned = owned_urls(&saved)?;
        if !enabled && owned.is_empty() {
            self.codeg.registered = true;
            return Ok(());
        }
        let auth = self.codeg_credentials()?;
        let existing = post(&auth, "get_chat_event_webhooks", json!({}))?;
        let url = if enabled {
            self.codeg.url.clone()
        } else {
            String::new()
        };
        let next = merge_codeg_webhooks(&existing, &owned, &url)?;
        if enabled {
            let filter = post(&auth, "get_chat_event_filter", json!({}))?;
            let mut current: Vec<Value> = if filter.is_null() {
                CODEG_EVENTS[1..].iter().map(|e| json!(e)).collect()
            } else {
                filter
                    .as_array()
                    .filter(|a| a.iter().all(Value::is_string))
                    .cloned()
                    .ok_or("Codeg 事件配置无效")?
            };
            if CODEG_EVENTS.iter().any(|e| !current.contains(&json!(e))) {
                let channels = post(&auth, "list_chat_channels", json!({}))?;
                if channels
                    .as_array()
                    .ok_or("Codeg 推送配置无效")?
                    .iter()
                    .any(|c| c["enabled"] == true)
                    || next
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|w| w["url"] != url && w["enabled"] == true)
                {
                    return Err("Codeg 全局开关影响其他推送目标；请先在 Codeg 启用五类事件".into());
                }
                for e in CODEG_EVENTS {
                    if !current.contains(&json!(e)) {
                        current.push(json!(e));
                    }
                }
                post(&auth, "set_chat_event_filter", json!({"filter":current}))?;
            }
        }
        if !url.is_empty() && !owned.contains(&url) {
            owned.push(url.clone());
        }
        // Persist ownership before sending; uncertain network responses are safe to retry.
        atomic_json(&file, &json!({"owned":owned}))?;
        if existing != next {
            post(&auth, "set_chat_event_webhooks", json!({"webhooks":next}))?;
        }
        let verified = post(&auth, "get_chat_event_webhooks", json!({}))?;
        if verified != next {
            return Err("Codeg 未确认 Webhook 配置，请重试".into());
        }
        atomic_json(
            &file,
            &json!({"owned":if url.is_empty(){vec![]}else{vec![url]}}),
        )?;
        self.codeg.auth = Some(auth);
        self.codeg.registered = true;
        self.hub.health(
            "codeg",
            if enabled { "ok" } else { "disabled" },
            if enabled {
                "Webhook 已注册，等待 Codeg 事件（不扫描会话）"
            } else {
                "已关闭监听"
            },
        );
        // One-shot startup alignment; failures only show on codeg health.
        if enabled && !self.codeg.reconciled {
            self.codeg.reconciled = true;
            if let Err(e) = self.reconcile_codeg_connections() {
                self.hub.health("codeg", "error", &e);
            }
        }
        Ok(())
    }
    pub fn inspect_codeg_webhook(&self) -> Result<(&'static str, String), String> {
        let file = self
            .home
            .join(".workbuddy-shift-companion/codeg-webhook-native.json");
        let owned = match std::fs::read(file) {
            Ok(bytes) => {
                serde_json::from_slice::<Value>(&bytes).map_err(|_| "Codeg 注册记录无效")?
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => json!({"owned":[]}),
            Err(e) => return Err(e.to_string()),
        };
        let owned = owned_urls(&owned)?;
        let pending = (!self.integration_automatic("codeg")
            || self.settings["sources"]["codeg"]["enabled"] != true)
            && !owned.is_empty();
        let result = self
            .codeg_credentials()
            .and_then(|auth| post(&auth, "get_chat_event_webhooks", json!({})));
        match result {
            Err(e) => Ok((
                if pending { "pending" } else { "unavailable" },
                if pending {
                    format!("待注销；{e}，请启动 Codeg 后重试")
                } else {
                    e
                },
            )),
            Ok(value) => {
                let list = value.as_array().ok_or("Codeg Webhook 配置无效")?;
                let found = list
                    .iter()
                    .any(|v| v["url"] == self.codeg.url && v["enabled"] == true);
                if pending {
                    Ok(("pending", "接入已暂停；待重试注销".into()))
                } else if found {
                    let filter = post(
                        &self.codeg_credentials()?,
                        "get_chat_event_filter",
                        json!({}),
                    )?;
                    if !CODEG_EVENTS
                        .iter()
                        .all(|e| filter.as_array().is_some_and(|a| a.contains(&json!(e))))
                    {
                        Ok(("partial", "Webhook 已注册，但事件开关不完整，请修复".into()))
                    } else {
                        Ok(("installed", "Webhook 已注册".into()))
                    }
                } else {
                    Ok(("not_installed", "Webhook 未注册；注册需要开启监听".into()))
                }
            }
        }
    }
    pub fn stop_codeg_webhook(&mut self) {
        self.codeg.streams.clear();
        self.hub.hidden_codeg_codex_ids.clear();
        self.settings["sources"]["codeg"]["enabled"] = json!(false);
        self.codeg.registered = false;
        self.codeg.next_attempt = None;
        let _ = self.maintain_codeg_webhook();
    }
    fn codeg_metadata(&self, sid: &str) -> Result<Value, String> {
        let db = open(&self.paths("codeg"))?;
        // Parameter binding: the callback never supplies SQL identifiers or SQL text.
        let table = if db.prepare("SELECT id FROM conversation LIMIT 0").is_ok() {
            "conversation"
        } else {
            "conversations"
        };
        let mut st = db
            .prepare(&format!("SELECT * FROM {table} WHERE id=?1"))
            .map_err(|_| "Codeg 会话表不可读")?;
        let names: Vec<String> = st.column_names().iter().map(|s| s.to_string()).collect();
        let raw = st
            .query_row([sid], |r| {
                let mut out = serde_json::Map::new();
                for (i, n) in names.iter().enumerate() {
                    let v = match r.get_ref(i)? {
                        ValueRef::Text(b) => json!(String::from_utf8_lossy(b)),
                        ValueRef::Integer(n) => json!(n),
                        _ => Value::Null,
                    };
                    out.insert(n.clone(), v);
                }
                Ok(Value::Object(out))
            })
            .map_err(|_| "Codeg 会话未找到")?;
        let get = |keys: &[&str]| {
            keys.iter()
                .map(|k| raw[*k].clone())
                .find(|v| !v.is_null() && !text(v).is_empty())
                .unwrap_or(Value::Null)
        };
        let mut cwd = get(&["origin_cwd", "cwd", "workspace"]);
        if text(&cwd).is_empty() {
            if let Ok(p) = db.query_row(
                "SELECT path FROM folder WHERE id=?1",
                [text(&raw["folder_id"])],
                |r| r.get::<_, String>(0),
            ) {
                cwd = json!(p);
            }
        }
        let mut meta = json!({"cwd":cwd,"title":raw["title"],"agentType":get(&["agent_type","agent"]),"externalId":raw["external_id"],"folderId":raw["folder_id"],"isSubagent":!raw["parent_id"].is_null() || raw["kind"] == "delegate"});
        if meta["isSubagent"] == true && !raw["parent_id"].is_null() {
            // One extra keyed lookup: the badge names the session the child was
            // delegated from. An unreadable parent only drops that detail.
            let mut st = db
                .prepare(&format!("SELECT title FROM {table} WHERE id=?1"))
                .map_err(|_| "Codeg 会话表不可读")?;
            if let Ok(Some(title)) =
                st.query_row([text(&raw["parent_id"])], |r| r.get::<_, Option<String>>(0))
            {
                if !title.trim().is_empty() {
                    meta["parentTitle"] = json!(codeg_title(&title));
                }
            }
        }
        Ok(meta)
    }
    pub fn is_codeg_child_codex(&self, external_id: &str) -> bool {
        if self.settings["sources"]["codeg"]["enabled"] != true
            || !self.integration_automatic("codeg")
            || external_id.is_empty()
        {
            return false;
        }
        let Ok(db) = open(&self.paths("codeg")) else {
            return false;
        };
        let table = if db.prepare("SELECT id FROM conversation LIMIT 0").is_ok() {
            "conversation"
        } else {
            "conversations"
        };
        let id = external_id.strip_prefix("thr_").unwrap_or(external_id);
        let Ok(mut st) = db.prepare(&format!(
            "SELECT id FROM {table} WHERE external_id IN (?1, ?2) LIMIT 1"
        )) else {
            return false;
        };
        let Ok(sid) = st.query_row(rusqlite::params![id, format!("thr_{id}")], |r| {
            r.get::<_, i64>(0)
        }) else {
            return false;
        };
        let Ok(meta) = self.codeg_metadata(&sid.to_string()) else {
            return false;
        };
        text(&meta["agentType"]).eq_ignore_ascii_case("codex") && meta["isSubagent"] == true
    }
}
