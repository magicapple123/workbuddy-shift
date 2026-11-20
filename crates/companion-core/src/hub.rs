use crate::{now, question_tool, text};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashSet, VecDeque};
pub const STALE: i64 = 45 * 60 * 1000;
/// Optional waits stop blocking after this window, matching the host app's own
/// 60-second no-interaction auto-resolution.
pub const OPTIONAL_TTL_MS: i64 = 60_000;
pub const KIND_PERMISSION: &str = "permission";
pub const KIND_QUESTION: &str = "question";
/// Anti-false-positive window for a tool permission gate (`R5-09`).
///
/// The client's own auto-approval runs *after* `PermissionRequest`, so an
/// auto-approved tool signals a gate and then resolves it milliseconds later.
/// A permission wait is therefore stored with `graceUntil = ts + PERMISSION_GRACE_MS`
/// and stays invisible until the window elapses; a `resolve` inside it removes
/// the item, so nothing is ever shown or notified for that call.
pub const PERMISSION_GRACE_MS: i64 = 2_500;
pub fn terminal(s: &str) -> bool {
    matches!(s, "done" | "error" | "aborted")
}
/// Which kind of attention one `wait` event asks for.
///
/// Producers that know the difference stamp `kind`; the rest are inferred so a
/// stored snapshot from an older runtime still renders correctly. Everything
/// that carries a question payload is a question; the remaining waits (a
/// sandbox approval log line, an unnamed gate) are tool permissions — a gate
/// with no question is exactly what the user has to authorise.
pub fn pending_kind(ev: &Value) -> &'static str {
    match text(&ev["kind"]).as_str() {
        KIND_PERMISSION => return KIND_PERMISSION,
        KIND_QUESTION => return KIND_QUESTION,
        _ => {}
    }
    let tool = text(&ev["tool"]);
    if tool == "permission" || tool == "notification" {
        return KIND_PERMISSION;
    }
    if tool == "ask"
        || question_tool(&tool)
        || ev["questions"].as_array().is_some_and(|q| !q.is_empty())
    {
        return KIND_QUESTION;
    }
    KIND_PERMISSION
}
#[derive(Default)]
pub struct Hub {
    pub sessions: BTreeMap<String, Value>,
    pub hidden_codeg_codex_ids: HashSet<String>,
    pub events: VecDeque<Value>,
    pub sources: BTreeMap<String, Value>,
    pub ready: bool,
    pub started: i64,
}
impl Hub {
    pub fn hide_codeg_child_codex(&mut self, external_id: &str) {
        let id = external_id.strip_prefix("thr_").unwrap_or(external_id);
        if !id.is_empty() {
            self.hidden_codeg_codex_ids.insert(id.to_owned());
        }
    }
    pub fn new() -> Self {
        Self {
            started: now(),
            ..Self::default()
        }
    }
    pub fn health(&mut self, source: &str, state: &str, detail: &str) {
        self.sources.insert(
            source.into(),
            json!({"state":state,"detail":detail,"checkedAt":now()}),
        );
    }
    pub fn ingest(&mut self, ev: Value) {
        let source = text(&ev["source"]);
        let sid = text(&ev["sessionId"]);
        if source.is_empty() || sid.is_empty() {
            return;
        }
        let id = format!("{source}:{sid}");
        let ts = ev["ts"].as_i64().unwrap_or_else(now);
        let ty = text(&ev["type"]);
        let round = text(&ev["roundId"]);
        let s=self.sessions.entry(id.clone()).or_insert_with(||json!({"id":id,"source":source,"sessionId":sid,"cwd":"","title":"","roundId":if round.is_empty(){format!("observed:{ts}")}else{round.clone()},"startedAt":ts,"updatedAt":0,"status":"unknown","steps":[],"pending":[],"tokens":null,"endedAt":null}));
        for key in [
            "cwd",
            "title",
            "folderId",
            "agentType",
            "externalId",
            "webPort",
            "hostKind",
            "sourceLabel",
            "parentTitle",
            "subagent",
        ] {
            if !ev[key].is_null() && !text(&ev[key]).is_empty() {
                s[key] = if key == "title" {
                    json!(text(&ev[key]).chars().take(240).collect::<String>())
                } else if key == "sourceLabel" {
                    json!(text(&ev[key]).chars().take(60).collect::<String>())
                } else if key == "parentTitle" {
                    json!(text(&ev[key]).chars().take(80).collect::<String>())
                } else {
                    ev[key].clone()
                };
            }
        }
        if ty == "meta" {
            if !round.is_empty() && text(&s["roundId"]).starts_with("observed:") {
                s["roundId"] = json!(round);
            }
            return;
        }
        // Anonymous hook approvals have no matching tool result id. Reconcile
        // against model continuation, including transcript replay after restore.
        if source == "codex" && ty == "activity" {
            s["pending"].as_array_mut().unwrap().retain(|p| {
                !(text(&p["id"]).starts_with("perm:") && p["ts"].as_i64().unwrap_or(i64::MAX) < ts)
            });
            if s["status"] == "wait" && s["pending"].as_array().unwrap().is_empty() {
                s["status"] = json!("running");
            }
        }
        let same_round_request = source == "codex"
            && !round.is_empty()
            && round == text(&s["roundId"])
            && matches!(ty.as_str(), "wait" | "resolve");
        if ts < s["updatedAt"].as_i64().unwrap_or(0) && !same_round_request {
            return;
        }
        if ty == "start" && round != text(&s["roundId"]) {
            s["roundId"] = json!(if round.is_empty() {
                format!("turn:{ts}")
            } else {
                round.clone()
            });
            s["startedAt"] = json!(ts);
            s["endedAt"] = Value::Null;
            s["pending"] = json!([]);
            s["permissionChecks"] = json!([]);
            s["steps"] = json!([]);
            s["status"] = json!("running");
            s["tokens"] = Value::Null;
            if source == "codex" {
                s.as_object_mut().unwrap().remove("recovered");
            }
            if let Some(o) = s.as_object_mut() {
                o.remove("endedBy");
            }
        }
        if !round.is_empty() && text(&s["roundId"]).starts_with("observed:") {
            s["roundId"] = json!(round);
        }
        if !round.is_empty() && ty != "start" && round != text(&s["roundId"]) {
            return;
        }
        s["updatedAt"] = json!(ts.max(s["updatedAt"].as_i64().unwrap_or(0)));
        let mut event = None;
        match ty.as_str() {
            "start" => {
                s["status"] = json!(if s["pending"].as_array().unwrap().is_empty() {
                    "running"
                } else {
                    "wait"
                })
            }
            "step" if !terminal(&text(&s["status"])) => {
                let a = s["steps"].as_array_mut().unwrap();
                if !a.iter().any(|x| x["id"] == ev["eventId"]) {
                    a.push(json!({"id":ev["eventId"],"ts":ts,"label":text(&ev["label"]).chars().take(200).collect::<String>()}));
                }
                if a.len() > 20 {
                    a.remove(0);
                }
                s["status"] = json!(if s["pending"].as_array().unwrap().is_empty() {
                    "running"
                } else {
                    "wait"
                });
            }
            "wait" if !terminal(&text(&s["status"])) => {
                let kind = pending_kind(&ev);
                // Grace is opt-in per event: a source that never reported a
                // false positive (Codeg, the Codex question card) keeps its
                // current latency, and only adapters that ask for it are held.
                let grace = ev["graceMs"].as_i64().unwrap_or(0).clamp(0, 60_000);
                let a = s["pending"].as_array_mut().unwrap();
                if !a.iter().any(|x| x["id"] == ev["callId"]) {
                    let mut item = json!({"id":ev["callId"],"kind":kind,"tool":ev["tool"],"text":text(&ev["text"]).chars().take(500).collect::<String>(),"questions":ev["questions"].as_array().cloned().unwrap_or_default(),"ts":ts});
                    // Only async Codex questions are optional; every other
                    // source keeps an item that never expires on its own.
                    if ev["optional"] == true {
                        item["optional"] = json!(true);
                    }
                    if grace > 0 {
                        item["graceUntil"] = json!(ts + grace);
                    }
                    a.push(item);
                }
                s["status"] = json!("wait");
                // A held request must not announce itself before it is visible,
                // otherwise the tray notification would fire inside the grace
                // window. The snapshot derives the event once it is shown.
                if grace == 0 {
                    event = Some(("wait".to_owned(), ev["callId"].clone()));
                }
            }
            "resolve" => {
                s["pending"]
                    .as_array_mut()
                    .unwrap()
                    .retain(|p| p["id"] != ev["callId"]);
                if !terminal(&text(&s["status"])) {
                    s["status"] = json!(if s["pending"].as_array().unwrap().is_empty() {
                        "running"
                    } else {
                        "wait"
                    });
                }
            }
            "permission_check" if !terminal(&text(&s["status"])) => {
                if !s["permissionChecks"].is_array() {
                    s["permissionChecks"] = json!([]);
                }
                let checks = s["permissionChecks"].as_array_mut().unwrap();
                if !checks.iter().any(|check| check["id"] == ev["callId"]) {
                    checks.push(json!({"id":ev["callId"],"ts":ts}));
                }
            }
            "permission_resolve" => {
                if !s["permissionChecks"].is_array() {
                    s["permissionChecks"] = json!([]);
                }
                s["permissionChecks"]
                    .as_array_mut()
                    .unwrap()
                    .retain(|check| check["id"] != ev["callId"]);
            }
            "end" => {
                s["status"] = ev["status"].clone();
                s["endedAt"] = json!(ts);
                s["pending"] = json!([]);
                s["permissionChecks"] = json!([]);
                if !text(&ev["endedBy"]).is_empty() {
                    s["endedBy"] = ev["endedBy"].clone();
                }
                if matches!(text(&s["status"]).as_str(), "done" | "error") {
                    event = Some((text(&s["status"]), s["roundId"].clone()));
                }
            }
            "tokens" if ev["tokens"].is_number() => s["tokens"] = ev["tokens"].clone(),
            _ => {}
        }
        if let Some((kind, key)) = event {
            let eid = json!([id, s["roundId"], kind, key]).to_string();
            if !self.events.iter().any(|e| e["id"] == eid) {
                self.events.push_back(json!({"id":eid,"sessionId":id,"roundId":s["roundId"],"kind":kind,"ts":ts,"historical":!self.ready||ts<self.started,"title":s["title"]}));
            }
            if self.events.len() > 500 {
                self.events.pop_front();
            }
        }
    }
    pub fn snapshot(&self) -> Value {
        let time = now();
        let hosted: HashSet<String> = self
            .sessions
            .values()
            .filter(|s| s["source"] == "codeg")
            .map(|s| text(&s["externalId"]).trim_start_matches("thr_").to_owned())
            .filter(|s| !s.is_empty())
            .collect();
        let mut sessions: Vec<Value> = self
            .sessions
            .values()
            .filter(|s| {
                !(s["source"] == "codex"
                    && (hosted.contains(text(&s["sessionId"]).trim_start_matches("thr_"))
                        || self
                            .hidden_codeg_codex_ids
                            .contains(text(&s["sessionId"]).trim_start_matches("thr_"))))
            })
            .cloned()
            .map(|mut s| {
                // An optional wait expires lazily on read: the host app hides
                // its own question card after the user stops interacting, so a
                // stale item must not keep the session in `wait`.
                // A permission wait is likewise projected lazily: `graceUntil`
                // holds it back until the client could still have auto-approved
                // it, so the card (and the notification derived from it) only
                // appears once the user really is the blocker.
                s["pending"].as_array_mut().unwrap().retain(|p| {
                    let held = p["graceUntil"].as_i64().unwrap_or(0) > time;
                    !held
                        && !(p["optional"] == true
                            && time - p["ts"].as_i64().unwrap_or(0) > OPTIONAL_TTL_MS)
                });
                if s["status"] == "wait" && s["pending"].as_array().unwrap().is_empty() {
                    s["status"] = json!("running");
                }
                let stale = !terminal(&text(&s["status"]))
                    && s["pending"].as_array().is_some_and(|a| a.is_empty())
                    && time - s["updatedAt"].as_i64().unwrap_or(0) > STALE;
                let cwd = text(&s["cwd"]);
                s["project"] = json!(std::path::Path::new(&cwd)
                    .file_name()
                    .and_then(|v| v.to_str())
                    .filter(|v| !v.is_empty())
                    .unwrap_or(s["source"].as_str().unwrap_or("")));
                s["stale"] = json!(stale);
                if stale {
                    s["status"] = json!("unknown");
                }
                s["elapsed"] = json!(((s["endedAt"].as_i64().unwrap_or(time)
                    - s["startedAt"].as_i64().unwrap_or(time))
                    as f64
                    / 1000.)
                    .max(0.));
                s["progress"] = Value::Null;
                s
            })
            .collect();
        sessions.sort_by_key(|s| std::cmp::Reverse(s["updatedAt"].as_i64().unwrap_or(0)));
        let mut events: Vec<_> = self
            .events
            .iter()
            .filter(|e| {
                let id = text(&e["sessionId"]);
                if let Some(s) = self.sessions.get(&id) {
                    let sid = text(&s["sessionId"]);
                    s["source"] != "codex"
                        || !(self
                            .hidden_codeg_codex_ids
                            .contains(sid.trim_start_matches("thr_"))
                            || hosted.contains(sid.trim_start_matches("thr_")))
                } else {
                    !self.hidden_codeg_codex_ids.contains(
                        id.strip_prefix("codex:")
                            .unwrap_or(&id)
                            .trim_start_matches("thr_"),
                    )
                }
            })
            .cloned()
            .collect();
        for s in &sessions {
            for p in s["pending"].as_array().into_iter().flatten() {
                let id = json!([s["id"], s["roundId"], "wait", p["id"]]).to_string();
                if !events.iter().any(|e| e["id"] == id) {
                    events.push(json!({"id":id,"sessionId":s["id"],"roundId":s["roundId"],"kind":"wait","ts":p["ts"],"historical":true,"title":s["title"]}));
                }
            }
        }
        json!({"version":1,"ready":self.ready,"ts":time,"sources":self.sources,"sessions":sessions,"events":events})
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One running WorkBuddy session whose round started 10s ago.
    fn started(hub: &mut Hub) -> i64 {
        let start = now() - 10_000;
        hub.ingest(json!({"source":"workbuddy","sessionId":"s","type":"start","roundId":"r1","ts":start}));
        start
    }

    /// A `wait` for that session; permissions carry the adapter's grace window.
    fn wait(call: &str, kind: &str, ts: i64) -> Value {
        let mut ev = json!({"source":"workbuddy","sessionId":"s","type":"wait","roundId":"r1","callId":call,"kind":kind,"tool":if kind == KIND_PERMISSION {"Bash"} else {"AskUserQuestion"},"text":"选哪个？","ts":ts});
        if kind == KIND_PERMISSION {
            ev["graceMs"] = json!(PERMISSION_GRACE_MS);
        }
        ev
    }

    fn row(snapshot: &Value) -> Value {
        snapshot["sessions"].as_array().unwrap().iter().find(|s| s["id"] == "workbuddy:s").cloned().unwrap_or(Value::Null)
    }
    fn ids(snapshot: &Value) -> Vec<String> {
        row(snapshot)["pending"].as_array().into_iter().flatten().map(|p| text(&p["id"])).collect()
    }
    fn waits(snapshot: &Value) -> usize {
        snapshot["events"].as_array().unwrap().iter().filter(|e| e["kind"] == json!("wait")).count()
    }

    #[test]
    fn permission_wait_is_held_for_the_grace_window() {
        let mut hub = Hub::new();
        let start = started(&mut hub);
        // `settled` outlived the 2.5s window, `fresh` has not.
        hub.ingest(wait("settled", KIND_PERMISSION, start + 5_000));
        hub.ingest(wait("fresh", KIND_PERMISSION, start + 9_000));
        let snapshot = hub.snapshot();
        assert_eq!(ids(&snapshot), vec!["settled"]);
        assert_eq!(row(&snapshot)["status"], json!("wait"));
        assert_eq!(row(&snapshot)["pending"][0]["kind"], json!(KIND_PERMISSION));
        // Only the visible gate is announced: the tray bridge reads these.
        assert_eq!(waits(&snapshot), 1);
        // The held request is tracked, not dropped; the projection compares
        // against `graceUntil`, so it appears once its own window elapses.
        assert_eq!(hub.sessions["workbuddy:s"]["pending"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn resolve_inside_the_grace_window_never_shows_or_announces() {
        let mut hub = Hub::new();
        let start = started(&mut hub);
        hub.ingest(wait("perm:1", KIND_PERMISSION, start + 500));
        // The client auto-approved: the tool result arrives 200ms later.
        hub.ingest(json!({"source":"workbuddy","sessionId":"s","type":"resolve","roundId":"r1","callId":"perm:1","ts":start + 700}));
        let snapshot = hub.snapshot();
        assert!(ids(&snapshot).is_empty());
        assert_eq!(row(&snapshot)["status"], json!("running"));
        assert_eq!(waits(&snapshot), 0);
    }

    #[test]
    fn repeated_wait_events_are_deduplicated_by_call_id() {
        let mut hub = Hub::new();
        let start = started(&mut hub);
        for offset in [1_000, 1_100, 1_200] {
            hub.ingest(wait("c1", KIND_QUESTION, start + offset));
        }
        let snapshot = hub.snapshot();
        assert_eq!(ids(&snapshot), vec!["c1"]);
        assert_eq!(waits(&snapshot), 1);
    }

    #[test]
    fn question_waits_keep_kind_and_are_not_held() {
        let mut hub = Hub::new();
        let start = started(&mut hub);
        hub.ingest(wait("q1", KIND_QUESTION, start + 1));
        let snapshot = hub.snapshot();
        assert_eq!(row(&snapshot)["pending"][0]["kind"], json!(KIND_QUESTION));
        assert_eq!(ids(&snapshot), vec!["q1"]);
        assert!(row(&snapshot)["pending"][0]["graceUntil"].is_null());
    }

    #[test]
    fn pending_kind_is_inferred_for_sources_that_do_not_stamp_it() {
        // A sandbox approval log line carries only a tool name.
        assert_eq!(pending_kind(&json!({"tool":"Bash","text":"沙箱审批待确认：Bash"})), KIND_PERMISSION);
        assert_eq!(pending_kind(&json!({"tool":"permission"})), KIND_PERMISSION);
        assert_eq!(pending_kind(&json!({"tool":"notification"})), KIND_PERMISSION);
        assert_eq!(pending_kind(&json!({"tool":"ask"})), KIND_QUESTION);
        assert_eq!(pending_kind(&json!({"tool":"AskUserQuestion"})), KIND_QUESTION);
        assert_eq!(pending_kind(&json!({"tool":"x","questions":[{"text":"?"}]})), KIND_QUESTION);
        // An explicit stamp always wins over the inference.
        assert_eq!(pending_kind(&json!({"tool":"Bash","kind":"question"})), KIND_QUESTION);
        assert_eq!(pending_kind(&json!({"tool":"ask","kind":"permission"})), KIND_PERMISSION);
    }

    #[test]
    fn end_of_round_clears_a_held_permission_wait() {
        let mut hub = Hub::new();
        let start = started(&mut hub);
        hub.ingest(wait("perm:1", KIND_PERMISSION, start + 1_000));
        assert_eq!(ids(&hub.snapshot()), vec!["perm:1"]);
        hub.ingest(json!({"source":"workbuddy","sessionId":"s","type":"end","roundId":"r1","status":"done","ts":start + 2_000}));
        let snapshot = hub.snapshot();
        assert!(ids(&snapshot).is_empty());
        assert_eq!(row(&snapshot)["status"], json!("done"));
    }
}
