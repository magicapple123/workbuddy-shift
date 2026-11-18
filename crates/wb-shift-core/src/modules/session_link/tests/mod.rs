use super::content::*;
use super::link_store::*;
use super::operation_log::*;
use super::preview::*;
use super::sync_decision::*;
use crate::modules::session::SessionPaths;
use crate::modules::session_backup;
use crate::modules::variant::WbVariant;
use std::path::{Path, PathBuf};
use std::time::Duration;

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "wb_shift_link_{}_{name}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn temp_paths(dir: &TempDir) -> SessionPaths {
    SessionPaths {
        store_root: dir.path().join("store"),
        data_root: dir.path().join("data"),
        link_namespace: crate::modules::session::LinkNamespace::WorkBuddy,
        auth_file: dir.path().join("auth.info"),
    }
}

fn member(uid: &str, session_id: &str, state: MemberState) -> LinkMember {
    LinkMember {
        member_id: format!("m-{uid}-{session_id}"),
        account_id: None,
        uid: uid.to_string(),
        session_id: session_id.to_string(),
        variant: None,
        state,
        linked_at: 1,
        last_synced_at: None,
    }
}

fn group_with_members(id: &str, members: Vec<LinkMember>) -> LinkGroup {
    LinkGroup {
        id: id.to_string(),
        variant: WbVariant::Cn,
        created_at: 1,
        members,
        pair_bases: Vec::new(),
    }
}

fn operation(id: &str, phase: OpPhase, target_uid: &str, target_session: &str) -> Operation {
    Operation {
        version: OPERATION_VERSION,
        operation_id: id.to_string(),
        kind: "copy".to_string(),
        variant: WbVariant::Cn,
        source_variant: None,
        group_id: "g-1".to_string(),
        source: OperationMember {
            account_id: None,
            uid: "uid-a".to_string(),
            session_id: "sess-1".to_string(),
        },
        target: OperationMember {
            account_id: None,
            uid: target_uid.to_string(),
            session_id: target_session.to_string(),
        },
        expected_content_digest: "digest".to_string(),
        expected_record_count: 1,
        phase,
        backup: None,
        lifecycle_version: None,
        cleanup_state: None,
        last_error: None,
        created_at: 1,
        updated_at: 1,
    }
}

fn sample_body(cid: &str) -> String {
    format!(
        "{}\n{}\n",
        r#"{"type":"user","sessionId":"CID","text":"你好"}"#.replace("CID", cid),
        r#"{"type":"assistant","sessionId":"CID","text":"hi"}"#.replace("CID", cid)
    )
}

// ---- 主题片段（测试名与排序均与拆分前一致） ----

include!("content.rs");

include!("link_store.rs");

include!("operation_log.rs");

include!("sync_decision.rs");

include!("preview.rs");
