//! Codeg 委派子会话状态与释放规则。

use super::*;

/// A known delegated child. It stays invisible unless it waits for the user,
/// so this only carries what it takes to ignore its traffic cheaply and to
/// release the temporary card again.
#[derive(Default)]
pub(crate) struct ChildState {
    /// Conversation id of the child; always resolved, never provisional.
    pub(crate) sid: String,
    /// Request id of the card on screen, if any.
    pub(crate) open: Option<String>,
    /// When that card was created, for the TTL sweep.
    pub(crate) started_at: i64,
    /// Recently answered request ids; a late duplicate must not revive them.
    pub(crate) resolved: VecDeque<String>,
}

impl Collector {
    /// Give up a child's temporary card: the hub session goes away, the stream
    /// is released (dropping it stops its socket and retry loop) and the
    /// answered request is remembered so a late duplicate cannot revive it.
    pub(crate) fn codeg_release_child(&mut self, conn: &str, answered: Option<&str>) {
        let Some(child) = self.codeg.children.get_mut(conn) else {
            return;
        };
        let open = child.open.take();
        child.started_at = 0;
        for id in [answered, open.as_deref()].into_iter().flatten() {
            if child.resolved.iter().any(|known| known == id) {
                continue;
            }
            if child.resolved.len() >= CODEG_CHILD_ANSWERED {
                child.resolved.pop_front();
            }
            child.resolved.push_back(id.to_owned());
        }
        let sid = child.sid.clone();
        self.codeg.streams.remove(conn);
        self.hub.sessions.remove(&format!("codeg:{sid}"));
    }
}

impl Collector {
    /// Release cards past their TTL, or every card once the source is disabled.
    /// The known-child list itself survives while the source stays enabled, so
    /// delegated traffic keeps costing nothing.
    pub(crate) fn codeg_release_children(&mut self, ttl: Option<i64>) {
        let time = now();
        let release: Vec<String> = self
            .codeg
            .children
            .iter()
            .filter(|(_, child)| {
                child.open.is_some() && ttl.is_none_or(|ttl| time - child.started_at > ttl)
            })
            .map(|(conn, _)| conn.clone())
            .collect();
        for conn in release {
            self.codeg_release_child(&conn, None);
        }
        if ttl.is_none() {
            self.codeg.children.clear();
        }
    }
}
