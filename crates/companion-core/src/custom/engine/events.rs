//! 自定义模板事件映射：模板动作 -> 标准 Outcome。

use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutcomeKind {
    Accepted,
    Ignored,
    Rejected,
}

impl OutcomeKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Accepted => "accepted",
            Self::Ignored => "ignored",
            Self::Rejected => "rejected",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Outcome {
    pub kind: OutcomeKind,
    /// Action label for accepted events (`start`, `wait`, ...).
    pub action: Option<&'static str>,
    /// Machine-readable reason code; for accepted events the action label.
    pub reason: String,
    pub detail: String,
    pub path: String,
    pub raw_event: Option<String>,
    pub session_id: Option<String>,
    pub round_id: Option<String>,
    pub events: Vec<Value>,
}

impl Outcome {
    pub(crate) fn accepted(
        action: Action,
        raw_event: &str,
        session_id: &str,
        round_id: &str,
        events: Vec<Value>,
    ) -> Self {
        Self {
            kind: OutcomeKind::Accepted,
            action: Some(action.label()),
            reason: action.label().into(),
            detail: String::new(),
            path: String::new(),
            raw_event: Some(raw_event.into()),
            session_id: Some(session_id.into()),
            round_id: Some(round_id.into()),
            events,
        }
    }
    pub(crate) fn ignored(
        action: Action,
        reason: &'static str,
        detail: impl Into<String>,
        raw_event: &str,
        session_id: &str,
        round_id: Option<String>,
    ) -> Self {
        Self {
            kind: OutcomeKind::Ignored,
            action: Some(action.label()),
            reason: reason.into(),
            detail: detail.into(),
            path: String::new(),
            raw_event: Some(raw_event.into()),
            session_id: Some(session_id.into()),
            round_id,
            events: Vec::new(),
        }
    }
    pub(crate) fn rejected(reason: &'static str, path: &str, detail: impl Into<String>) -> Self {
        Self {
            kind: OutcomeKind::Rejected,
            action: None,
            reason: reason.into(),
            detail: detail.into(),
            path: path.into(),
            raw_event: None,
            session_id: None,
            round_id: None,
            events: Vec::new(),
        }
    }
    pub fn with_event(mut self, raw_event: Option<&str>, session_id: Option<&str>) -> Self {
        if let Some(raw_event) = raw_event {
            self.raw_event = Some(raw_event.into());
        }
        if let Some(session_id) = session_id {
            self.session_id = Some(session_id.into());
        }
        self
    }
    pub fn is_accepted(&self) -> bool {
        self.kind == OutcomeKind::Accepted
    }
    pub fn to_value(&self) -> Value {
        json!({
            "outcome": self.kind.label(),
            "action": self.action,
            "reason": self.reason,
            "detail": self.detail.chars().take(DETAIL_MAX).collect::<String>(),
            "path": self.path,
            "event": self.raw_event,
            "sessionId": self.session_id,
            "roundId": self.round_id,
            "events": self.events,
        })
    }
}

impl Engine {
    /// Maps one raw payload. `now` is the receive time, used when the template
    /// does not map a timestamp.
    pub fn apply(&mut self, template: &Template, raw: &Value, now: i64) -> Outcome {
        if !raw.is_object() {
            return Outcome::rejected("invalid_payload", "", "载荷必须是 JSON 对象");
        }
        let raw_event = match required_event(raw, &template.mapping.event) {
            Ok(value) => value,
            Err(error) => return error.into_outcome(),
        };
        let Some(action) = template.action(&raw_event) else {
            return Outcome::ignored(
                Action::Start,
                "unknown_event",
                format!("未映射的事件 {raw_event}"),
                &raw_event,
                "",
                None,
            );
        };
        if template
            .ignore_if_present
            .iter()
            .any(|pointer| present_value(raw, pointer))
        {
            return Outcome::ignored(
                action,
                "ignored_field_present",
                "命中 ignoreIfPresent，已忽略",
                &raw_event,
                "",
                None,
            );
        }
        let session_id = match id_field(
            raw,
            Some(&template.mapping.session_id),
            SESSION_ID_MAX,
            "sessionId",
            "载荷中缺少会话 ID",
        ) {
            Ok(Some(value)) => value,
            Ok(None) => {
                return Outcome::rejected(
                    "missing_session_id",
                    "/mapping/sessionId",
                    "载荷中缺少会话 ID",
                )
            }
            Err(error) => return error.into_outcome().with_event(Some(&raw_event), None),
        };
        let round_id = match id_field(
            raw,
            template.mapping.round_id.as_ref(),
            ID_FIELD_MAX,
            "roundId",
            "字段类型无效",
        ) {
            Ok(value) => value,
            Err(error) => {
                return error
                    .into_outcome()
                    .with_event(Some(&raw_event), Some(&session_id))
            }
        };
        let event_id = match id_field(
            raw,
            template.mapping.event_id.as_ref(),
            EVENT_ID_MAX,
            "eventId",
            "字段类型无效",
        ) {
            Ok(value) => value,
            Err(error) => {
                return error
                    .into_outcome()
                    .with_event(Some(&raw_event), Some(&session_id))
            }
        };
        let request_id = match id_field(
            raw,
            template.mapping.request_id.as_ref(),
            ID_FIELD_MAX,
            "requestId",
            "字段类型无效",
        ) {
            Ok(value) => value,
            Err(error) => {
                return error
                    .into_outcome()
                    .with_event(Some(&raw_event), Some(&session_id))
            }
        };
        let cwd = match text_field(raw, template.mapping.cwd.as_ref(), CWD_MAX, "cwd") {
            Ok(value) => value,
            Err(error) => {
                return error
                    .into_outcome()
                    .with_event(Some(&raw_event), Some(&session_id))
            }
        };
        let title = match text_field(raw, template.mapping.title.as_ref(), TITLE_MAX, "title") {
            Ok(value) => value,
            Err(error) => {
                return error
                    .into_outcome()
                    .with_event(Some(&raw_event), Some(&session_id))
            }
        };
        let ts = match timestamp_field(raw, template.mapping.timestamp.as_ref()) {
            Ok(Some(value)) => value,
            Ok(None) => now,
            Err(error) => {
                return error
                    .into_outcome()
                    .with_event(Some(&raw_event), Some(&session_id))
            }
        };
        let source = crate::custom::source_of(&template.id);
        let event_context = StandardEventContext {
            template,
            source: &source,
            session_id: &session_id,
            ts,
            cwd: cwd.as_deref(),
            title: title.as_deref(),
        };
        let key = format!("{source}:{session_id}");
        let current = self.sessions.get(&key).cloned();
        match action {
            Action::Start => {
                let round = round_id.clone().unwrap_or_else(|| format!("custom:{ts}"));
                // An explicit event id is checked first: the same payload resent is a
                // duplicate, not a new round.
                if let (Some(state), Some(event_id)) = (&current, &event_id) {
                    if state.seen(&round, event_id) {
                        return Outcome::ignored(
                            action,
                            "duplicate_event",
                            "重复事件已忽略",
                            &raw_event,
                            &session_id,
                            Some(round),
                        );
                    }
                }
                if current.as_ref().is_some_and(|state| state.round == round) {
                    return Outcome::ignored(
                        action,
                        "duplicate_start",
                        "同轮次的重复 start 已忽略",
                        &raw_event,
                        &session_id,
                        Some(round),
                    );
                }
                let mut next = SessionState::new(round.clone());
                if let Some(event_id) = &event_id {
                    next.remember(&round, event_id);
                }
                let events = vec![event_context.event(&round, json!({"type":"start"}))];
                self.store(key, next);
                Outcome::accepted(action, &raw_event, &session_id, &round, events)
            }
            _ => {
                let Some(mut state) = current else {
                    return Outcome::ignored(
                        action,
                        "no_active_round",
                        "没有活跃轮次，事件未创建会话",
                        &raw_event,
                        &session_id,
                        None,
                    );
                };
                if let Some(round) = &round_id {
                    if *round != state.round {
                        return Outcome::ignored(
                            action,
                            "late_round",
                            "事件轮次与当前轮次不一致，已忽略",
                            &raw_event,
                            &session_id,
                            Some(state.round.clone()),
                        );
                    }
                }
                if !state.active {
                    return Outcome::ignored(
                        action,
                        "round_ended",
                        "轮次已结束，事件已忽略",
                        &raw_event,
                        &session_id,
                        Some(state.round.clone()),
                    );
                }
                if let Some(event_id) = &event_id {
                    if state.seen(&state.round.clone(), event_id) {
                        return Outcome::ignored(
                            action,
                            "duplicate_event",
                            "重复事件已忽略",
                            &raw_event,
                            &session_id,
                            Some(state.round.clone()),
                        );
                    }
                }
                let round = state.round.clone();
                let events = match action {
                    Action::Wait(reason) => {
                        let call_id = request_id
                            .clone()
                            .unwrap_or_else(|| WAIT_CALL_ID.to_owned());
                        state.push_pending(&call_id);
                        vec![event_context.event(&round, json!({
                                "type":"wait",
                                "callId":call_id,
                                "tool":"custom",
                                "text": if reason == WaitReason::Permission { "等待权限确认" } else { "等待用户输入" },
                            }))]
                    }
                    Action::Resume => {
                        let call_id = request_id
                            .clone()
                            .unwrap_or_else(|| WAIT_CALL_ID.to_owned());
                        if !state.take_pending(&call_id) {
                            return Outcome::ignored(
                                action,
                                "no_pending_wait",
                                "没有匹配的等待项，resume 已忽略",
                                &raw_event,
                                &session_id,
                                Some(round),
                            );
                        }
                        vec![event_context.event(&round, json!({"type":"resolve","callId":call_id}))]
                    }
                    Action::Finish(status) => {
                        state.active = false;
                        state.pending.clear();
                        vec![event_context
                            .event(&round, json!({"type":"end","status":status.label()}))]
                    }
                    Action::Close => {
                        state.active = false;
                        state.pending.clear();
                        vec![event_context.event(
                            &round,
                            json!({"type":"end","status":"aborted","endedBy":raw_event}),
                        )]
                    }
                    Action::Start => unreachable!(),
                };
                if let Some(event_id) = &event_id {
                    state.remember(&round, event_id);
                }
                self.store(key, state);
                Outcome::accepted(action, &raw_event, &session_id, &round, events)
            }
        }
    }
}
