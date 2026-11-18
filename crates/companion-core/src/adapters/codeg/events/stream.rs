//! Codeg 确认流：注册、游标与发送。

use super::*;

pub(crate) fn stream_ask(kind: &str, value: &Value) -> Value {
    match kind {
        "pending_question" => value.clone(),
        "pending_plan_approval" => {
            json!({"approval_id":value["approval_id"],"questions":[{"question":value["plan_markdown"],"options":[{"label":"批准"},{"label":"拒绝"}]}]})
        }
        _ => {
            json!({"request_id":value["request_id"],"questions":[{"question":value["tool_call"]["title"].as_str().or(value["tool_call"]["name"].as_str()).unwrap_or("需要你的许可"),"options":value["options"].as_array().into_iter().flatten().map(|o| json!({"label":o["name"].as_str().or(o["label"].as_str()).unwrap_or(""),"description":o["kind"]})).collect::<Vec<_>>()}]})
        }
    }
}

pub(crate) fn request_id(value: &Value) -> Option<&str> {
    value["question_id"]
        .as_str()
        .or(value["request_id"].as_str())
        .or(value["approval_id"].as_str())
        .filter(|s| !s.is_empty())
}

impl Collector {
    /// One-shot alignment at startup or integration enablement: enumerate live
    /// connections, then re-seed only the in-flight ones, because a session that
    /// waits for a delegated child emits no events at all. Never called again;
    /// runtime stays purely event-driven.
    pub(crate) fn reconcile_codeg_connections(&mut self) -> Result<(), String> {
        let auth = self
            .codeg
            .auth
            .clone()
            .ok_or_else(|| "Codeg Web Service 不可用".to_string())?;
        let listed = post(&auth, "acp_list_connections", json!({}))?;
        let rows = listed
            .as_array()
            .cloned()
            .ok_or("Codeg 连接列表无效".to_string())?;
        let mut attempted = 0usize;
        let mut failed = 0usize;
        for row in rows.iter().take(CODEG_ALIGN_LIMIT) {
            let conn = text(&row["id"]);
            if conn.trim().is_empty() || conn.len() > 256 {
                continue;
            }
            attempted += 1;
            let snap = match post(
                &auth,
                "acp_get_session_snapshot",
                json!({"connectionId":conn}),
            ) {
                Ok(snap) => snap,
                Err(_) => {
                    failed += 1;
                    continue;
                }
            };
            let sid = text(&snap["conversation_id"]);
            if sid.is_empty() {
                continue;
            }
            let meta = self.codeg_metadata(&sid).unwrap_or(json!({}));
            if meta["isSubagent"] == true {
                if text(&meta["agentType"]).eq_ignore_ascii_case("codex") {
                    let external_id = if text(&snap["external_id"]).is_empty() {
                        text(&meta["externalId"])
                    } else {
                        text(&snap["external_id"])
                    };
                    self.hub.hide_codeg_child_codex(&external_id);
                }
                continue;
            }
            // A webhook may have won the race; never start a round twice or
            // revive the one the hub already finished.
            if self.hub.sessions.contains_key(&format!("codeg:{sid}")) {
                continue;
            }
            let pending = [
                "pending_question",
                "pending_permission",
                "pending_plan_approval",
            ]
            .iter()
            .any(|key| !snap[*key].is_null());
            if snap["status"] != "prompting" && !pending {
                continue;
            }
            self.codeg.connections.insert(conn.clone(), sid.clone());
            let ts = now();
            self.codeg.sequence += 1;
            let seq = self.codeg.sequence;
            self.codeg_seed_session(&sid, &snap, &meta, ts, seq, Some(""));
            self.reconcile_codeg_snapshot(&conn, &sid, &snap, false);
            self.attach_codeg_stream(&conn, &sid);
            self.codeg_sync_stream_cursor(&conn, &snap);
        }
        // One health write: a second note must not erase the first.
        let mut notes = Vec::new();
        if attempted > 0 && failed == attempted {
            notes.push("启动对齐未能读取会话快照");
        }
        if rows.len() > CODEG_ALIGN_LIMIT {
            notes.push("Codeg 连接数超出上限，仅对齐前 64 条");
        }
        if !notes.is_empty() {
            self.hub.health("codeg", "partial", &notes.join("；"));
        }
        Ok(())
    }
}

impl Collector {
    /// Shared by the webhook path and the startup alignment so both emit the
    /// same `start` shape and round id. `start_body` only fills in when the
    /// session metadata has no title.
    pub(crate) fn codeg_seed_session(
        &mut self,
        sid: &str,
        snap: &Value,
        meta: &Value,
        ts: i64,
        seq: u64,
        start_body: Option<&str>,
    ) -> Value {
        let mut base = merge(
            meta.clone(),
            json!({"source":"codeg","sessionId":sid,"ts":ts}),
        );
        if !snap["external_id"].is_null() {
            base["externalId"] = snap["external_id"].clone();
        }
        if !snap["folder_id"].is_null() {
            base["folderId"] = snap["folder_id"].clone();
        }
        if let Some(body) = start_body {
            let title = if text(&base["title"]).is_empty() {
                body.to_string()
            } else {
                text(&base["title"])
            };
            self.hub.ingest(merge(
                base.clone(),
                json!({"type":"start","roundId":format!("hook:{ts}:{seq}"),"title":title}),
            ));
        }
        base
    }
}

impl Collector {
    pub(crate) fn codeg_sync_stream_cursor(&mut self, conn: &str, snap: &Value) {
        if let (Some(seq), Some(stream)) =
            (snap["event_seq"].as_u64(), self.codeg.streams.get(conn))
        {
            let old = stream.seq.load(std::sync::atomic::Ordering::Acquire);
            stream.seq.store(
                if old == u64::MAX { seq } else { old.max(seq) },
                std::sync::atomic::Ordering::Release,
            );
        }
    }
}

impl Collector {
    pub(crate) fn attach_codeg_stream(&mut self, conn: &str, sid: &str) {
        if self.codeg.streams.get(conn).is_some_and(|s| s.sid == sid) {
            return;
        }
        let (Some(auth), Some(sink)) = (self.codeg.auth.clone(), self.codeg.stream_sink.clone())
        else {
            return;
        };
        if self.codeg.streams.len() >= 512 {
            if let Some(key) = self.codeg.streams.keys().next().cloned() {
                self.codeg.streams.remove(&key);
            }
        }
        self.codeg.sequence += 1;
        let subscription = format!("companion-{}-{}", now(), self.codeg.sequence);
        self.codeg.streams.insert(
            conn.into(),
            super::super::codeg_stream::Stream::new(
                conn.into(),
                sid.into(),
                subscription,
                auth,
                sink,
            ),
        );
    }
}

impl Collector {
    pub(crate) fn codeg_send(&mut self, sid: &str, event: Value) {
        self.hub.ingest(merge(
            json!({"source":"codeg","sessionId":sid,"ts":now()}),
            event,
        ));
    }
}
