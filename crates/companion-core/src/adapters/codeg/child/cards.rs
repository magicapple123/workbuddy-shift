//! Codeg 委派子会话临时卡片与等待。

use super::*;

impl Collector {
    /// A child shows a temporary card only while a request waits for the user.
    /// The read-only stream is the only source of the answer and a detached
    /// connection can never be attached again, so it is subscribed in this same
    /// pass — never after the fact.
    pub(crate) fn codeg_child_wait(
        &mut self,
        conn: &str,
        sid: &str,
        snap: &Value,
        meta: &mut Value,
        p: &Value,
    ) {
        let ask = codeg_ask(snap);
        let Some(call) = request_id(&ask).map(str::to_owned) else {
            // Without the request id in the snapshot no answer could ever be
            // matched, so nothing is shown at all.
            return;
        };
        if self.codeg.children.get(conn).is_some_and(|child| {
            child.resolved.iter().any(|id| *id == call)
                || child.open.as_deref() == Some(call.as_str())
        }) {
            // Already answered, or already on screen: the stream owns the state
            // and a delayed webhook must not rewind it.
            return;
        }
        meta["title"] = json!(codeg_title(&text(&meta["title"])));
        meta["subagent"] = json!(true);
        let ts = now();
        self.codeg.sequence += 1;
        let seq = self.codeg.sequence;
        let key = format!("codeg:{sid}");
        let start_title = (!self.hub.sessions.contains_key(&key)).then(|| text(&p["body"]));
        let base = self.codeg_seed_session(sid, snap, meta, ts, seq, start_title.as_deref());
        let pending = self.hub.sessions[&key]["pending"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        for item in pending {
            self.hub.ingest(merge(
                base.clone(),
                json!({"type":"resolve","callId":item["id"]}),
            ));
        }
        self.hub.ingest(merge(
            base,
            json!({"type":"wait","callId":call,"tool":if p["event"] == "question_request" {"ask"} else {"permission"},"text":question(&ask),"questions":questions(&ask)}),
        ));
        if let Some(child) = self.codeg.children.get_mut(conn) {
            child.open = Some(call);
            child.started_at = ts;
        }
        self.attach_codeg_stream(conn, sid);
        self.codeg_sync_stream_cursor(conn, snap);
        self.hub.health(
            "codeg",
            "ok",
            "已收到 Codeg Webhook；确认状态通过实时事件同步",
        );
    }
}
