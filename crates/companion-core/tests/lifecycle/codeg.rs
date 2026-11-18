use super::*;

#[test]
fn codeg_never_scans_existing_conversations() {
    let home = Home::new();
    let dir = home.0.join("Library/Application Support/app.codeg");
    std::fs::create_dir_all(&dir).unwrap();
    let db = rusqlite::Connection::open(dir.join("codeg.db")).unwrap();
    db.execute_batch("CREATE TABLE conversation(id TEXT,status TEXT); INSERT INTO conversation VALUES('old','running');").unwrap();
    let mut c = home.collector("codeg");
    c.poll();
    c.poll();
    assert!(c.hub.sessions.is_empty());
    for (event, status) in [
        ("user_prompt_sent", "running"),
        ("question_request", "wait"),
        ("turn_complete", "done"),
        ("user_prompt_sent", "running"),
        ("error", "error"),
    ] {
        assert!(c.ingest_hook(&json!({"source":"codeg","connection_id":"c","event":event,"body":"hello","fields":[{"label":"Question","value":"Pick A"}]})));
        assert_eq!(c.hub.sessions["codeg:connection:c"]["status"], status);
    }
    c.settings["sources"]["codeg"]["enabled"] = json!(false);
    assert!(
        !c.ingest_hook(&json!({"source":"codeg","connection_id":"c","event":"user_prompt_sent"}))
    );
}

/// Minimal loopback Codeg API so the native startup alignment can be exercised
/// without the real app; mirrors the Node fixture used by tests/codeg-hooks.
struct CodegApi {
    port: u16,
    state: std::sync::Arc<std::sync::Mutex<CodegApiState>>,
}
#[derive(Default)]
struct CodegApiState {
    hooks: serde_json::Value,
    connections: serde_json::Value,
    snapshots: std::collections::HashMap<String, serde_json::Value>,
    calls: Vec<String>,
    fail_list: bool,
}
impl CodegApi {
    fn start() -> Self {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let state = std::sync::Arc::new(std::sync::Mutex::new(CodegApiState {
            hooks: json!([]),
            connections: json!([]),
            ..Default::default()
        }));
        let shared = state.clone();
        std::thread::spawn(move || {
            for incoming in listener.incoming() {
                let Ok(mut stream) = incoming else { continue };
                let mut buf = Vec::new();
                let mut chunk = [0u8; 4096];
                let head_end = loop {
                    if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        break Some(i + 4);
                    }
                    match stream.read(&mut chunk) {
                        Ok(0) | Err(_) => break None,
                        Ok(n) => buf.extend_from_slice(&chunk[..n]),
                    }
                };
                let Some(head_end) = head_end else { continue };
                let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
                let length: usize = head
                    .lines()
                    .find_map(|line| {
                        let (key, value) = line.split_once(':')?;
                        key.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse().ok())?
                    })
                    .unwrap_or(0);
                while buf.len() < head_end + length {
                    match stream.read(&mut chunk) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => buf.extend_from_slice(&chunk[..n]),
                    }
                }
                let body: serde_json::Value =
                    serde_json::from_slice(&buf[head_end..]).unwrap_or(serde_json::Value::Null);
                let method = head
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or("/")
                    .trim_start_matches("/api/")
                    .to_string();
                let (status, result) = {
                    let mut state = shared.lock().unwrap();
                    state.calls.push(method.clone());
                    match method.as_str() {
                        "get_chat_event_webhooks" => ("200 OK", state.hooks.clone()),
                        "set_chat_event_webhooks" => {
                            state.hooks = body["webhooks"].clone();
                            ("200 OK", serde_json::Value::Null)
                        }
                        "get_chat_event_filter" => {
                            ("200 OK", json!(companion_core::adapters::CODEG_EVENTS))
                        }
                        "set_chat_event_filter" => ("200 OK", serde_json::Value::Null),
                        "list_chat_channels" => ("200 OK", json!([])),
                        "acp_list_connections" if state.fail_list => {
                            ("503 Service Unavailable", serde_json::Value::Null)
                        }
                        "acp_list_connections" => ("200 OK", state.connections.clone()),
                        "acp_get_session_snapshot" => (
                            "200 OK",
                            state
                                .snapshots
                                .get(body["connectionId"].as_str().unwrap_or(""))
                                .cloned()
                                .unwrap_or(serde_json::Value::Null),
                        ),
                        _ => ("404 Not Found", serde_json::Value::Null),
                    }
                };
                let payload = result.to_string();
                let _ = write!(stream,"HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",payload.len());
                let _ = stream.flush();
            }
        });
        Self { port, state }
    }
    fn set_connections(&self, rows: serde_json::Value) {
        self.state.lock().unwrap().connections = rows;
    }
    fn set_snapshot(&self, conn: &str, snapshot: serde_json::Value) {
        self.state
            .lock()
            .unwrap()
            .snapshots
            .insert(conn.into(), snapshot);
    }
    fn set_fail_list(&self, fail: bool) {
        self.state.lock().unwrap().fail_list = fail;
    }
    fn calls(&self) -> Vec<String> {
        self.state.lock().unwrap().calls.clone()
    }
    fn lists(&self) -> usize {
        self.calls()
            .iter()
            .filter(|m| *m == "acp_list_connections")
            .count()
    }
}
/// Codeg database with credentials plus the conversations used by the fixtures.
fn codeg_home(home: &Home, port: u16) -> Collector {
    let dir = home.0.join("Library/Application Support/app.codeg");
    std::fs::create_dir_all(&dir).unwrap();
    let db = rusqlite::Connection::open(dir.join("codeg.db")).unwrap();
    db.execute_batch(&format!(
        "CREATE TABLE app_metadata(key TEXT,value TEXT);
         CREATE TABLE conversation(id INTEGER,title TEXT,agent_type TEXT,external_id TEXT,folder_id INTEGER,status TEXT,parent_id INTEGER,kind TEXT);
         CREATE TABLE folder(id INTEGER,path TEXT);
         INSERT INTO app_metadata VALUES('web_service_port','{port}');
         INSERT INTO app_metadata VALUES('web_service_token','secret-test-token');
         INSERT INTO conversation VALUES(214,'Build feature','codex','thr-native',1,'in_progress',NULL,'regular');
         INSERT INTO conversation VALUES(215,'Child task','codex','thr-child',1,'in_progress',214,'delegate');
         INSERT INTO conversation VALUES(216,'Idle task','codex','thr-idle',1,'in_progress',NULL,'regular');
         INSERT INTO folder VALUES(1,'/project/test');"
    ))
    .unwrap();
    let mut c = home.collector("codeg");
    c.configure_codeg_webhook("http://127.0.0.1:1/api/codeg-webhook/test".into());
    c
}

#[test]
fn codeg_startup_alignment_recovers_prompting_connections() {
    let home = Home::new();
    let api = CodegApi::start();
    api.set_connections(json!([{"id":"connection-1","agent_type":"codex","status":"prompting"}]));
    api.set_snapshot("connection-1",json!({"conversation_id":214,"external_id":"thr-native","folder_id":1,"event_seq":77,"status":"prompting","pending_question":{"question_id":"q1","questions":[{"question":"Pick","options":[{"label":"A"},{"label":"B"}]}]}}));
    let mut c = codeg_home(&home, api.port);
    assert!(c.is_codeg_child_codex("thr-child"));
    assert!(!c.is_codeg_child_codex("thr-native"));
    c.poll();
    let session = c.hub.sessions["codeg:214"].clone();
    assert_eq!(
        session["status"], "wait",
        "an in-flight session is seeded without a new event"
    );
    assert_eq!(session["title"], "Build feature");
    assert_eq!(session["cwd"], "/project/test");
    assert_eq!(session["pending"][0]["id"], "q1");
    assert_eq!(session["pending"][0]["text"], "Pick");
    assert_eq!(
        session["pending"][0]["questions"][0]["options"][1]["label"],
        "B"
    );
    let snapshot = c.hub.snapshot();
    let wait = snapshot["events"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["kind"] == "wait")
        .unwrap()
        .clone();
    assert!(wait["roundId"].as_str().unwrap().starts_with("hook:"));
    // One-shot: registration succeeded, no later poll scans again.
    assert_eq!(api.lists(), 1);
    c.poll();
    assert_eq!(api.lists(), 1);
    // A late completion webhook cannot end the recovered round.
    assert!(c.ingest_hook(
        &json!({"source":"codeg","connection_id":"connection-1","event":"turn_complete"})
    ));
    assert_eq!(c.hub.sessions["codeg:214"]["status"], "wait");
    // The alignment records the connection, so a stale snapshot never invents a duplicate.
    api.set_snapshot("connection-1", serde_json::Value::Null);
    assert!(c.ingest_hook(
        &json!({"source":"codeg","connection_id":"connection-1","event":"question_request"})
    ));
    assert_eq!(c.hub.sessions.len(), 1);
    assert!(c.hub.sessions.contains_key("codeg:214"));
}

#[test]
fn codeg_startup_alignment_seeds_prompting_without_pending_as_running() {
    let home = Home::new();
    let api = CodegApi::start();
    api.set_connections(json!([{"id":"connection-1","agent_type":"codex","status":"prompting"}]));
    api.set_snapshot("connection-1",json!({"conversation_id":214,"external_id":"thr-native","folder_id":1,"status":"prompting","event_seq":9}));
    let mut c = codeg_home(&home, api.port);
    c.poll();
    let session = c.hub.sessions["codeg:214"].clone();
    assert_eq!(
        session["status"], "running",
        "a prompting session without pending requests is seeded as running"
    );
    assert!(session["pending"].as_array().unwrap().is_empty());
    assert_eq!(session["title"], "Build feature");
    assert_eq!(session["cwd"], "/project/test");
    let snapshot = c.hub.snapshot();
    assert_eq!(
        snapshot["sessions"].as_array().unwrap().len(),
        1,
        "the recovered session is visible"
    );
    assert!(
        snapshot["events"]
            .as_array()
            .unwrap()
            .iter()
            .all(|e| e["kind"] != "wait"),
        "no pending request means no wait event"
    );
}

#[test]
fn codeg_startup_alignment_seeds_connected_connection_with_pending() {
    let home = Home::new();
    let api = CodegApi::start();
    api.set_connections(json!([{"id":"connection-1","agent_type":"codex","status":"connected"}]));
    api.set_snapshot("connection-1",json!({"conversation_id":214,"status":"connected","event_seq":4,"pending_question":{"question_id":"q7","questions":[{"question":"Keep waiting?"}]}}));
    let mut c = codeg_home(&home, api.port);
    c.poll();
    let session = c.hub.sessions["codeg:214"].clone();
    assert_eq!(
        session["status"], "wait",
        "a pending request outranks the connected status"
    );
    assert_eq!(session["pending"][0]["id"], "q7");
    assert_eq!(session["pending"][0]["text"], "Keep waiting?");
    assert!(c.hub.snapshot()["events"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e["kind"] == "wait"));
}

#[test]
fn codeg_startup_alignment_skips_idle_child_and_snapshot_less_connections() {
    let home = Home::new();
    let api = CodegApi::start();
    api.set_connections(json!([{"id":"idle"},{"id":"child"},{"id":"orphan"},{"id":""}]));
    api.set_snapshot(
        "idle",
        json!({"conversation_id":216,"status":"connected","event_seq":3}),
    );
    api.set_snapshot("child",json!({"conversation_id":215,"status":"prompting","event_seq":4,"pending_question":{"question_id":"q9","questions":[{"question":"Child?"}]}}));
    api.set_snapshot("orphan", json!({"status":"prompting","event_seq":5}));
    let mut c = codeg_home(&home, api.port);
    c.hub.ingest(json!({"source":"codex","sessionId":"thr-child","type":"start","roundId":"r1","cwd":"/project/test"}));
    c.hub.ingest(json!({"source":"codex","sessionId":"thr-child","type":"wait","roundId":"r1","callId":"ask","text":"Child question"}));
    c.poll();
    assert_eq!(
        c.hub.sessions.len(),
        1,
        "the child Codex hook is retained internally"
    );
    assert!(
        c.hub.snapshot()["sessions"].as_array().unwrap().is_empty(),
        "the child Codex hook is hidden from the rail"
    );
    assert!(c.hub.snapshot()["events"].as_array().unwrap().is_empty());
    assert!(
        !c.ingest_codeg_stream(&json!({"type":"event","connection_id":"child"})),
        "skipped connections are never subscribed"
    );
}

#[test]
fn codeg_child_codex_hook_is_hidden_before_codeg_callback() {
    let home = Home::new();
    let mut c = codeg_home(&home, 1);
    c.settings["sources"]["codex"]["enabled"] = json!(true);
    assert!(c.ingest_hook(&json!({"session_id":"thr-child","hook_event_name":"UserPromptSubmit","prompt":"Child work"})));
    assert!(c.hub.snapshot()["sessions"].as_array().unwrap().is_empty());
    assert!(c.ingest_hook(&json!({"session_id":"native-cli","hook_event_name":"UserPromptSubmit","prompt":"Independent work"})));
    assert_eq!(c.hub.snapshot()["sessions"].as_array().unwrap().len(), 1);
}

#[test]
fn codeg_startup_alignment_never_restarts_a_known_round() {
    let home = Home::new();
    let api = CodegApi::start();
    api.set_connections(json!([{"id":"connection-1"}]));
    api.set_snapshot("connection-1",json!({"conversation_id":214,"status":"prompting","event_seq":30,"pending_question":{"question_id":"q1","questions":[{"question":"Pick"}]}}));
    let mut c = codeg_home(&home, api.port);
    // A webhook won the race: the hub already owns this round.
    c.hub.ingest(
        json!({"source":"codeg","sessionId":"214","type":"start","roundId":"hook:1:1","ts":1}),
    );
    c.poll();
    assert_eq!(c.hub.sessions["codeg:214"]["roundId"], "hook:1:1");
    assert!(c.hub.sessions["codeg:214"]["pending"]
        .as_array()
        .unwrap()
        .is_empty());
    // Re-enabling the integration re-arms exactly one more alignment.
    let before = api.lists();
    c.configure_codeg_webhook("http://127.0.0.1:1/api/codeg-webhook/test".into());
    c.poll();
    assert_eq!(api.lists(), before + 1);
    assert_eq!(c.hub.sessions.len(), 1);
}

#[test]
fn codeg_startup_alignment_failure_only_degrades_health() {
    let home = Home::new();
    let api = CodegApi::start();
    api.set_fail_list(true);
    let mut c = codeg_home(&home, api.port);
    c.poll();
    assert!(
        c.codeg.registered,
        "registration survives an alignment failure"
    );
    assert_eq!(c.hub.sources["codeg"]["state"], "error");
    assert_eq!(c.hub.sources["codeg"]["detail"], "Codeg Web Service 不可用");
    assert!(c.hub.sessions.is_empty());
    assert_eq!(api.lists(), 1);
    api.set_fail_list(false);
    let calls = api.calls().len();
    c.poll();
    assert_eq!(
        api.calls().len(),
        calls,
        "no retry storm and no second scan"
    );
    assert_eq!(api.lists(), 1);
}
