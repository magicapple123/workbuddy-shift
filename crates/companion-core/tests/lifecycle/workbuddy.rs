use super::*;
use companion_core::now;

#[test]
fn workbuddy_wait_and_authoritative_completion() {
    let home = Home::new();
    let mut c = home.collector("workbuddy");
    let t = now();
    assert!(c.ingest_hook(&json!({
        "source":"workbuddy",
        "session_id":"x",
        "cwd":"/p",
        "timestamp":t,
        "hook_event_name":"UserPromptSubmit",
        "prompt":"hello"
    })));
    assert!(c.ingest_hook(&json!({
        "source":"workbuddy",
        "session_id":"x",
        "cwd":"/p",
        "timestamp":t+1,
        "hook_event_name":"PreToolUse",
        "tool_name":"AskUserQuestion",
        "tool_use_id":"q",
        "tool_input":{"questions":[{"question":"选哪个？"}]}
    })));
    assert_eq!(c.hub.sessions["workbuddy:x"]["status"], "wait");
    assert_eq!(
        c.hub.sessions["workbuddy:x"]["pending"][0]["text"],
        "选哪个？"
    );
    assert!(c.ingest_hook(&json!({
        "source":"workbuddy",
        "session_id":"x",
        "timestamp":t+2,
        "hook_event_name":"PostToolUse",
        "tool_name":"AskUserQuestion",
        "tool_use_id":"q"
    })));
    assert_eq!(c.hub.sessions["workbuddy:x"]["status"], "running");
    assert!(c.ingest_hook(&json!({
        "source":"workbuddy",
        "session_id":"x",
        "timestamp":t+3,
        "hook_event_name":"Stop"
    })));
    assert_eq!(c.hub.sessions["workbuddy:x"]["status"], "done");
    assert_eq!(c.hub.sessions["workbuddy:x"]["agentType"], "workbuddy");
    let mut restarted = home.collector("workbuddy");
    restarted.poll();
    assert!(restarted.hub.sessions.is_empty());
}

#[test]
fn workbuddy_international_hooks_set_agent_type() {
    let home = Home::new();
    let mut c = home.collector("workbuddy");
    assert!(c.ingest_hook(&json!({
        "source":"workbuddy",
        "agent_edition":"international",
        "session_id":"ai",
        "hook_event_name":"UserPromptSubmit",
        "prompt":"hello"
    })));
    assert_eq!(c.hub.sessions["workbuddy:ai"]["agentType"], "workbuddy-ai");
}

#[test]
fn workbuddy_permission_and_notification_become_wait() {
    let home = Home::new();
    let mut c = home.collector("workbuddy");
    assert!(c.ingest_hook(&json!({
        "source":"workbuddy",
        "session_id":"p",
        "hook_event_name":"UserPromptSubmit",
        "prompt":"read secrets"
    })));
    assert!(c.ingest_hook(&json!({
        "source":"workbuddy",
        "session_id":"p",
        "hook_event_name":"PermissionRequest",
        "tool_name":"Read",
        "tool_use_id":"cred",
        "message":"Allow the model to access sensitive credentials?"
    })));
    assert_eq!(c.hub.sessions["workbuddy:p"]["status"], "wait");
    assert_eq!(
        c.hub.sessions["workbuddy:p"]["pending"][0]["text"],
        "Allow the model to access sensitive credentials?"
    );
    assert!(c.ingest_hook(&json!({
        "source":"workbuddy",
        "session_id":"p",
        "hook_event_name":"PostToolUse",
        "tool_name":"Read",
        "tool_use_id":"cred"
    })));
    assert_eq!(c.hub.sessions["workbuddy:p"]["status"], "running");
    assert!(c.ingest_hook(&json!({
        "source":"workbuddy",
        "session_id":"n",
        "hook_event_name":"Notification",
        "message":"Allow the model to access sensitive credentials?"
    })));
    assert_eq!(c.hub.sessions["workbuddy:n"]["status"], "wait");
    assert!(c.ingest_hook(&json!({
        "source":"workbuddy",
        "session_id":"n",
        "hook_event_name":"Notification",
        "notification_type":"auth_success"
    })));
    assert_eq!(c.hub.sessions["workbuddy:n"]["status"], "wait");
    assert_eq!(
        companion_core::adapters::workbuddy_edition(std::path::Path::new(
            "/tmp/.workbuddy-ai/settings.json"
        )),
        "international"
    );
    assert_eq!(
        companion_core::adapters::workbuddy_edition(std::path::Path::new(
            "/tmp/.workbuddy/settings.json"
        )),
        "domestic"
    );
}

#[test]
fn workbuddy_hooks_preserve_foreign_settings_and_skip_permissions() {
    let merged = companion_core::adapters::merge_workbuddy_hooks(
        json!({
            "sandbox":{"extraAllowWrite":["~/tmp"]},
            "hooks":{"PermissionRequest":[{"hooks":[{"type":"http","url":"http://example"}]}]}
        }),
        "bin hook --home /tmp --source workbuddy",
    )
    .unwrap();
    assert_eq!(merged["sandbox"]["extraAllowWrite"][0], "~/tmp");
    assert_eq!(
        merged["hooks"]["PermissionRequest"][0]["hooks"][0]["url"],
        "http://example"
    );
    assert!(merged["hooks"].get("PermissionRequest").is_some());
    let pre = merged["hooks"]["PreToolUse"][0]["hooks"][0]["command"]
        .as_str()
        .unwrap();
    assert!(pre.contains("--source workbuddy"));
    assert_eq!(merged["hooks"]["PreToolUse"][0]["matcher"], "");
}

#[test]
fn workbuddy_settings_cover_international_and_domestic_editions() {
    let home = Home::new();
    std::fs::create_dir_all(home.0.join(".workbuddy/binaries")).unwrap();
    std::fs::create_dir_all(home.0.join(".workbuddy-ai")).unwrap();
    let files = companion_core::adapters::workbuddy_settings_files(&home.0, "");
    assert_eq!(files, vec![home.0.join(".workbuddy-ai/settings.json")]);
    std::fs::write(home.0.join(".workbuddy/settings.json"), "{}").unwrap();
    let files = companion_core::adapters::workbuddy_settings_files(&home.0, "");
    assert_eq!(
        files,
        vec![
            home.0.join(".workbuddy-ai/settings.json"),
            home.0.join(".workbuddy/settings.json")
        ]
    );
    std::fs::remove_dir_all(home.0.join(".workbuddy-ai")).unwrap();
    let files = companion_core::adapters::workbuddy_settings_files(&home.0, "");
    assert_eq!(files, vec![home.0.join(".workbuddy/settings.json")]);
}
#[test]
fn workbuddy_log_watch_marks_sandbox_approval_and_clears_on_settle() {
    use std::io::Write;
    let home = Home::new();
    let mut c = home.collector("workbuddy");
    assert!(c.ingest_hook(&json!({
        "source":"workbuddy","session_id":"s1","hook_event_name":"UserPromptSubmit","prompt":"scan"
    })));
    assert_eq!(c.hub.sessions["workbuddy:s1"]["status"], "running");

    let dir = home.0.join(".workbuddy/logs/2026-09-26");
    std::fs::create_dir_all(&dir).unwrap();
    let log = dir.join("session.log");
    std::fs::write(&log, "[Info] startup\n").unwrap();
    // 首次扫描只登记 offset（不看历史内容）。
    c.poll_workbuddy_log_watch();
    assert!(c.workbuddy_log_watch.primed);

    let append = |line: &str| {
        let mut f = std::fs::OpenOptions::new().append(true).open(&log).unwrap();
        writeln!(f, "{line}").unwrap();
    };
    let scan = |c: &mut Collector| {
        c.workbuddy_log_watch.last_scan = 0; // 绕过节流
        c.poll_workbuddy_log_watch();
    };

    // 弹框行 → 已跟踪会话变 wait；未跟踪会话不产生幽灵会话。
    append("[9/26/2026, 3:00:11 AM.478] [Info] [pid=1] [enqueueSandboxApproval] Enqueued sandbox approval: tool=Bash, id=call-1, session=s1, mainSession=s1, queueSize=1");
    append("[9/26/2026, 3:00:12 AM.000] [Info] [pid=1] [enqueueSandboxApproval] Enqueued sandbox approval: tool=Bash, id=call-2, session=s2, mainSession=s2, queueSize=1");
    scan(&mut c);
    assert_eq!(c.hub.sessions["workbuddy:s1"]["status"], "wait");
    assert_eq!(c.hub.sessions["workbuddy:s1"]["pending"][0]["id"], "call-1");
    assert!(c.hub.sessions["workbuddy:s1"]["pending"][0]["text"]
        .as_str()
        .unwrap()
        .contains("沙箱审批"));
    assert!(!c.hub.sessions.contains_key("workbuddy:s2"));

    // 允许行 → resolve，恢复 running。
    append("[9/26/2026, 3:00:20 AM.123] [Info] [pid=1] [Approve] User approved tool: Bash, id: call-1, alwaysApprove: false, scope: session");
    scan(&mut c);
    assert_eq!(c.hub.sessions["workbuddy:s1"]["status"], "running");
    assert!(c.hub.sessions["workbuddy:s1"]["pending"]
        .as_array()
        .unwrap()
        .is_empty());
}
#[test]
fn workbuddy_log_watch_idles_without_active_sessions() {
    let home = Home::new();
    let mut c = home.collector("workbuddy");
    // 无任何会话：不扫描、不 prime。
    c.poll_workbuddy_log_watch();
    assert!(!c.workbuddy_log_watch.primed);
    // 会话进入运行后才开始跟踪。
    assert!(c.ingest_hook(&json!({
        "source":"workbuddy","session_id":"s1","hook_event_name":"UserPromptSubmit","prompt":"scan"
    })));
    c.poll_workbuddy_log_watch();
    assert!(c.workbuddy_log_watch.primed);
    // 会话结束后回到空闲：不再推进扫描。
    assert!(c.ingest_hook(&json!({
        "source":"workbuddy","session_id":"s1","hook_event_name":"Stop"
    })));
    c.workbuddy_log_watch.last_scan = 0;
    c.poll_workbuddy_log_watch();
    assert_eq!(c.workbuddy_log_watch.last_scan, 0);
}
