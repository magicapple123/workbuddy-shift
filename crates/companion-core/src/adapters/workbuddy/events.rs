//! WorkBuddy hook 事件解析与 ingest。

use super::*;

/// 文案取自 `message` / `title`；都没有时用调用方给的兜底文案
/// （授权请求与提问的兜底措辞不同，不能共用一句「需要你确认」）。
pub(crate) fn wait_text(p: &Value, fallback: &str) -> String {
    let message = content(&p["message"]);
    if !message.trim().is_empty() {
        return message;
    }
    let title = p["title"].as_str().map(str::trim).unwrap_or("");
    if !title.is_empty() {
        return title.to_owned();
    }
    fallback.to_owned()
}

pub(crate) fn confirmation_notification(p: &Value) -> bool {
    let kind = p["notification_type"]
        .as_str()
        .or(p["notificationType"].as_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if kind == "auth_success" {
        return false;
    }
    if kind == "idle_prompt"
        || kind.contains("permission")
        || kind.contains("confirm")
        || kind.contains("approval")
        || kind.contains("credential")
    {
        return true;
    }
    let text = format!(
        "{} {}",
        content(&p["message"]),
        p["title"].as_str().unwrap_or("")
    );
    text.contains('?')
        || text.contains('？')
        || text.contains("确认")
        || text.contains("Allow ")
        || text.contains("Deny ")
        || text.contains("允许")
        || text.contains("拒绝")
        || text.to_ascii_lowercase().contains("credential")
        || text.contains("凭证")
}

impl Collector {
    pub fn ingest_workbuddy_hook(&mut self, p: &Value) -> bool {
        if self.settings["sources"]["workbuddy"]["enabled"] != true {
            return false;
        }
        let get = |a: &str, b: &str| p[a].as_str().or(p[b].as_str()).unwrap_or("").to_owned();
        let sid = get("session_id", "sessionId");
        let event = get("hook_event_name", "hookEventName");
        if sid.is_empty()
            || !matches!(
                event.as_str(),
                "SessionStart"
                    | "SessionEnd"
                    | "UserPromptSubmit"
                    | "PreToolUse"
                    | "PostToolUse"
                    | "PostToolUseFailure"
                    | "PermissionRequest"
                    | "Stop"
                    | "Notification"
                    | "PreCompact"
                    | "Interrupt"
            )
        {
            return false;
        }
        let key = format!("workbuddy:{sid}");
        if self.closed_monitor_sessions.contains(&key) {
            if matches!(event.as_str(), "Stop" | "SessionEnd" | "Interrupt") {
                return false;
            }
            if matches!(
                event.as_str(),
                "SessionStart" | "UserPromptSubmit" | "PreToolUse" | "PermissionRequest"
            ) {
                self.closed_monitor_sessions.remove(&key);
            }
        }
        let ts = p["timestamp"]
            .as_i64()
            .filter(|n| *n > 0)
            .unwrap_or_else(now);
        let previous = self.workbuddy_live.get(&sid).cloned();
        let mut state = previous
            .clone()
            .unwrap_or(json!({"roundId":"","cwd":"","calls":{}}));
        let turn = get("turn_id", "turnId");
        let begins = matches!(event.as_str(), "SessionStart" | "UserPromptSubmit");
        let round = if !turn.is_empty() {
            turn
        } else if begins {
            format!("turn:{ts}")
        } else {
            state["roundId"]
                .as_str()
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .unwrap_or(format!("turn:{ts}"))
        };
        if previous.is_some() && state["roundId"] != round && !begins {
            return false;
        }
        if previous.is_none() || state["roundId"] != round {
            state["calls"] = json!({});
        }
        state["roundId"] = json!(round);
        if let Some(cwd) = p["cwd"].as_str().filter(|s| !s.is_empty()) {
            state["cwd"] = json!(cwd);
        }
        let edition = get("agent_edition", "agentEdition");
        let agent_type = if matches!(edition.as_str(), "international" | "workbuddy-ai") {
            "workbuddy-ai".to_owned()
        } else if matches!(edition.as_str(), "domestic" | "workbuddy") {
            "workbuddy".to_owned()
        } else {
            state["agentType"]
                .as_str()
                .filter(|s| !s.is_empty())
                .unwrap_or("workbuddy")
                .to_owned()
        };
        state["agentType"] = json!(agent_type);
        let base = json!({
            "source":"workbuddy",
            "sessionId":sid,
            "cwd":state["cwd"],
            "agentType":agent_type,
            "roundId":round,
            "ts":ts
        });
        let mut emit = |ev| self.hub.ingest(merge(base.clone(), ev));
        if previous.is_none() || begins {
            emit(json!({"type":"start"}));
        }
        let title = p["session_title"]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .unwrap_or_default();
        let tool = get("tool_name", "toolName");
        let id = get("tool_use_id", "toolUseId");
        let input = p
            .get("tool_input")
            .or(p.get("toolInput"))
            .cloned()
            .unwrap_or(Value::Null);
        match event.as_str() {
            "UserPromptSubmit" => {
                for (call, c) in state["calls"].as_object().unwrap() {
                    if c["async"] == true || c["permission"] == true {
                        emit(json!({"type":"resolve","callId":call}));
                    }
                }
                let prompt = content(&p["prompt"]);
                let label = if !title.is_empty() { title } else { prompt };
                if !label.trim().is_empty() {
                    emit(json!({"type":"meta","title":label}));
                }
            }
            "PreToolUse" => {
                if !id.is_empty() && state["calls"][&id]["resolved"] != true {
                    state["calls"][&id] = json!({
                        "tool":tool,
                        "resolved":false,
                        "async":tool.ends_with("request_user_input_async"),
                        "ts":ts
                    });
                    if question_tool(&tool) {
                        emit(json!({
                            "type":"wait",
                            "callId":id,
                            "kind":"question",
                            "tool":tool,
                            "text":question(&input),
                            "questions":questions(&input)
                        }));
                    } else {
                        emit(json!({"type":"step","eventId":id,"label":tool}));
                    }
                }
            }
            "PostToolUse" | "PostToolUseFailure" => {
                if !id.is_empty() {
                    let is_async = state["calls"][&id]["async"] == true
                        || tool.ends_with("request_user_input_async");
                    state["calls"][&id] = json!({
                        "tool":tool,
                        "resolved":true,
                        "async":is_async,
                        "ts":ts
                    });
                    if !is_async {
                        emit(json!({"type":"resolve","callId":id}));
                    }
                }
                // 权限确认通过后客户端会继续执行同一个工具：`PermissionRequest`
                // 没带 `tool_use_id` 时用的是 `perm:<ts>` 占位，按 id 对不上，
                // 这里按工具名兜底清掉，避免卡片一直挂着。
                let stale: Vec<String> = state["calls"]
                    .as_object()
                    .map(|calls| {
                        calls
                            .iter()
                            .filter(|(call, c)| {
                                c["permission"] == true
                                    && c["resolved"] != true
                                    && *call != &id
                                    && !tool.is_empty()
                                    && c["tool"] == json!(tool)
                            })
                            .map(|(call, _)| call.clone())
                            .collect()
                    })
                    .unwrap_or_default();
                for call in stale {
                    if let Some(c) = state["calls"][&call].as_object_mut() {
                        c.insert("resolved".into(), json!(true));
                    }
                    emit(json!({"type":"resolve","callId":call}));
                }
            }
            "PermissionRequest" => {
                let call = if id.is_empty() {
                    format!("perm:{ts}")
                } else {
                    id.clone()
                };
                if state["calls"][&call]["resolved"] != true {
                    state["calls"][&call] = json!({
                        "tool":if tool.is_empty() {"permission"} else {&tool},
                        "resolved":false,
                        "permission":true,
                        "ts":ts
                    });
                    // 只做检测，不替用户做「允许/拒绝」决策；`graceMs` 让 hub 在
                    // 2.5 秒内保持沉默，覆盖客户端的自动批准。
                    emit(json!({
                        "type":"wait",
                        "callId":call,
                        "kind":"permission",
                        "graceMs":crate::hub::PERMISSION_GRACE_MS,
                        "tool":if tool.is_empty() {"permission"} else {&tool},
                        "text":wait_text(p, "需要你授权")
                    }));
                }
            }
            "Stop" | "SessionEnd" | "Interrupt" => {
                emit(json!({
                    "type":"end",
                    "status":if event=="Interrupt" {"aborted"} else {"done"}
                }));
            }
            "Notification" => {
                if !title.is_empty() {
                    emit(json!({"type":"meta","title":title}));
                }
                if confirmation_notification(p) {
                    let call = if id.is_empty() {
                        format!("notify:{ts}")
                    } else {
                        id.clone()
                    };
                    if state["calls"][&call]["resolved"] != true {
                        state["calls"][&call] = json!({
                            "tool":"notification",
                            "resolved":false,
                            "permission":true,
                            "ts":ts
                        });
                        emit(json!({
                            "type":"wait",
                            "callId":call,
                            "kind":"permission",
                            "graceMs":crate::hub::PERMISSION_GRACE_MS,
                            "tool":"notification",
                            "text":wait_text(p, "需要你授权")
                        }));
                    }
                } else {
                    emit(json!({"type":"activity"}));
                }
            }
            _ => {
                if !title.is_empty() {
                    emit(json!({"type":"meta","title":title}));
                }
            }
        }
        while state["calls"].as_object().unwrap().len() > 256 {
            let oldest = state["calls"]
                .as_object()
                .unwrap()
                .iter()
                .filter(|(_, v)| v["resolved"] == true)
                .min_by_key(|(_, v)| v["ts"].as_i64().unwrap_or(0))
                .map(|(k, _)| k.clone());
            if let Some(key) = oldest {
                state["calls"].as_object_mut().unwrap().remove(&key);
            } else {
                break;
            }
        }
        self.workbuddy_live.insert(sid, state);
        self.workbuddy_hook_count += 1;
        self.workbuddy_presence.note_hook();
        self.poll_workbuddy().ok();
        true
    }
}

#[cfg(test)]
mod permission_tests {
    use super::*;
    use std::path::PathBuf;

    struct Home(PathBuf);
    impl Home {
        fn new() -> Self {
            static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "wb-permission-{}-{}-{}",
                std::process::id(),
                now(),
                SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
        fn collector(&self) -> Collector {
            let mut settings = crate::settings::defaults();
            for id in crate::settings::SOURCES {
                settings["sources"][id]["enabled"] = json!(id == "workbuddy");
            }
            crate::atomic_json(
                &self.0.join(".workbuddy-shift-companion/settings.json"),
                &settings,
            )
            .unwrap();
            Collector::new(self.0.clone()).unwrap()
        }
    }
    impl Drop for Home {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn pending(c: &Collector, sid: &str) -> Vec<Value> {
        c.hub.sessions[&format!("workbuddy:{sid}")]["pending"]
            .as_array()
            .cloned()
            .unwrap_or_default()
    }

    fn visible_pending(c: &Collector, sid: &str) -> Vec<Value> {
        c.hub.snapshot()["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["id"] == json!(format!("workbuddy:{sid}")))
            .map(|s| s["pending"].as_array().cloned().unwrap_or_default())
            .unwrap_or_default()
    }

    fn start(c: &mut Collector, sid: &str, ts: i64) {
        assert!(c.ingest_workbuddy_hook(&json!({
            "source":"workbuddy","session_id":sid,"hook_event_name":"UserPromptSubmit","prompt":"go","timestamp":ts
        })));
    }

    #[test]
    fn permission_request_is_a_held_permission_wait() {
        let home = Home::new();
        let mut c = home.collector();
        let t = now();
        start(&mut c, "p", t);
        assert!(c.ingest_workbuddy_hook(&json!({
            "source":"workbuddy","session_id":"p","hook_event_name":"PermissionRequest",
            "tool_name":"Bash","tool_use_id":"perm-tool","message":"允许删除 3 个文件？","timestamp":t + 1
        })));
        let items = pending(&c, "p");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["kind"], json!(crate::hub::KIND_PERMISSION));
        assert_eq!(items[0]["tool"], json!("Bash"));
        assert_eq!(
            items[0]["graceUntil"].as_i64(),
            Some(t + 1 + crate::hub::PERMISSION_GRACE_MS)
        );
        // 仍在 2.5s 宽限期内：不入快照、不改状态。
        assert!(visible_pending(&c, "p").is_empty());
        assert_eq!(c.hub.sessions["workbuddy:p"]["status"], json!("wait"));
    }

    #[test]
    fn notification_confirmation_is_a_permission_wait() {
        let home = Home::new();
        let mut c = home.collector();
        let t = now();
        start(&mut c, "n", t);
        assert!(c.ingest_workbuddy_hook(&json!({
            "source":"workbuddy","session_id":"n","hook_event_name":"Notification",
            "notification_type":"permission_request","message":"允许访问凭证？","timestamp":t + 1
        })));
        let items = pending(&c, "n");
        assert_eq!(items[0]["kind"], json!(crate::hub::KIND_PERMISSION));
        assert!(items[0]["graceUntil"].as_i64().is_some());
        // 非确认类通知不产生待确认项。
        assert!(c.ingest_workbuddy_hook(&json!({
            "source":"workbuddy","session_id":"n","hook_event_name":"Notification",
            "notification_type":"auth_success","timestamp":t + 2
        })));
        assert_eq!(pending(&c, "n").len(), 1);
    }

    #[test]
    fn question_tool_is_a_question_wait_without_grace() {
        let home = Home::new();
        let mut c = home.collector();
        let t = now();
        start(&mut c, "q", t);
        assert!(c.ingest_workbuddy_hook(&json!({
            "source":"workbuddy","session_id":"q","hook_event_name":"PreToolUse",
            "tool_name":"AskUserQuestion","tool_use_id":"q1",
            "tool_input":{"questions":[{"question":"选哪个？"}]},"timestamp":t + 1
        })));
        let items = pending(&c, "q");
        assert_eq!(items[0]["kind"], json!(crate::hub::KIND_QUESTION));
        assert_eq!(items[0]["text"], json!("选哪个？"));
        assert!(items[0]["graceUntil"].is_null());
        // 提问不被宽限期挡住。
        assert_eq!(visible_pending(&c, "q").len(), 1);
    }

    #[test]
    fn tool_result_clears_a_permission_wait_that_has_no_tool_use_id() {
        let home = Home::new();
        let mut c = home.collector();
        let t = now();
        start(&mut c, "r", t);
        assert!(c.ingest_workbuddy_hook(&json!({
            "source":"workbuddy","session_id":"r","hook_event_name":"PermissionRequest",
            "tool_name":"Bash","message":"允许执行？","timestamp":t + 1
        })));
        assert_eq!(pending(&c, "r").len(), 1);
        // 客户端批准后继续执行工具：PostToolUse 带的是另一个 id。
        assert!(c.ingest_workbuddy_hook(&json!({
            "source":"workbuddy","session_id":"r","hook_event_name":"PostToolUse",
            "tool_name":"Bash","tool_use_id":"tool-run","timestamp":t + 2
        })));
        assert!(pending(&c, "r").is_empty());
        assert_eq!(c.hub.sessions["workbuddy:r"]["status"], json!("running"));
    }

    #[test]
    fn resolve_inside_the_grace_window_leaves_nothing_visible() {
        let home = Home::new();
        let mut c = home.collector();
        let t = now();
        start(&mut c, "g", t);
        assert!(c.ingest_workbuddy_hook(&json!({
            "source":"workbuddy","session_id":"g","hook_event_name":"PermissionRequest",
            "tool_name":"Write","tool_use_id":"w1","message":"允许写入？","timestamp":t + 1
        })));
        assert!(c.ingest_workbuddy_hook(&json!({
            "source":"workbuddy","session_id":"g","hook_event_name":"PostToolUse",
            "tool_name":"Write","tool_use_id":"w1","timestamp":t + 2
        })));
        assert!(pending(&c, "g").is_empty());
        assert!(visible_pending(&c, "g").is_empty());
        // 也没有任何 wait 事件被播出去（否则托盘通知会误报）。
        assert!(c.hub.snapshot()["events"]
            .as_array()
            .unwrap()
            .iter()
            .all(|e| e["kind"] != json!("wait")));
    }
}
