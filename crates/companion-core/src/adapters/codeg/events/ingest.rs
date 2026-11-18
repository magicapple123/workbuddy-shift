//! Codeg webhook ingest：权限/提问/完成事件。

use super::*;

impl Collector {
    pub fn ingest_codeg_hook(&mut self, p: &Value) -> bool {
        if self.settings["sources"]["codeg"]["enabled"] != true
            || !self.integration_automatic("codeg")
            || p["source"] != "codeg"
        {
            return false;
        }
        let event = text(&p["event"]);
        let conn = text(&p["connection_id"]);
        if !CODEG_EVENTS.contains(&event.as_str()) || conn.trim().is_empty() || conn.len() > 256 {
            return false;
        }
        let request = matches!(event.as_str(), "question_request" | "permission_request");
        let known_sid = self
            .codeg
            .children
            .get(&conn)
            .map(|child| child.sid.clone());
        // A known child costs nothing until it waits for the user: no snapshot,
        // no metadata, no session. Only the end of its turn releases the card.
        if known_sid.is_some() && !request {
            let open = self
                .codeg
                .children
                .get(&conn)
                .is_some_and(|child| child.open.is_some());
            if open && matches!(event.as_str(), "turn_complete" | "error") {
                self.codeg_release_child(&conn, None);
            }
            return true;
        }
        if self.codeg.auth.is_none() {
            self.codeg.auth = self.codeg_credentials().ok();
        }
        let snap = self
            .codeg
            .auth
            .as_ref()
            .and_then(|a| post(a, "acp_get_session_snapshot", json!({"connectionId":conn})).ok())
            .unwrap_or(Value::Null);
        let provisional = format!("connection:{conn}");
        let sid = if !snap["conversation_id"].is_null() {
            text(&snap["conversation_id"])
        } else if let Some(sid) = known_sid.clone() {
            sid
        } else {
            self.codeg
                .connections
                .get(&conn)
                .cloned()
                .unwrap_or(provisional.clone())
        };
        // A silently finished child keeps no conversation id; seeding a session
        // here would leave a `done` ghost for a task that never needed a user.
        if sid == provisional
            && matches!(event.as_str(), "turn_complete" | "error")
            && !self.hub.sessions.contains_key(&format!("codeg:{sid}"))
        {
            return true;
        }
        if sid != provisional {
            self.codeg.connections.insert(conn.clone(), sid.clone());
            self.hub.sessions.remove(&format!("codeg:{provisional}"));
        }
        if self.codeg.connections.len() > 512 {
            if let Some(k) = self.codeg.connections.keys().next().cloned() {
                self.codeg.connections.remove(&k);
            }
        }
        let mut meta = if sid != provisional {
            self.codeg_metadata(&sid).unwrap_or(json!({}))
        } else {
            json!({})
        };
        if meta["isSubagent"] == true || known_sid.is_some() {
            if text(&meta["agentType"]).eq_ignore_ascii_case("codex") {
                let external_id = if text(&snap["external_id"]).is_empty() {
                    text(&meta["externalId"])
                } else {
                    text(&snap["external_id"])
                };
                self.hub.hide_codeg_child_codex(&external_id);
            }
            self.codeg.children.entry(conn.clone()).or_default().sid = sid.clone();
            if !request {
                // Keep known children ignored during API/database outages; only a
                // waiting request buys a child a temporary card.
                if self
                    .codeg
                    .children
                    .get(&conn)
                    .is_some_and(|child| child.open.is_some())
                {
                    self.codeg_release_child(&conn, None);
                }
                self.codeg.streams.remove(&conn);
                self.hub.sessions.remove(&format!("codeg:{provisional}"));
                self.hub.sessions.remove(&format!("codeg:{sid}"));
                return true;
            }
            self.codeg_child_wait(&conn, &sid, &snap, &mut meta, p);
            return true;
        }
        if self.codeg.streams.get(&conn).is_some_and(|s| s.sid != sid) {
            self.codeg.streams.remove(&conn);
        }
        // HTTP snapshots and webhook deliveries can lag behind the live stream.
        if let Some(stream) = self.codeg.streams.get(&conn) {
            let cursor = stream.seq.load(std::sync::atomic::Ordering::Acquire);
            if cursor != u64::MAX {
                if snap["event_seq"].as_u64().is_some_and(|seq| seq < cursor) {
                    return true;
                }
                if event == "user_prompt_sent"
                    && snap["status"]
                        .as_str()
                        .is_some_and(|status| status != "prompting")
                    && self
                        .hub
                        .sessions
                        .get(&format!("codeg:{sid}"))
                        .is_some_and(|session| crate::hub::terminal(&text(&session["status"])))
                {
                    return true;
                }
            }
        }
        if matches!(event.as_str(), "question_request" | "permission_request")
            && self
                .codeg
                .streams
                .get(&conn)
                .is_some_and(|s| s.seq.load(std::sync::atomic::Ordering::Acquire) != u64::MAX)
        {
            return true;
        }
        if event == "turn_complete" && snap["status"] == "prompting" {
            return true;
        }
        let key = format!("codeg:{sid}");
        if self.closed_monitor_sessions.contains(&key) {
            if matches!(event.as_str(), "turn_complete" | "error") {
                return true;
            }
            self.closed_monitor_sessions.remove(&key);
        }
        let ts = now();
        self.codeg.sequence += 1;
        let seq = self.codeg.sequence;
        let k = format!("codeg:{sid}");
        let start_title = (event == "user_prompt_sent" || !self.hub.sessions.contains_key(&k))
            .then(|| text(&p["body"]));
        let base = self.codeg_seed_session(&sid, &snap, &meta, ts, seq, start_title.as_deref());
        if matches!(event.as_str(), "question_request" | "permission_request")
            && !crate::hub::terminal(&text(&self.hub.sessions[&k]["status"]))
        {
            let ask = codeg_ask(&snap);
            let pending = self.hub.sessions[&k]["pending"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            for item in pending {
                self.hub.ingest(merge(
                    base.clone(),
                    json!({"type":"resolve","callId":item["id"]}),
                ));
            }
            let fields = p["fields"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|f| text(&f["value"]))
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join("\n");
            let message = if !ask.is_null() {
                question(&ask)
            } else if !fields.is_empty() {
                fields
            } else {
                text(&p["body"])
            };
            let call = request_id(&ask)
                .map(str::to_owned)
                .unwrap_or(format!("codeg:{conn}:{seq}"));
            self.hub.ingest(merge(base,json!({"type":"wait","callId":call,"tool":if event=="question_request"{"ask"}else{"permission"},"text":message,"questions":questions(&ask)})));
        } else if matches!(event.as_str(), "turn_complete" | "error") {
            self.hub.ingest(merge(
                base,
                json!({"type":"end","status":if event=="error"{"error"}else{"done"}}),
            ));
        }
        if snap["status"] == "prompting" {
            self.reconcile_codeg_snapshot(&conn, &sid, &snap, false);
        }
        self.attach_codeg_stream(&conn, &sid);
        self.codeg_sync_stream_cursor(&conn, &snap);
        self.hub.health(
            "codeg",
            "ok",
            "已收到 Codeg Webhook；确认状态通过实时事件同步",
        );
        true
    }
}
