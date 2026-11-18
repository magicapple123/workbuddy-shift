//! Codeg 委派子会话：临时卡片、TTL 与释放规则。
use super::events::{codeg_ask, codeg_title, request_id};
use super::*;
use crate::{merge, question, questions};
use std::collections::VecDeque;

mod cards;
mod metadata;

pub(crate) use metadata::ChildState;

#[cfg(test)]
mod child_card_tests {
    use super::super::events::{codeg_ask, codeg_title, stream_ask};
    use super::*;
    use crate::settings;
    use std::io::{Read, Write};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    struct Home(std::path::PathBuf);
    impl Home {
        fn new() -> Self {
            static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let p = std::env::temp_dir().join(format!(
                "codeg-child-{}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
                SEQ.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&p).unwrap();
            Self(p)
        }
    }
    impl Drop for Home {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Minimal loopback Codeg API: only the keyed snapshot the adapter asks for
    /// after a webhook. Unlisted connections answer `null`, like a connection
    /// that is already gone.
    struct Api {
        port: u16,
        stop: Arc<AtomicBool>,
        snapshots: Arc<Mutex<HashMap<String, Value>>>,
        calls: Arc<Mutex<Vec<String>>>,
        worker: Option<std::thread::JoinHandle<()>>,
    }
    impl Api {
        fn start() -> Self {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let port = listener.local_addr().unwrap().port();
            let stop = Arc::new(AtomicBool::new(false));
            let snapshots: Arc<Mutex<HashMap<String, Value>>> = Default::default();
            let calls: Arc<Mutex<Vec<String>>> = Default::default();
            let worker = {
                let (stop, snapshots, calls) = (stop.clone(), snapshots.clone(), calls.clone());
                std::thread::spawn(move || {
                    while !stop.load(Ordering::Acquire) {
                        let Ok((mut stream, _)) = listener.accept() else {
                            std::thread::sleep(Duration::from_millis(2));
                            continue;
                        };
                        let _ = stream.set_nonblocking(false);
                        let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
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
                        let body: Value =
                            serde_json::from_slice(&buf[head_end..]).unwrap_or(Value::Null);
                        let method = head
                            .split_whitespace()
                            .nth(1)
                            .unwrap_or("/")
                            .trim_start_matches("/api/")
                            .to_string();
                        let payload = if method == "acp_get_session_snapshot" {
                            calls.lock().unwrap().push(method);
                            snapshots
                                .lock()
                                .unwrap()
                                .get(body["connectionId"].as_str().unwrap_or(""))
                                .cloned()
                                .unwrap_or(Value::Null)
                        } else {
                            Value::Null
                        };
                        let payload = payload.to_string();
                        let _ = write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",payload.len());
                        let _ = stream.flush();
                    }
                })
            };
            Self {
                port,
                stop,
                snapshots,
                calls,
                worker: Some(worker),
            }
        }
        fn set(&self, conn: &str, snapshot: Value) {
            self.snapshots.lock().unwrap().insert(conn.into(), snapshot);
        }
        fn calls(&self) -> usize {
            self.calls.lock().unwrap().len()
        }
    }
    impl Drop for Api {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Release);
            if let Some(worker) = self.worker.take() {
                let _ = worker.join();
            }
        }
    }

    /// Codeg home whose database holds a parent task and three delegated
    /// children: a delegate, a regular child of the parent and a `kind=delegate`
    /// task without a parent row.
    fn collector(home: &Home, port: u16) -> Collector {
        let mut s = settings::defaults();
        for id in settings::SOURCES {
            s["sources"][id]["enabled"] = json!(id == "codeg");
        }
        crate::atomic_json(&home.0.join(".workbuddy-shift-companion/settings.json"), &s).unwrap();
        let dir = home.0.join("Library/Application Support/app.codeg");
        std::fs::create_dir_all(&dir).unwrap();
        let db = rusqlite::Connection::open(dir.join("codeg.db")).unwrap();
        db.execute_batch(
            "CREATE TABLE conversation(id INTEGER,title TEXT,agent_type TEXT,external_id TEXT,folder_id INTEGER,status TEXT,parent_id INTEGER,kind TEXT);
             CREATE TABLE folder(id INTEGER,path TEXT);
             INSERT INTO conversation VALUES(214,'Build feature','codex','thr-native',1,'in_progress',NULL,'regular');
             INSERT INTO conversation VALUES(215,'Child task\n<recommended_plugins> Here','codex','thr-child',1,'in_progress',214,'delegate');
             INSERT INTO conversation VALUES(217,'Second child','codex','thr-second',1,'in_progress',214,'regular');
             INSERT INTO conversation VALUES(218,'Root child','codex','thr-root',1,'in_progress',NULL,'delegate');
             INSERT INTO folder VALUES(1,'/project/test');",
        )
        .unwrap();
        db.execute(
            "INSERT INTO conversation VALUES(219,?1,'codex','thr-long',1,'in_progress',214,'delegate')",
            ["word ".repeat(30).trim()],
        )
        .unwrap();
        drop(db);
        let mut c = Collector::new(home.0.clone()).unwrap();
        c.codeg.auth = Some((port, "test-token".into()));
        c.set_codeg_stream_sink(Arc::new(|_| {}));
        c
    }

    /// The snapshot a waiting child reports: its conversation, still prompting,
    /// with one permission request.
    fn pending(sid: i64, request: &str) -> Value {
        json!({"conversation_id":sid,"status":"prompting","event_seq":5,"pending_permission":{"request_id":request,"tool_call":{"title":"Allow shell?"},"options":[{"name":"允许","kind":"allow_once"}]}})
    }

    /// A child runs invisibly, surfaces exactly one temporary card while it
    /// waits for the user and gives that card up with the answer.
    #[test]
    fn codeg_child_requests_surface_a_temporary_card_until_answered() {
        let home = Home::new();
        let api = Api::start();
        api.set("connection-1", json!({"conversation_id":214,"external_id":"thr-native","folder_id":1,"status":"prompting"}));
        api.set("child", json!({"conversation_id":215,"external_id":"thr-child","folder_id":1,"status":"prompting"}));
        let mut c = collector(&home, api.port);
        assert!(c.ingest_hook(&json!({"source":"codeg","connection_id":"connection-1","event":"user_prompt_sent","body":"Build"})));
        let parent = c.hub.sessions["codeg:214"].clone();
        // Running children never reach the rail, whatever their metadata says.
        assert!(c.ingest_hook(&json!({"source":"codeg","connection_id":"child","event":"user_prompt_sent","body":"Child"})));
        assert!(!c.hub.sessions.contains_key("codeg:215"));
        let calls = api.calls();
        api.set("child", Value::Null);
        assert!(c.ingest_hook(
            &json!({"source":"codeg","connection_id":"child","event":"user_prompt_sent"})
        ));
        assert_eq!(
            api.calls(),
            calls,
            "a known child never queries the API again"
        );
        // The request surfaces the card, its parent and the child's stream.
        api.set("child", pending(215, "p1"));
        assert!(c.ingest_hook(&json!({"source":"codeg","connection_id":"child","event":"permission_request","body":"Task"})));
        let child = c.hub.sessions["codeg:215"].clone();
        assert_eq!(child["status"], "wait");
        assert_eq!(child["subagent"], true);
        assert_eq!(child["parentTitle"], "Build feature");
        assert_eq!(child["title"], "Child task <recommended_plugins> Here");
        assert_eq!(child["cwd"], "/project/test");
        assert_eq!(child["pending"][0]["id"], "p1");
        assert_eq!(child["pending"][0]["text"], "Allow shell?");
        assert!(c.hub.snapshot()["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["id"] == "codeg:215"));
        assert!(
            c.codeg.streams.contains_key("child"),
            "the wait subscribes the child stream"
        );
        assert_eq!(
            c.hub.sessions["codeg:214"], parent,
            "the child never changes its parent"
        );
        // The authoritative answer releases both.
        let subscription = c.codeg.streams["child"].subscription.clone();
        assert!(c.ingest_codeg_stream(&json!({"type":"event","connection_id":"child","subscription_id":subscription,"envelope":{"seq":6,"type":"permission_resolved","connection_id":"child","request_id":"p1"}})));
        assert!(!c.hub.sessions.contains_key("codeg:215"));
        assert!(!c.codeg.streams.contains_key("child"));
        assert_eq!(c.hub.sessions["codeg:214"], parent);
        // A late duplicate of the answered request never revives the card.
        assert!(c.ingest_hook(&json!({"source":"codeg","connection_id":"child","event":"permission_request","body":"Task"})));
        assert!(!c.hub.sessions.contains_key("codeg:215"));
        // A request whose snapshot cannot be read names no request id: nothing is
        // shown, because no answer could ever be matched against it.
        api.set("child", Value::Null);
        assert!(c.ingest_hook(&json!({"source":"codeg","connection_id":"child","event":"permission_request","body":"Task"})));
        assert!(!c.hub.sessions.contains_key("codeg:215"));
        assert!(!c.codeg.streams.contains_key("child"));
        // A new request from the same child surfaces again.
        api.set("child", pending(215, "p2"));
        assert!(c.ingest_hook(&json!({"source":"codeg","connection_id":"child","event":"permission_request","body":"Task"})));
        assert_eq!(c.hub.sessions["codeg:215"]["pending"][0]["id"], "p2");
        assert!(c.codeg.streams.contains_key("child"));
    }

    /// The end of a child's turn releases the card even without an answer.
    #[test]
    fn codeg_child_turn_completion_releases_the_card() {
        let home = Home::new();
        let api = Api::start();
        api.set("child", pending(215, "p1"));
        let mut c = collector(&home, api.port);
        assert!(c.ingest_hook(&json!({"source":"codeg","connection_id":"child","event":"permission_request","body":"Task"})));
        assert!(c.hub.sessions.contains_key("codeg:215"));
        assert!(c.ingest_hook(
            &json!({"source":"codeg","connection_id":"child","event":"turn_complete"})
        ));
        assert!(
            !c.hub.sessions.contains_key("codeg:215"),
            "a child never keeps a done card"
        );
        assert!(!c.codeg.streams.contains_key("child"));
        // The finished request cannot come back through a delayed webhook.
        assert!(c.ingest_hook(&json!({"source":"codeg","connection_id":"child","event":"permission_request","body":"Task"})));
        assert!(!c.hub.sessions.contains_key("codeg:215"));
    }

    /// An answer that lands while the webhook is still in flight releases the
    /// card on the stream snapshot instead of leaving a running child behind.
    #[test]
    fn codeg_child_card_released_by_an_idle_stream_snapshot() {
        let home = Home::new();
        let api = Api::start();
        api.set("child", pending(215, "p1"));
        let mut c = collector(&home, api.port);
        assert!(c.ingest_hook(&json!({"source":"codeg","connection_id":"child","event":"permission_request","body":"Task"})));
        let subscription = c.codeg.streams["child"].subscription.clone();
        assert!(c.ingest_codeg_stream(&json!({"type":"snapshot","connection_id":"child","subscription_id":subscription,"event_seq":7,"snapshot":{"conversation_id":215,"status":"prompting"}})));
        assert!(!c.hub.sessions.contains_key("codeg:215"));
        assert!(!c.codeg.streams.contains_key("child"));
        // A snapshot for another conversation settles nothing here.
        api.set("child", pending(215, "p2"));
        assert!(c.ingest_hook(&json!({"source":"codeg","connection_id":"child","event":"permission_request","body":"Task"})));
        assert!(c.hub.sessions.contains_key("codeg:215"));
        let subscription = c.codeg.streams["child"].subscription.clone();
        assert!(c.ingest_codeg_stream(&json!({"type":"snapshot","connection_id":"child","subscription_id":subscription,"event_seq":8,"snapshot":{"conversation_id":999,"status":"prompting"}})));
        assert!(c.hub.sessions.contains_key("codeg:215"));
    }

    /// A cancelled child emits no signal at all, so the card has a time bound.
    #[test]
    fn codeg_child_cards_expire_without_an_answer() {
        let home = Home::new();
        let api = Api::start();
        api.set("child", pending(215, "p1"));
        let mut c = collector(&home, api.port);
        assert!(c.ingest_hook(&json!({"source":"codeg","connection_id":"child","event":"permission_request","body":"Task"})));
        c.codeg.children.get_mut("child").unwrap().started_at = now() - CODEG_CHILD_TTL - 1;
        c.maintain_codeg_webhook().unwrap();
        assert!(!c.hub.sessions.contains_key("codeg:215"));
        assert!(!c.codeg.streams.contains_key("child"));
        assert!(
            c.codeg.children.contains_key("child"),
            "the child stays known"
        );
        // A fresh request still surfaces after the sweep.
        api.set("child", pending(215, "p2"));
        assert!(c.ingest_hook(&json!({"source":"codeg","connection_id":"child","event":"permission_request","body":"Task"})));
        assert_eq!(c.hub.sessions["codeg:215"]["pending"][0]["id"], "p2");
        assert!(c.codeg.streams.contains_key("child"));
    }

    /// Disabling the source drops every temporary card with its stream.
    #[test]
    fn codeg_child_cards_are_released_when_the_source_is_disabled() {
        let home = Home::new();
        let api = Api::start();
        api.set("child", pending(215, "p1"));
        let mut c = collector(&home, api.port);
        assert!(c.ingest_hook(&json!({"source":"codeg","connection_id":"child","event":"permission_request","body":"Task"})));
        c.settings["sources"]["codeg"]["enabled"] = json!(false);
        c.maintain_codeg_webhook().unwrap();
        assert!(!c.hub.sessions.contains_key("codeg:215"));
        assert!(!c.codeg.streams.contains_key("child"));
        assert!(c.codeg.children.is_empty());
    }

    /// A child that finishes silently has no conversation to resolve: the
    /// provisional session that path used to seed was a `done` ghost on a task
    /// that never needed the user.
    #[test]
    fn codeg_silent_child_completion_never_seeds_a_session() {
        let home = Home::new();
        let api = Api::start();
        let mut c = collector(&home, api.port);
        assert!(c.ingest_hook(&json!({"source":"codeg","connection_id":"child","event":"turn_complete","body":"CodeBuddy session completed"})));
        assert!(c.hub.sessions.is_empty());
        assert!(c.hub.snapshot()["sessions"].as_array().unwrap().is_empty());
        assert_eq!(api.calls(), 1, "the event was still resolved once");
        // An existing provisional session still ends on its own completion.
        assert!(c.ingest_hook(&json!({"source":"codeg","connection_id":"gone","event":"user_prompt_sent","body":"Task"})));
        assert_eq!(c.hub.sessions["codeg:connection:gone"]["status"], "running");
        assert!(c.ingest_hook(
            &json!({"source":"codeg","connection_id":"gone","event":"turn_complete"})
        ));
        assert_eq!(c.hub.sessions["codeg:connection:gone"]["status"], "done");
    }

    /// Every child shape the database can report is treated as a child, and a
    /// long prompt fragment is collapsed and capped before the rail sees it.
    #[test]
    fn codeg_child_metadata_covers_every_delegation_shape() {
        let home = Home::new();
        let api = Api::start();
        let c = collector(&home, api.port);
        let meta = |sid: i64| c.codeg_metadata(&sid.to_string()).unwrap();
        for id in [215, 217, 218, 219] {
            assert_eq!(meta(id)["isSubagent"], true, "conversation {id}");
        }
        assert_eq!(meta(214)["isSubagent"], false);
        assert_eq!(meta(215)["title"], "Child task\n<recommended_plugins> Here");
        assert_eq!(meta(215)["parentTitle"], "Build feature");
        assert_eq!(meta(219)["parentTitle"], "Build feature");
        assert_eq!(codeg_title(&text(&meta(219)["title"])).chars().count(), 80);
        assert_eq!(
            meta(218).get("parentTitle"),
            None,
            "a parentless delegate carries no parent title"
        );
    }

    /// The Node twin parses a snapshot and a stream envelope with one function,
    /// so the two Rust parsers must agree too: a tool call carrying only a `name`
    /// and an option carrying only a `label` may not read differently depending
    /// on which path surfaced the card.
    #[test]
    fn codeg_child_permission_ask_matches_the_stream_twin() {
        let snap = json!({"pending_permission":{"request_id":"p1","tool_call":{"name":"Bash"},"options":[{"label":"允许","kind":"allow_once"}]}});
        let ask = codeg_ask(&snap);
        assert_eq!(
            ask,
            stream_ask("pending_permission", &snap["pending_permission"])
        );
        assert_eq!(question(&ask), "Bash");
        assert_eq!(questions(&ask)[0]["options"][0]["label"], "允许");
    }
}
