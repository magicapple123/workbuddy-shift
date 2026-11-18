// WorkBuddy is event-driven via Claude Code-compatible command hooks.
// Permissions stay in WorkBuddy's native GUI; this adapter never gates them.
// Sandbox approvals do not emit any hook event (see README known limits), so a
// best-effort watcher below detects them from WorkBuddy's own run logs.
use super::*;
use crate::{content, merge, question, question_tool, questions};
use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

mod events;
mod hooks;
mod log_watch;

pub use hooks::*;
pub use log_watch::*;

// ---- 沙箱审批日志观察 ----
// 沙箱类审批（敏感凭证/沙箱写/HTTP 拦截/批量删除）不发送任何 hook 事件，
// 但会写进 WorkBuddy 的会话级运行日志。这里做增量扫描，把弹框/处理事件
// 转成 hub 事件（wait / resolve），使悬浮窗显示「待确认」。
// 只匹配审批行，其余内容读入即弃；不读取会话文件、数据库或 transcript。
#[cfg(test)]
mod approval_log_tests {
    use super::*;

    #[test]
    fn parses_approval_and_settle_lines() {
        let request = "[9/26/2026, 2:44:29 AM.430] [Info] [pid=79853] [enqueueSandboxApproval] Enqueued sandbox approval: tool=Bash, id=chatcmpl-tool-b76a22bd4ea6be30, session=90b92496-4265-46dc-a497-10b119927240, mainSession=90b92496-4265-46dc-a497-10b119927240, queueSize=1";
        assert_eq!(
            parse_approval_line(request),
            Some(ApprovalEvent::Request {
                tool: "Bash".into(),
                call_id: "chatcmpl-tool-b76a22bd4ea6be30".into(),
                session: "90b92496-4265-46dc-a497-10b119927240".into(),
            })
        );
        let approved = "[9/26/2026, 2:44:45 AM.120] [Info] [pid=79853] [Approve] User approved tool: Bash, id: chatcmpl-tool-b76a22bd4ea6be30, alwaysApprove: false, scope: session";
        assert_eq!(
            parse_approval_line(approved),
            Some(ApprovalEvent::Settled {
                call_id: "chatcmpl-tool-b76a22bd4ea6be30".into()
            })
        );
        let rejected = "[9/26/2026, 2:32:01 AM.719] [Info] [pid=70427] [Reject] User rejected tool: Bash, id: chatcmpl-tool-cfc21a572fa2488a815a1f792f807d16";
        assert_eq!(
            parse_approval_line(rejected),
            Some(ApprovalEvent::Settled {
                call_id: "chatcmpl-tool-cfc21a572fa2488a815a1f792f807d16".into()
            })
        );
        assert_eq!(parse_approval_line("[Info] unrelated line"), None);
        assert_eq!(
            parse_approval_line("Sandbox approval timed out after 1800000ms, denying"),
            None
        );
    }
}
