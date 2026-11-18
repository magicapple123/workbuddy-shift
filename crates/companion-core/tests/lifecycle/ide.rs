use super::*;
#[cfg(any(target_os = "macos", target_os = "linux"))]
use companion_core::host_process::{HostPresence, Presence};
use companion_core::now;
#[cfg(any(target_os = "macos", target_os = "linux"))]
use std::time::Duration;

#[test]
fn ide_hooks_only_lifecycle_and_shared_cli_filter() {
    let home = Home::new();
    let mut c = home.collector("codebuddy-ide");
    c.poll();
    assert!(c.hub.sessions.is_empty());
    assert!(!c.ingest_hook(&json!({"agent_source":"codebuddy-ide","session_id":"cli","hook_event_name":"UserPromptSubmit","client":"cli"})));
    let cases: serde_json::Value =
        serde_json::from_str(include_str!("../fixtures/codebuddy-ide-hooks.json")).unwrap();
    let t = now();
    for (i, case) in cases.as_array().unwrap().iter().enumerate() {
        let p = companion_core::merge(
            json!({"agent_source":"codebuddy-ide","client":"CodeBuddyIDE","session_id":"x","cwd":"/project","timestamp":t+i as i64}),
            case["hook"].clone(),
        );
        assert_eq!(c.ingest_hook(&p), case["accepted"] != false, "case {i}");
        let status = c
            .hub
            .sessions
            .get("codebuddy-ide:x")
            .map(|s| s["status"].clone())
            .unwrap_or_default();
        assert_eq!(status, case["status"], "case {i}");
    }
    let mut restarted = home.collector("codebuddy-ide");
    restarted.poll();
    assert!(restarted.hub.sessions.is_empty());
    c.settings["sources"]["codebuddy-ide"]["enabled"] = json!(false);
    assert!(!c.ingest_hook(&json!({"agent_source":"codebuddy-ide","client":"CodeBuddyIDE","session_id":"x","hook_event_name":"UserPromptSubmit"})));
}
#[test]
fn ide_hooks_stamp_international_and_domestic_editions() {
    let home = Home::new();
    let mut c = home.collector("codebuddy-ide");
    assert!(c.ingest_hook(&json!({
        "agent_source":"codebuddy-ide",
        "client":"CodeBuddyIDE",
        "session_id":"intl",
        "hook_event_name":"UserPromptSubmit",
        "agent_edition":"international"
    })));
    assert_eq!(
        c.hub.sessions["codebuddy-ide:intl"]["agentType"],
        "codebuddy"
    );
    assert!(c.ingest_hook(&json!({
        "agent_source":"codebuddy-ide",
        "client":"CodeBuddyIDE",
        "session_id":"cn",
        "hook_event_name":"UserPromptSubmit",
        "agent_edition":"domestic"
    })));
    assert_eq!(
        c.hub.sessions["codebuddy-ide:cn"]["agentType"],
        "codebuddycn"
    );
    std::fs::create_dir_all(home.0.join(".codebuddy")).unwrap();
    std::fs::create_dir_all(home.0.join(".codebuddycn")).unwrap();
    let files = companion_core::adapters::codebuddy_settings_files(&home.0, "");
    assert_eq!(
        files,
        vec![
            home.0.join(".codebuddy/settings.json"),
            home.0.join(".codebuddycn/settings.json")
        ]
    );
    assert_eq!(
        companion_core::adapters::codebuddy_edition(&home.0.join(".codebuddy/settings.json")),
        "international"
    );
    assert_eq!(
        companion_core::adapters::codebuddy_edition(&home.0.join(".codebuddycn/settings.json")),
        "domestic"
    );
}
#[test]
fn ide_hook_merge_preserves_user_handlers_and_is_idempotent() {
    use companion_core::adapters::{merge_codebuddy_ide_hooks, CODEBUDDY_IDE_HOOK_EVENTS};
    let user = json!({"hooks":{"Stop":[{"hooks":[{"command":"echo keep"}]}],"FinalStop":[{"hooks":[{"command":"echo final"}]}]},"custom":true});
    let command = "'/tmp/companion-runtime-v1' hook --source codebuddy-ide";
    let merged = merge_codebuddy_ide_hooks(user.clone(), command).unwrap();
    assert_eq!(merged["hooks"]["Stop"][0], user["hooks"]["Stop"][0]);
    assert_eq!(merged["hooks"]["FinalStop"], user["hooks"]["FinalStop"]);
    assert_eq!(merged["custom"], true);
    assert_eq!(
        merge_codebuddy_ide_hooks(merged.clone(), command).unwrap(),
        merged
    );
    for e in CODEBUDDY_IDE_HOOK_EVENTS {
        assert_eq!(
            merged["hooks"][e].as_array().unwrap().last().unwrap()["hooks"][0]["command"],
            command
        );
    }
    assert!(merge_codebuddy_ide_hooks(json!({"hooks":{"Stop":{}}}), command).is_err());
}

// 这批集成用例验证的是「宿主进程消失 → 会话被标记 aborted」，判定依赖外部 ps，只在 macOS/Linux 上启用；
// 不支持的平台上 observe() 恒为 Unknown，没有可断言的行为，故整组只在支持探测的平台上运行。
#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn host_exit_marks_unfinished_sessions_aborted_and_recovers() {
    let home = Home::new();
    let mut c = home.collector("workbuddy");
    c.ingest_workbuddy_hook(&json!({
        "session_id":"s1","hook_event_name":"UserPromptSubmit","prompt":"build","timestamp":now()
    }));
    assert_eq!(c.hub.sessions["workbuddy:s1"]["status"], json!("running"));
    *c.presence_mut("workbuddy", "workbuddy").unwrap() = HostPresence::with_runner(
        "workbuddy",
        Duration::ZERO,
        2,
        Box::new(|| Ok("/sbin/launchd\n".into())),
    );
    c.presence_mut("workbuddy", "workbuddy")
        .unwrap()
        .note_hook();
    c.poll_workbuddy().unwrap();
    assert_eq!(c.hub.sessions["workbuddy:s1"]["status"], json!("running"));
    c.poll_workbuddy().unwrap();
    assert_eq!(c.hub.sessions["workbuddy:s1"]["status"], json!("aborted"));
    assert_eq!(c.hub.sessions["workbuddy:s1"]["endedBy"], json!("host"));
    assert_eq!(c.hub.sources["workbuddy"]["state"], json!("exited"));
    c.ingest_workbuddy_hook(&json!({
        "session_id":"s1","hook_event_name":"UserPromptSubmit","prompt":"again","timestamp":now()+1
    }));
    assert_eq!(c.hub.sources["workbuddy"]["state"], json!("ok"));
    assert_eq!(c.hub.sessions["workbuddy:s1"]["status"], json!("running"));
    assert!(c.hub.sessions["workbuddy:s1"]["endedBy"].is_null());
}

// The VS Code plugin shares the IDE settings file, so a session belongs to the
// host kind its hook payload named; each kind is probed and ended on its own.
#[cfg(any(target_os = "macos", target_os = "linux"))]
const PS_NO_HOST: &str = "/sbin/launchd\n";
#[cfg(any(target_os = "macos", target_os = "linux"))]
const PS_VSCODE: &str = "/Applications/Visual Studio Code.app/Contents/MacOS/Electron\n";
#[cfg(any(target_os = "macos", target_os = "linux"))]
const PS_IDE: &str = "/Applications/CodeBuddy.app/Contents/MacOS/CodeBuddy\n";

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn ide_presence(kind: &str, output: &'static str) -> HostPresence {
    HostPresence::with_runner(kind, Duration::ZERO, 2, Box::new(move || Ok(output.into())))
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn ide_hook(c: &mut Collector, client: &str, sid: &str, ts: i64) {
    assert!(c.ingest_hook(&json!({
        "agent_source":"codebuddy-ide","client":client,"session_id":sid,"cwd":"/project",
        "timestamp":ts,"hook_event_name":"UserPromptSubmit","prompt":"work"
    })));
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn ide_host_kinds_are_probed_and_ended_independently() {
    let home = Home::new();
    let mut c = home.collector("codebuddy-ide");
    let t = now();
    ide_hook(&mut c, "CodeBuddyIDE", "ide", t);
    ide_hook(&mut c, "VSCode", "code", t);
    assert_eq!(
        c.hub.sessions["codebuddy-ide:ide"]["hostKind"],
        json!("codebuddy-ide")
    );
    assert_eq!(
        c.hub.sessions["codebuddy-ide:code"]["hostKind"],
        json!("vscode")
    );
    // Only VS Code is still running.
    *c.presence_mut("codebuddy-ide", "codebuddy-ide").unwrap() =
        ide_presence("codebuddy-ide", PS_NO_HOST);
    *c.presence_mut("codebuddy-ide", "vscode").unwrap() = ide_presence("vscode", PS_VSCODE);
    for kind in ["codebuddy-ide", "vscode"] {
        c.presence_mut("codebuddy-ide", kind).unwrap().note_hook();
    }
    c.poll_ide().unwrap();
    assert_eq!(c.hub.sources["codebuddy-ide"]["state"], json!("ok"));
    c.poll_ide().unwrap();
    assert_eq!(
        c.presence_mut("codebuddy-ide", "codebuddy-ide")
            .unwrap()
            .observe(),
        Presence::Gone
    );
    assert_eq!(
        c.presence_mut("codebuddy-ide", "vscode").unwrap().observe(),
        Presence::Alive
    );
    assert_eq!(
        c.hub.sessions["codebuddy-ide:ide"]["status"],
        json!("aborted")
    );
    assert_eq!(
        c.hub.sessions["codebuddy-ide:ide"]["endedBy"],
        json!("host")
    );
    assert_eq!(
        c.hub.sessions["codebuddy-ide:code"]["status"],
        json!("running")
    );
    assert!(c.hub.sessions["codebuddy-ide:code"]["endedBy"].is_null());
    // A live kind keeps the source connected even while its sibling exited.
    assert_eq!(c.hub.sources["codebuddy-ide"]["state"], json!("ok"));
    assert_eq!(
        c.hub.sources["codebuddy-ide"]["detail"],
        json!("已连接 CodeBuddy Hook（不读取会话文件）")
    );
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn ide_exit_names_only_the_exited_kind() {
    let home = Home::new();
    let mut c = home.collector("codebuddy-ide");
    ide_hook(&mut c, "CodeBuddyIDE", "ide", now());
    *c.presence_mut("codebuddy-ide", "codebuddy-ide").unwrap() =
        ide_presence("codebuddy-ide", PS_NO_HOST);
    // The VS Code kind never saw a hook: it must stay unknown and unmentioned.
    *c.presence_mut("codebuddy-ide", "vscode").unwrap() = ide_presence("vscode", PS_NO_HOST);
    c.presence_mut("codebuddy-ide", "codebuddy-ide")
        .unwrap()
        .note_hook();
    c.poll_ide().unwrap();
    c.poll_ide().unwrap();
    assert_eq!(
        c.hub.sessions["codebuddy-ide:ide"]["status"],
        json!("aborted")
    );
    assert_eq!(c.hub.sources["codebuddy-ide"]["state"], json!("exited"));
    assert_eq!(
        c.hub.sources["codebuddy-ide"]["detail"],
        json!("CodeBuddy IDE 已退出，未完成的任务已标记中止")
    );
    assert_eq!(
        c.presence_mut("codebuddy-ide", "vscode").unwrap().observe(),
        Presence::Unknown
    );
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn vscode_exit_names_only_the_exited_kind() {
    let home = Home::new();
    let mut c = home.collector("codebuddy-ide");
    ide_hook(&mut c, "VSCode", "code", now());
    *c.presence_mut("codebuddy-ide", "vscode").unwrap() = ide_presence("vscode", PS_NO_HOST);
    *c.presence_mut("codebuddy-ide", "codebuddy-ide").unwrap() =
        ide_presence("codebuddy-ide", PS_NO_HOST);
    c.presence_mut("codebuddy-ide", "vscode")
        .unwrap()
        .note_hook();
    c.poll_ide().unwrap();
    c.poll_ide().unwrap();
    assert_eq!(
        c.hub.sessions["codebuddy-ide:code"]["status"],
        json!("aborted")
    );
    assert_eq!(c.hub.sources["codebuddy-ide"]["state"], json!("exited"));
    assert_eq!(
        c.hub.sources["codebuddy-ide"]["detail"],
        json!("VS Code 已退出，未完成的任务已标记中止")
    );
    assert_eq!(
        c.presence_mut("codebuddy-ide", "codebuddy-ide")
            .unwrap()
            .observe(),
        Presence::Unknown
    );
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn vscode_exit_ends_only_vscode_sessions() {
    let home = Home::new();
    let mut c = home.collector("codebuddy-ide");
    let t = now();
    ide_hook(&mut c, "CodeBuddyIDE", "ide", t);
    ide_hook(&mut c, "VSCode", "code", t);
    *c.presence_mut("codebuddy-ide", "vscode").unwrap() = ide_presence("vscode", PS_NO_HOST);
    *c.presence_mut("codebuddy-ide", "codebuddy-ide").unwrap() =
        ide_presence("codebuddy-ide", PS_IDE);
    for kind in ["codebuddy-ide", "vscode"] {
        c.presence_mut("codebuddy-ide", kind).unwrap().note_hook();
    }
    c.poll_ide().unwrap();
    c.poll_ide().unwrap();
    assert_eq!(
        c.hub.sessions["codebuddy-ide:code"]["status"],
        json!("aborted")
    );
    assert_eq!(
        c.hub.sessions["codebuddy-ide:code"]["endedBy"],
        json!("host")
    );
    assert_eq!(
        c.hub.sessions["codebuddy-ide:ide"]["status"],
        json!("running")
    );
    assert!(c.hub.sessions["codebuddy-ide:ide"]["endedBy"].is_null());
    assert_eq!(c.hub.sources["codebuddy-ide"]["state"], json!("ok"));
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn both_kinds_gone_reports_once_for_both() {
    let home = Home::new();
    let mut c = home.collector("codebuddy-ide");
    let t = now();
    ide_hook(&mut c, "CodeBuddyIDE", "ide", t);
    ide_hook(&mut c, "VSCode", "code", t);
    for kind in ["codebuddy-ide", "vscode"] {
        *c.presence_mut("codebuddy-ide", kind).unwrap() = ide_presence(kind, PS_NO_HOST);
        c.presence_mut("codebuddy-ide", kind).unwrap().note_hook();
    }
    c.poll_ide().unwrap();
    c.poll_ide().unwrap();
    assert_eq!(
        c.hub.sessions["codebuddy-ide:ide"]["status"],
        json!("aborted")
    );
    assert_eq!(
        c.hub.sessions["codebuddy-ide:code"]["status"],
        json!("aborted")
    );
    assert_eq!(c.hub.sources["codebuddy-ide"]["state"], json!("exited"));
    assert_eq!(
        c.hub.sources["codebuddy-ide"]["detail"],
        json!("CodeBuddy IDE 与 VS Code 已退出，未完成的任务已标记中止")
    );
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn unhooked_host_kinds_never_conclude_an_exit() {
    let home = Home::new();
    let mut c = home.collector("codebuddy-ide");
    for kind in ["codebuddy-ide", "vscode"] {
        *c.presence_mut("codebuddy-ide", kind).unwrap() = ide_presence(kind, PS_NO_HOST);
    }
    c.poll_ide().unwrap();
    c.poll_ide().unwrap();
    assert_eq!(c.hub.sources["codebuddy-ide"]["state"], json!("ok"));
    assert_eq!(
        c.hub.sources["codebuddy-ide"]["detail"],
        json!("等待新的 CodeBuddy Hook；不恢复历史会话")
    );
    assert!(c.hub.sessions.is_empty());
}
