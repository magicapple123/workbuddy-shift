//! 自定义模板引擎的会话状态：轮次、去重与容量上限。

use super::*;

#[derive(Clone, Debug)]
pub(crate) struct SessionState {
    pub(crate) round: String,
    pub(crate) active: bool,
    pub(crate) seen: VecDeque<String>,
    pub(crate) seen_set: HashSet<String>,
    pub(crate) pending: VecDeque<String>,
}

impl SessionState {
    pub(crate) fn new(round: String) -> Self {
        Self {
            round,
            active: true,
            seen: VecDeque::new(),
            seen_set: HashSet::new(),
            pending: VecDeque::new(),
        }
    }
    pub(crate) fn key(round: &str, event_id: &str) -> String {
        format!("{round}|{event_id}")
    }
    pub(crate) fn seen(&self, round: &str, event_id: &str) -> bool {
        self.seen_set.contains(&Self::key(round, event_id))
    }
    pub(crate) fn remember(&mut self, round: &str, event_id: &str) {
        let key = Self::key(round, event_id);
        if self.seen_set.insert(key.clone()) {
            self.seen.push_back(key);
        }
        while self.seen.len() > DEDUP_PER_SESSION {
            if let Some(old) = self.seen.pop_front() {
                self.seen_set.remove(&old);
            }
        }
    }
    pub(crate) fn push_pending(&mut self, call_id: &str) {
        if self.pending.iter().any(|id| id == call_id) {
            return;
        }
        self.pending.push_back(call_id.to_owned());
        while self.pending.len() > PENDING_MAX {
            self.pending.pop_front();
        }
    }
    pub(crate) fn take_pending(&mut self, call_id: &str) -> bool {
        let found = self.pending.iter().position(|id| id == call_id);
        match found {
            Some(index) => {
                self.pending.remove(index);
                true
            }
            None => false,
        }
    }
}

impl Engine {
    pub(crate) fn store(&mut self, key: String, state: SessionState) {
        if !self.sessions.contains_key(&key) {
            self.order.push_back(key.clone());
        }
        self.sessions.insert(key, state);
        while self.sessions.len() > SESSIONS_MAX {
            let Some(old) = self.order.pop_front() else {
                break;
            };
            self.sessions.remove(&old);
        }
    }
}

impl Engine {
    /// Removes one session; returns true when it existed.
    pub fn forget(&mut self, key: &str) -> bool {
        self.order.retain(|entry| entry != key);
        self.sessions.remove(key).is_some()
    }
}

impl Engine {
    pub fn forget_source(&mut self, source: &str) -> usize {
        let prefix = format!("{source}:");
        let keys: Vec<String> = self
            .sessions
            .keys()
            .filter(|key| key.starts_with(&prefix))
            .cloned()
            .collect();
        let count = keys.len();
        for key in keys {
            self.forget(&key);
        }
        count
    }
}
