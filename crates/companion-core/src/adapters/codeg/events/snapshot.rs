//! Codeg 启动快照对齐与实时流 envelope。

use super::*;

/// The request a snapshot is waiting on, in the precedence every path shares.
/// It parses each pending field with the same helper the stream frames use, so a
/// snapshot and an envelope carrying one request can never describe it
/// differently — the Node twin parses both shapes in one function for the same
/// reason.
pub(crate) fn codeg_ask(snap: &Value) -> Value {
    if !snap["pending_question"].is_null() {
        stream_ask("pending_question", &snap["pending_question"])
    } else if !snap["pending_plan_approval"].is_null() {
        stream_ask("pending_plan_approval", &snap["pending_plan_approval"])
    } else if !snap["pending_permission"].is_null() {
        stream_ask("pending_permission", &snap["pending_permission"])
    } else {
        Value::Null
    }
}

/// Whether a snapshot still carries any pending request.
pub(crate) fn codeg_pending(snap: &Value) -> bool {
    [
        "pending_question",
        "pending_permission",
        "pending_plan_approval",
    ]
    .iter()
    .any(|key| !snap[*key].is_null())
}

/// A child title is a raw prompt fragment: collapse its whitespace so the rail
/// keeps one line, and cap what it has to render.
pub(crate) fn codeg_title(raw: &str) -> String {
    raw.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(CODEG_CHILD_TITLE)
        .collect()
}

impl Collector {
    pub(crate) fn reconcile_codeg_snapshot(
        &mut self,
        conn: &str,
        sid: &str,
        snap: &Value,
        authoritative: bool,
    ) {
        // A child card lives exactly as long as the stream still shows a pending
        // request. An authoritative snapshot without one releases it, which
        // covers an answer that landed while the webhook was still in flight.
        if authoritative
            && self
                .codeg
                .children
                .get(conn)
                .is_some_and(|child| child.open.is_some())
            && (snap["conversation_id"].is_null() || text(&snap["conversation_id"]) == sid)
            && !codeg_pending(snap)
        {
            self.codeg_release_child(conn, None);
            return;
        }
        let Some(session) = self.hub.sessions.get(&format!("codeg:{sid}")) else {
            return;
        };
        if crate::hub::terminal(&text(&session["status"])) {
            return;
        }
        let asks: Vec<_> = [
            "pending_question",
            "pending_permission",
            "pending_plan_approval",
        ]
        .iter()
        .filter_map(|kind| {
            let ask = stream_ask(kind, &snap[*kind]);
            request_id(&ask).map(|id| {
                (
                    id.to_string(),
                    ask.clone(),
                    if *kind == "pending_question" {
                        "ask"
                    } else {
                        "permission"
                    },
                )
            })
        })
        .collect();
        if authoritative
            && snap["status"] == "connected"
            && [
                "pending_question",
                "pending_permission",
                "pending_plan_approval",
            ]
            .iter()
            .all(|key| snap[*key].is_null())
        {
            self.codeg_send(sid, json!({"type":"end","status":"done"}));
            return;
        }
        let pending = session["pending"].as_array().cloned().unwrap_or_default();
        if snap["status"] == "prompting" {
            for p in pending {
                if !asks.iter().any(|(id, _, _)| p["id"] == *id) {
                    self.codeg_send(sid, json!({"type":"resolve","callId":p["id"]}));
                }
            }
        }
        for (id, ask, tool) in asks {
            self.codeg_send(sid,json!({"type":"wait","callId":id,"tool":tool,"text":question(&ask),"questions":questions(&ask)}));
        }
    }
}

impl Collector {
    pub(crate) fn apply_codeg_envelope(&mut self, conn: &str, sid: &str, envelope: &Value) {
        let kind = text(&envelope["type"]);
        let resolved = matches!(
            kind.as_str(),
            "question_resolved" | "permission_resolved" | "plan_approval_resolved"
        );
        let live = self
            .hub
            .sessions
            .get(&format!("codeg:{sid}"))
            .is_some_and(|session| !crate::hub::terminal(&text(&session["status"])));
        // A child gives its card up on the authoritative answer or on the end of
        // its turn, whether or not the hub still holds the session.
        if (resolved || kind == "turn_complete") && self.codeg.children.contains_key(conn) {
            let answered = if resolved { request_id(envelope) } else { None };
            if live {
                if let Some(id) = answered {
                    self.codeg_send(sid, json!({"type":"resolve","callId":id}));
                } else {
                    self.codeg_send(sid, json!({"type":"end","status":"done"}));
                }
            }
            self.codeg_release_child(conn, answered);
            return;
        }
        if !live {
            return;
        }
        if resolved {
            if let Some(id) = request_id(envelope) {
                self.codeg_send(sid, json!({"type":"resolve","callId":id}));
            }
        } else if matches!(
            kind.as_str(),
            "question_request" | "permission_request" | "plan_approval_request"
        ) {
            let key = match kind.as_str() {
                "question_request" => "pending_question",
                "permission_request" => "pending_permission",
                _ => "pending_plan_approval",
            };
            let ask = stream_ask(key, envelope);
            if let Some(id) = request_id(envelope) {
                self.codeg_send(sid,json!({"type":"wait","callId":id,"tool":if key=="pending_question"{"ask"}else{"permission"},"text":question(&ask),"questions":questions(&ask)}));
            }
        } else if kind == "turn_complete" {
            self.codeg_send(sid, json!({"type":"end","status":"done"}));
        }
    }
}

impl Collector {
    pub fn ingest_codeg_stream(&mut self, frame: &Value) -> bool {
        use std::sync::atomic::Ordering;
        if self.settings["sources"]["codeg"]["enabled"] != true
            || !self.integration_automatic("codeg")
        {
            return false;
        }
        let conn = text(&frame["connection_id"]);
        let Some(stream) = self.codeg.streams.get(&conn) else {
            return false;
        };
        if frame["subscription_id"] != stream.subscription {
            return false;
        }
        let sid = stream.sid.clone();
        let cursor = stream.seq.clone();
        let mut changed = false;
        match frame["type"].as_str() {
            Some("snapshot") => {
                if let Some(seq) = frame["event_seq"].as_u64() {
                    let old = cursor.load(Ordering::Acquire);
                    if old != u64::MAX && seq < old {
                        return false;
                    }
                    changed = true;
                    cursor.store(seq, Ordering::Release);
                    self.reconcile_codeg_snapshot(&conn, &sid, &frame["snapshot"], true);
                }
            }
            Some("event" | "replay") => {
                let envelopes = if frame["type"] == "event" {
                    vec![frame["envelope"].clone()]
                } else {
                    frame["events"].as_array().cloned().unwrap_or_default()
                };
                for envelope in envelopes {
                    let Some(seq) = envelope["seq"].as_u64() else {
                        continue;
                    };
                    let old = cursor.load(Ordering::Acquire);
                    if envelope["connection_id"] != conn || (old != u64::MAX && seq <= old) {
                        continue;
                    }
                    changed |= matches!(
                        envelope["type"].as_str(),
                        Some(
                            "question_request"
                                | "permission_request"
                                | "plan_approval_request"
                                | "question_resolved"
                                | "permission_resolved"
                                | "plan_approval_resolved"
                                | "turn_complete"
                        )
                    );
                    cursor.store(seq, Ordering::Release);
                    self.apply_codeg_envelope(&conn, &sid, &envelope);
                }
                if frame["type"] == "replay" {
                    if let Some(high) = frame["high_water_seq"].as_u64() {
                        let old = cursor.load(Ordering::Acquire);
                        cursor.store(
                            if old == u64::MAX { high } else { old.max(high) },
                            Ordering::Release,
                        );
                    }
                }
            }
            Some("detached") => {
                self.codeg.streams.remove(&conn);
            }
            _ => return false,
        }
        changed
    }
}
