mod integrations;
use companion_core::{adapters::Collector, text};
use companion_runtime::{arg_value, call, home};
use serde_json::{json, Value};
use std::{
    collections::VecDeque,
    io::Read,
    path::{Path, PathBuf},
};
fn main() {
    let result = match std::env::args().nth(1).as_deref() {
        Some("hook") => (|| -> Result<(), String> {
            let mut bytes = Vec::new();
            let _ = std::io::stdin().take(1024 * 1024).read_to_end(&mut bytes);
            let agent = arg_value("--source").unwrap_or_else(|| "codex".into());
            // WorkBuddy treats empty stdout as invalid JSON. Codex SessionStart
            // treats JSON stdout as hook output. IDE and WorkBuddy accept `{}`.
            //
            // WorkBuddy / CodeBuddy IDE are observational hooks: print `{}` and
            // spool the event locally, then exit. Waiting on HTTP IPC here used
            // to consume the host's whole 3000ms budget under load (issue #20).
            // The edition comes from the registered command, never probed at hook
            // time (`ps` walking could itself take seconds on Unix).
            if companion_runtime::handle_host_hook(
                &home(),
                &bytes,
                &agent,
                arg_value("--edition").as_deref(),
                &mut std::io::stdout(),
            )? {
                return Ok(());
            }
            if let Ok(mut p) = serde_json::from_slice::<Value>(&bytes) {
                let codex_stop = agent == "codex"
                    && matches!(
                        p["hook_event_name"]
                            .as_str()
                            .or(p["hookEventName"].as_str()),
                        Some("Stop" | "Interrupt" | "SessionEnd")
                    );
                p["agent_source"] = json!(agent);
                if codex_stop {
                    // Stop expects JSON output. Report delivery failures to
                    // Codex instead of silently leaving the session running.
                    if let Err(delivery) = call(&home(), "hook", p.clone()) {
                        match companion_core::codex_recovery::record_offline_terminal(&home(), &p) {
                            Ok(true) => {}
                            Ok(false) => return Err(delivery),
                            Err(persist) => {
                                return Err(format!("{delivery}; 离线结束事件保存失败：{persist}"))
                            }
                        }
                    }
                    println!("{{}}");
                } else {
                    let _ = call(&home(), "hook", p);
                }
            }
            Ok(())
        })(),
        Some("serve") => serve(),
        Some("custom-hook") => custom_hook(),
        Some("uninstall-hooks") => (|| -> Result<(), String> {
            let mut collector = Collector::new(home())?;
            integrations::uninstall_all_hooks(&mut collector)?;
            println!("WorkBuddy Shift hooks 已移除");
            Ok(())
        })(),
        _ => {
            eprintln!(
                "companion-runtime serve | hook | custom-hook | uninstall-hooks [--home <dir>]"
            );
            Ok(())
        }
    };
    if let Err(e) = result {
        eprintln!("{e}");
        std::process::exit(1);
    }
}
fn quote_path(p: &Path) -> String {
    format!("'{}'", p.to_string_lossy().replace('\'', "'\\''"))
}
/// One raw hook payload from a user-configured third-party tool. The source is
/// the `--integration` argument, never the payload, and stdout stays empty so a
/// host that parses our output cannot mistake it for agent instructions.
fn custom_hook() -> Result<(), String> {
    let integration = arg_value("--integration").unwrap_or_default();
    if !companion_core::custom::is_valid_id(&integration) {
        return Err("custom-hook 需要 --integration <id>，id 必须匹配 [a-z][a-z0-9-]{0,63}".into());
    }
    let mut bytes = Vec::new();
    std::io::stdin()
        .take(companion_core::custom::limits::PAYLOAD_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > companion_core::custom::limits::PAYLOAD_BYTES {
        return Err("载荷超过 1 MiB 上限".into());
    }
    let raw: Value =
        serde_json::from_slice(&bytes).map_err(|_| "载荷必须是单个 JSON 值".to_string())?;
    let outcome = call(
        &home(),
        "custom_hook",
        json!({"integration": integration, "payload": raw}),
    )?;
    if outcome["outcome"] == json!("rejected") {
        return Err(format!(
            "事件未接收：{}（{}）",
            text(&outcome["reason"]),
            text(&outcome["detail"])
        ));
    }
    Ok(())
}
fn hook_command(binary: &Path, home: &Path, source: &str, edition: Option<&str>) -> String {
    let mut command = format!(
        "{} hook --home {} --source {}",
        quote_path(binary),
        quote_path(home),
        source
    );
    if let Some(edition) = edition {
        command.push_str(" --edition ");
        command.push_str(edition);
    }
    command
}
fn ensure_hook_binary(home: &Path) -> Result<PathBuf, String> {
    let binary = std::env::current_exe().map_err(|e| e.to_string())?;
    // A stable native hook survives either host being upgraded or removed.
    let hook_dir = home.join(".workbuddy-shift-companion/bin");
    std::fs::create_dir_all(&hook_dir).map_err(|e| e.to_string())?;
    let hook_binary = hook_dir.join(if cfg!(windows) {
        "companion-runtime-v1.exe"
    } else {
        "companion-runtime-v1"
    });
    let temp = hook_dir.join("companion-runtime-v1.new");
    std::fs::copy(&binary, &temp).map_err(|e| e.to_string())?;
    std::fs::rename(&temp, &hook_binary).map_err(|e| e.to_string())?;
    Ok(hook_binary)
}
struct Notifications {
    seen: VecDeque<String>,
    initialized: bool,
}
impl Notifications {
    fn ingest(&mut self, s: &Value) -> Vec<Value> {
        if s["ready"] != true {
            return vec![];
        }
        let mut fresh = Vec::new();
        for e in s["events"].as_array().into_iter().flatten() {
            let id = text(&e["id"]);
            if self.seen.contains(&id) {
                continue;
            }
            self.seen.push_back(id.clone());
            let session = s["sessions"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|s| s["id"] == e["sessionId"]);
            let unresolved = e["kind"] == "wait"
                && session.is_some_and(|s| {
                    s["roundId"] == e["roundId"]
                        && s["pending"].as_array().into_iter().flatten().any(|p| {
                            serde_json::from_str::<Value>(&id)
                                .ok()
                                .is_some_and(|a| a[3] == p["id"])
                        })
                });
            if e["kind"] == "wait" && !unresolved
                || e["kind"] != "wait" && (!self.initialized || e["historical"] == true)
            {
                continue;
            }
            fresh.push(e.clone());
        }
        self.initialized = true;
        while self.seen.len() > 2000 {
            self.seen.pop_front();
        }
        fresh
    }
}
fn ingest_hook_durably(collector: &mut Collector, payload: &Value) -> Result<Value, String> {
    let accepted = collector.ingest_hook(payload);
    if !accepted {
        return Ok(json!(false));
    }
    let sid = payload["session_id"]
        .as_str()
        .or(payload["sessionId"].as_str())
        .unwrap_or("");
    let hidden_codex_child = payload["agent_source"] == "codex"
        && collector
            .hub
            .hidden_codeg_codex_ids
            .contains(sid.trim_start_matches("thr_"));
    if payload["agent_source"] == "codex" && !hidden_codex_child {
        let session = collector
            .hub
            .sessions
            .get(&format!("codex:{sid}"))
            .ok_or("Codex 会话状态缺失")?;
        let live = collector.live.get(sid).ok_or("Codex 轮次状态缺失")?;
        companion_core::codex_recovery::save(
            &collector.home,
            &collector.settings,
            &collector.integrations,
            sid,
            session,
            live,
        )?;
    }
    if payload["agent_source"] == "codex"
        && !hidden_codex_child
        && matches!(
            payload["hook_event_name"]
                .as_str()
                .or(payload["hookEventName"].as_str()),
            Some("Stop" | "Interrupt" | "SessionEnd")
        )
    {
        match companion_core::codex_recovery::record_offline_terminal(&collector.home, payload) {
            Ok(true) => Ok(json!(accepted)),
            Ok(false) => Err("结束事件未被持久保存".into()),
            Err(e) => Err(e),
        }
    } else {
        Ok(json!(accepted))
    }
}
fn collector_request_command(command: &str) -> bool {
    matches!(
        command,
        "settings_set"
            | "settings_check"
            | "integrations_get"
            | "integrations_set"
            | "custom_integrations_get"
            | "custom_integrations_set"
            | "custom_preview"
            | "custom_hook"
            | "session_monitor_close"
    )
}

mod serve;

use serve::serve;

#[cfg(test)]
mod recovery_tests {
    use super::*;
    use companion_core::now;
    #[test]
    fn manual_close_is_routed_to_the_serialized_collector_queue() {
        assert!(collector_request_command("session_monitor_close"));
        assert!(!collector_request_command("unknown"));
    }
    #[test]
    fn terminal_rpc_rejects_a_failed_durable_write() {
        let home =
            std::env::temp_dir().join(format!("codex-write-failure-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&home).unwrap();
        let mut collector = Collector::new(home.clone()).unwrap();
        let started = now();
        assert_eq!(ingest_hook_durably(&mut collector, &json!({"agent_source":"codex","session_id":"tracked","turn_id":"one","hook_event_name":"UserPromptSubmit","timestamp":started})).unwrap(), true);
        let file = home.join(".workbuddy-shift-companion/codex-recovery-v1.json");
        std::fs::remove_file(&file).unwrap();
        std::fs::create_dir(&file).unwrap();
        let stopped = json!({"agent_source":"codex","session_id":"tracked","turn_id":"one","hook_event_name":"Stop","timestamp":started+1});
        assert!(ingest_hook_durably(&mut collector, &stopped).is_err());
        std::fs::remove_dir(&file).unwrap();
        std::fs::remove_dir_all(home).unwrap();
    }
    #[test]
    fn start_rpc_rejects_a_failed_durable_write() {
        let home = std::env::temp_dir().join(format!(
            "codex-start-write-failure-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(home.join(".workbuddy-shift-companion/codex-recovery-v1.json"))
            .unwrap();
        let mut collector = Collector::new(home.clone()).unwrap();
        assert!(ingest_hook_durably(&mut collector, &json!({"agent_source":"codex","session_id":"tracked","turn_id":"one","hook_event_name":"UserPromptSubmit","timestamp":now()})).is_err());
        std::fs::remove_dir_all(home).unwrap();
    }
    #[test]
    fn untracked_codex_stop_remains_harmless() {
        let home =
            std::env::temp_dir().join(format!("codex-untracked-stop-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&home).unwrap();
        let mut collector = Collector::new(home.clone()).unwrap();
        assert_eq!(ingest_hook_durably(&mut collector, &json!({"agent_source":"codex","session_id":"unknown","turn_id":"one","hook_event_name":"Stop","timestamp":now()})).unwrap(), false);
        assert!(collector.hub.sessions.is_empty());
        std::fs::remove_dir_all(home).unwrap();
    }
    #[test]
    fn hidden_codeg_child_stop_does_not_require_recovery_record() {
        let home =
            std::env::temp_dir().join(format!("codex-hidden-child-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&home).unwrap();
        let mut collector = Collector::new(home.clone()).unwrap();
        collector.hub.hide_codeg_child_codex("thr-child");
        let started = now();
        assert_eq!(ingest_hook_durably(&mut collector, &json!({"agent_source":"codex","session_id":"thr-child","turn_id":"one","hook_event_name":"UserPromptSubmit","timestamp":started})).unwrap(), true);
        assert_eq!(ingest_hook_durably(&mut collector, &json!({"agent_source":"codex","session_id":"thr-child","turn_id":"one","hook_event_name":"Stop","timestamp":started+1})).unwrap(), true);
        assert!(collector.hub.snapshot()["sessions"]
            .as_array()
            .unwrap()
            .is_empty());
        assert!(!home
            .join(".workbuddy-shift-companion/codex-recovery-v1.json")
            .exists());
        std::fs::remove_dir_all(home).unwrap();
    }
}
