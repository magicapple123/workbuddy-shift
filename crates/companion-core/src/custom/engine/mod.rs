//! Pure mapping engine: template + raw payload -> standard hub events.
//!
//! The engine owns the lifecycle rules (rounds, waits, dedup) and never touches
//! the hub, the filesystem or the network, so the preview path can reuse it with
//! no side effects.

use super::limits::*;
use super::template::{Action, Template, WaitReason};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet, VecDeque};

mod events;
mod state;

pub use events::*;
pub(crate) use state::*;

use super::mapping::{
    id_field, present_value, required_event, text_field, timestamp_field, StandardEventContext,
};
/// The single session-level wait slot used when the template does not map a request id.
pub const WAIT_CALL_ID: &str = "custom-wait";

/// A session that keeps sending unmatched events cannot grow without bound.
const PENDING_MAX: usize = 16;

pub(super) struct MappingError {
    reason: &'static str,
    path: String,
    detail: String,
}

impl MappingError {
    pub(super) fn new(
        reason: &'static str,
        path: impl Into<String>,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            reason,
            path: path.into(),
            detail: detail.into(),
        }
    }

    fn into_outcome(self) -> Outcome {
        Outcome::rejected(self.reason, &self.path, self.detail)
    }
}

#[derive(Default)]
pub struct Engine {
    sessions: HashMap<String, SessionState>,
    order: VecDeque<String>,
}

impl Engine {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Engine {
    pub fn session_count(&self) -> usize {
        self.sessions.len()
    }
}

impl Engine {
    /// Visible state for tests and diagnostics; `active` is true while a round is
    /// still open, `pending` counts unresolved waits.
    pub fn session_state(&self, source: &str, session_id: &str) -> Option<(String, bool, usize)> {
        self.sessions
            .get(&format!("{source}:{session_id}"))
            .map(|state| (state.round.clone(), state.active, state.pending.len()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounds_the_tracked_sessions() {
        let mut engine = Engine::new();
        for index in 0..(SESSIONS_MAX + 20) {
            engine.store(format!("custom:x:{index}"), SessionState::new("r".into()));
        }
        assert_eq!(engine.session_count(), SESSIONS_MAX);
    }

    #[test]
    fn dedup_buffer_is_bounded_and_forgets_oldest() {
        let mut state = SessionState::new("r".into());
        for index in 0..(DEDUP_PER_SESSION + 5) {
            state.remember("r", &format!("e{index}"));
        }
        assert_eq!(state.seen.len(), DEDUP_PER_SESSION);
        assert!(!state.seen("r", "e0"));
        assert!(state.seen("r", &format!("e{}", DEDUP_PER_SESSION + 4)));
    }
}
