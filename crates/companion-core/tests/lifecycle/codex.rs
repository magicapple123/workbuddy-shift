use super::*;
use companion_core::{hub::Hub, now};

#[test]
fn anonymous_approval_reconciles_replayed_continuation_without_clearing_questions() {
    let mut h = Hub::new();
    for ev in [
        json!({"type":"start","roundId":"r","ts":100}),
        json!({"type":"wait","callId":"perm:110","ts":110}),
        json!({"type":"wait","callId":"question","ts":111}),
        json!({"type":"tokens","ts":200}),
        json!({"type":"activity","ts":109}),
    ] {
        h.ingest(companion_core::merge(
            json!({"source":"codex","sessionId":"x"}),
            ev,
        ));
    }
    assert_eq!(
        h.sessions["codex:x"]["pending"].as_array().unwrap().len(),
        2
    );
    h.ingest(json!({"source":"codex","sessionId":"x","type":"activity","ts":120}));
    assert_eq!(h.sessions["codex:x"]["pending"][0]["id"], "question");
    assert_eq!(
        h.sessions["codex:x"]["pending"].as_array().unwrap().len(),
        1
    );
    assert_eq!(h.sessions["codex:x"]["status"], "wait");
    h.ingest(
        json!({"source":"codex","sessionId":"x","type":"resolve","callId":"question","ts":201}),
    );
    h.ingest(json!({"source":"codex","sessionId":"x","type":"wait","callId":"perm:210","ts":210}));
    h.ingest(json!({"source":"codex","sessionId":"x","type":"tokens","ts":250}));
    h.ingest(json!({"source":"codex","sessionId":"x","type":"activity","ts":220}));
    assert_eq!(h.sessions["codex:x"]["status"], "running");
    assert_eq!(h.sessions["codex:x"]["updatedAt"], 250);
}
#[test]
fn rejects_stale_round_end_and_resolves_wait() {
    let mut h = Hub::new();
    let t = now();
    for ev in [
        json!({"type":"start","roundId":"a","ts":t}),
        json!({"type":"wait","callId":"q","ts":t+1}),
        json!({"type":"start","roundId":"b","ts":t+2}),
        json!({"type":"end","roundId":"a","status":"done","ts":t+3}),
    ] {
        h.ingest(companion_core::merge(
            json!({"source":"codex","sessionId":"x"}),
            ev,
        ));
    }
    assert_eq!(h.sessions["codex:x"]["status"], "running");
    assert_eq!(h.sessions["codex:x"]["pending"], json!([]));
}
#[test]
fn hosted_codex_is_hidden_but_wait_survives_staleness() {
    let mut h = Hub::new();
    h.ingest(json!({"source":"codex","sessionId":"x","type":"wait","callId":"q","ts":1}));
    h.ingest(json!({"source":"codeg","sessionId":"y","externalId":"thr_x","type":"meta"}));
    assert_eq!(h.snapshot()["sessions"].as_array().unwrap().len(), 1);
    h.sessions.remove("codeg:y");
    assert_eq!(h.snapshot()["sessions"][0]["status"], "wait");
}

#[test]
fn codex_session_start_source_is_not_treated_as_agent_source() {
    let home = Home::new();
    let mut c = home.collector("codex");
    assert!(c.ingest_hook(&json!({
        "session_id":"x",
        "hook_event_name":"SessionStart",
        "source":"startup"
    })));
    assert_eq!(c.hub.sessions["codex:x"]["status"], "running");
    assert!(c.ingest_hook(&json!({
        "session_id":"x",
        "hook_event_name":"UserPromptSubmit",
        "prompt":"hello",
        "source":"resume"
    })));
    assert_eq!(c.hub.sessions["codex:x"]["title"], "hello");
}

#[test]
fn codex_hook_lifecycle_restores_only_tracked_sessions() {
    let home = Home::new();
    let mut c = home.collector("codex");
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("../fixtures/codex-hooks.json")).unwrap();
    let t = now();
    let transcript = home.0.join("transcript.jsonl");
    std::fs::write(&transcript, format!("{}\n",json!({"type":"event_msg","timestamp":t+9999,"payload":{"type":"task_complete","turn_id":"r"}}))).unwrap();
    let checkpoint = home.0.join(".workbuddy-shift-companion/codex-resume.json");
    let old = json!({"sessions":[{"source":"codex","sessionId":"old","id":"codex:old","status":"running","roundId":"r","updatedAt":t,"steps":[],"pending":[]}]}).to_string();
    std::fs::write(&checkpoint, &old).unwrap();
    for (i, case) in fixture.as_array().unwrap().iter().enumerate() {
        let hook = companion_core::merge(
            json!({"session_id":"x","turn_id":"r","cwd":"/project","timestamp":t+i as i64,"transcript_path":transcript}),
            case["hook"].clone(),
        );
        c.ingest_hook(&hook);
        c.poll();
        assert_eq!(
            c.hub.sessions["codex:x"]["status"], case["status"],
            "case {i}"
        );
        assert_eq!(
            c.hub.sessions["codex:x"]["pending"]
                .as_array()
                .unwrap()
                .len(),
            case["pending"].as_u64().unwrap() as usize,
            "case {i}"
        );
    }
    assert_eq!(std::fs::read_to_string(&checkpoint).unwrap(), old);
    let mut restarted = Collector::new(home.0.clone()).unwrap();
    restarted.poll();
    assert_eq!(restarted.hub.sessions.len(), 1);
    assert_eq!(restarted.hub.sessions["codex:x"]["status"], "done");
    assert_eq!(restarted.hub.sessions["codex:x"]["recovered"], true);
    assert!(restarted.hub.snapshot()["events"]
        .as_array()
        .unwrap()
        .is_empty());
    assert!(!restarted.hub.sessions.contains_key("codex:old"));
}

#[test]
fn codex_offline_stop_wins_over_saved_running_and_new_round_can_start() {
    let home = Home::new();
    let mut c = home.collector("codex");
    let started = now();
    assert!(c.ingest_hook(&json!({"session_id":"tracked","turn_id":"one","hook_event_name":"UserPromptSubmit","prompt":"Visible title","timestamp":started})));
    drop(c);
    let stopped = started + 100;
    assert!(companion_core::codex_recovery::record_offline_terminal(&home.0, &json!({"session_id":"tracked","turn_id":"one","hook_event_name":"Stop","timestamp":stopped})).unwrap());
    assert!(!companion_core::codex_recovery::record_offline_terminal(&home.0, &json!({"session_id":"unknown","turn_id":"one","hook_event_name":"Stop","timestamp":stopped})).unwrap());
    let mut restarted = Collector::new(home.0.clone()).unwrap();
    restarted.poll();
    assert_eq!(restarted.hub.sessions["codex:tracked"]["status"], "done");
    assert_eq!(restarted.hub.sessions["codex:tracked"]["endedAt"], stopped);
    assert!(!restarted.ingest_hook(&json!({"session_id":"tracked","turn_id":"one","hook_event_name":"SessionStart","timestamp":started})));
    assert_eq!(restarted.hub.sessions["codex:tracked"]["status"], "done");
    assert!(restarted.ingest_hook(&json!({"session_id":"tracked","turn_id":"two","hook_event_name":"UserPromptSubmit","timestamp":stopped+1})));
    assert_eq!(restarted.hub.sessions["codex:tracked"]["status"], "running");
    assert_eq!(restarted.hub.sessions["codex:tracked"]["roundId"], "two");
    assert!(!restarted.ingest_hook(&json!({"session_id":"tracked","turn_id":"one","hook_event_name":"UserPromptSubmit","timestamp":stopped})));
    assert_eq!(restarted.hub.sessions["codex:tracked"]["roundId"], "two");
}

#[test]
fn codex_offline_stop_with_equal_timestamp_wins_over_running_memory() {
    let home = Home::new();
    let mut collector = home.collector("codex");
    let timestamp = now();
    assert!(collector.ingest_hook(&json!({"session_id":"tracked","turn_id":"one","hook_event_name":"UserPromptSubmit","timestamp":timestamp})));
    assert!(companion_core::codex_recovery::record_offline_terminal(&home.0, &json!({"session_id":"tracked","turn_id":"one","hook_event_name":"Stop","timestamp":timestamp})).unwrap());
    collector.poll();
    assert_eq!(collector.hub.sessions["codex:tracked"]["status"], "done");
    assert!(!collector.ingest_hook(&json!({"session_id":"tracked","turn_id":"one","hook_event_name":"PreToolUse","timestamp":timestamp})));
    assert_eq!(collector.hub.sessions["codex:tracked"]["status"], "done");
}

#[test]
fn codex_monitor_close_forgets_only_matching_round_and_next_hook_recreates_it() {
    let home = Home::new();
    let mut collector = home.collector("codex");
    let timestamp = now();
    assert!(collector.ingest_hook(&json!({"session_id":"tracked","turn_id":"one","hook_event_name":"UserPromptSubmit","timestamp":timestamp})));
    assert!(collector.ingest_hook(&json!({"session_id":"other","turn_id":"other-round","hook_event_name":"UserPromptSubmit","timestamp":timestamp})));
    assert!(collector.ingest_hook(&json!({"session_id":"tracked","turn_id":"one","hook_event_name":"PreToolUse","tool_name":"request_user_input","tool_use_id":"ask","timestamp":timestamp+1})));
    assert!(collector
        .hub
        .events
        .iter()
        .any(|event| event["sessionId"] == "codex:tracked"));
    assert_eq!(
        collector
            .request(
                "session_monitor_close",
                &json!({"source":"codex","sessionId":"tracked","roundId":"stale"})
            )
            .unwrap(),
        json!({"closed":false})
    );
    assert_eq!(collector.hub.sessions["codex:tracked"]["roundId"], "one");
    assert_eq!(
        collector
            .request(
                "session_monitor_close",
                &json!({"source":"codex","sessionId":"tracked","roundId":"one"})
            )
            .unwrap(),
        json!({"closed":true})
    );
    assert!(!collector.hub.sessions.contains_key("codex:tracked"));
    assert!(!collector.live.contains_key("tracked"));
    assert!(!collector
        .hub
        .events
        .iter()
        .any(|event| event["sessionId"] == "codex:tracked" && event["roundId"] == "one"));
    assert!(collector.hub.sessions.contains_key("codex:other"));
    drop(collector);
    assert!(!companion_core::codex_recovery::record_offline_terminal(&home.0, &json!({"session_id":"tracked","turn_id":"one","hook_event_name":"Stop","timestamp":timestamp+2})).unwrap());
    let mut restarted = Collector::new(home.0.clone()).unwrap();
    assert!(!restarted.hub.sessions.contains_key("codex:tracked"));
    assert!(restarted.hub.sessions.contains_key("codex:other"));
    assert!(!restarted.ingest_hook(&json!({"session_id":"tracked","turn_id":"one","hook_event_name":"Stop","timestamp":timestamp+2})));
    assert!(restarted.ingest_hook(&json!({"session_id":"tracked","turn_id":"one","hook_event_name":"PreToolUse","tool_name":"functions.exec","tool_use_id":"next","timestamp":timestamp+3})));
    assert_eq!(restarted.hub.sessions["codex:tracked"]["status"], "running");
}

#[test]
fn codex_monitor_close_error_preserves_memory_and_recovery() {
    let home = Home::new();
    let mut collector = home.collector("codex");
    assert!(collector.ingest_hook(&json!({"session_id":"tracked","turn_id":"one","hook_event_name":"UserPromptSubmit","timestamp":now()})));
    let lock = home
        .0
        .join(".workbuddy-shift-companion/codex-recovery-v1.lock");
    std::fs::remove_file(&lock).unwrap();
    std::fs::create_dir(&lock).unwrap();
    assert!(collector
        .request(
            "session_monitor_close",
            &json!({"source":"codex","sessionId":"tracked","roundId":"one"})
        )
        .is_err());
    assert!(collector.hub.sessions.contains_key("codex:tracked"));
    assert!(collector.live.contains_key("tracked"));
    std::fs::remove_dir(&lock).unwrap();
    assert!(Collector::new(home.0.clone())
        .unwrap()
        .hub
        .sessions
        .contains_key("codex:tracked"));
}

#[test]
fn codex_monitor_close_forgets_legacy_memory_without_a_recovery_record() {
    let home = Home::new();
    let mut collector = home.collector("codex");
    let timestamp = now();
    assert!(collector.ingest_hook(&json!({"session_id":"tracked","turn_id":"one","hook_event_name":"UserPromptSubmit","timestamp":timestamp})));
    let store = home
        .0
        .join(".workbuddy-shift-companion/codex-recovery-v1.json");
    std::fs::remove_file(&store).unwrap();
    assert_eq!(
        collector
            .request(
                "session_monitor_close",
                &json!({"source":"codex","sessionId":"tracked","roundId":"one"})
            )
            .unwrap(),
        json!({"closed":true})
    );
    assert!(!collector.hub.sessions.contains_key("codex:tracked"));
    assert!(!Collector::new(home.0.clone())
        .unwrap()
        .hub
        .sessions
        .contains_key("codex:tracked"));
}

#[test]
fn codex_recovery_respects_policy_path_age_and_corruption() {
    let home = Home::new();
    let mut c = home.collector("codex");
    let timestamp = now();
    c.ingest_hook(&json!({"session_id":"tracked","turn_id":"one","hook_event_name":"UserPromptSubmit","timestamp":timestamp}));
    drop(c);
    let mut settings = settings::defaults();
    settings["sources"]["codex"]["path"] = json!("~/other-codex");
    atomic_json(
        &home.0.join(".workbuddy-shift-companion/settings.json"),
        &settings,
    )
    .unwrap();
    let changed = Collector::new(home.0.clone()).unwrap();
    assert!(changed.hub.sessions.is_empty());
    assert!(companion_core::codex_recovery::record_offline_terminal(&home.0, &json!({"session_id":"tracked","turn_id":"one","hook_event_name":"Stop","timestamp":timestamp+1})).is_ok_and(|accepted| !accepted));
    settings["sources"]["codex"]["enabled"] = json!(false);
    atomic_json(
        &home.0.join(".workbuddy-shift-companion/settings.json"),
        &settings,
    )
    .unwrap();
    assert!(Collector::new(home.0.clone())
        .unwrap()
        .hub
        .sessions
        .is_empty());
    assert!(companion_core::codex_recovery::record_offline_terminal(
        &home.0,
        &json!({"session_id":"tracked","hook_event_name":"Stop"})
    )
    .is_err());
    settings["sources"]["codex"] = json!({"enabled":true,"path":""});
    atomic_json(
        &home.0.join(".workbuddy-shift-companion/settings.json"),
        &settings,
    )
    .unwrap();
    let file = home
        .0
        .join(".workbuddy-shift-companion/codex-recovery-v1.json");
    std::fs::write(&file, "broken").unwrap();
    assert!(Collector::new(home.0.clone())
        .unwrap()
        .hub
        .sessions
        .is_empty());
    atomic_json(&file, &json!({"version":1,"path":"","sessions":{"tracked":{"session":{"id":"codex:tracked","source":"codex","sessionId":"tracked","roundId":"one","status":"running","updatedAt":timestamp},"live":{"roundId":"one","ended":false}}}})).unwrap();
    assert!(
        Collector::new(home.0.clone())
            .unwrap()
            .hub
            .sessions
            .is_empty(),
        "a recent but incomplete record is discarded"
    );
    atomic_json(&file, &json!({"version":1,"path":"","sessions":{"tracked":{"session":{"id":"codex:tracked","source":"codex","sessionId":"tracked","roundId":"one","status":"done","updatedAt":timestamp-3_600_001,"endedAt":timestamp-3_600_001},"live":{"roundId":"one","ended":true}}}})).unwrap();
    assert!(Collector::new(home.0.clone())
        .unwrap()
        .hub
        .sessions
        .is_empty());
}

#[test]
fn uninstalling_codex_clears_recovery_and_visible_sessions() {
    let home = Home::new();
    let mut collector = home.collector("codex");
    collector.ingest_hook(&json!({"session_id":"tracked","turn_id":"one","hook_event_name":"UserPromptSubmit","timestamp":now()}));
    assert!(home
        .0
        .join(".workbuddy-shift-companion/codex-recovery-v1.json")
        .exists());
    collector.set_integration_automatic("codex", false).unwrap();
    assert!(collector.hub.sessions.is_empty());
    assert!(!home
        .0
        .join(".workbuddy-shift-companion/codex-recovery-v1.json")
        .exists());
    assert!(Collector::new(home.0.clone())
        .unwrap()
        .hub
        .sessions
        .is_empty());
}

#[test]
fn permission_checks_do_not_generate_wait_notifications() {
    let home = Home::new();
    let mut c = home.collector("codex");
    for (event, tool, id) in [
        ("UserPromptSubmit", "", ""),
        ("PermissionRequest", "Bash", "a"),
    ] {
        c.ingest_hook(&json!({"session_id":"x","turn_id":"r","hook_event_name":event,"tool_name":tool,"tool_use_id":id}));
    }
    assert_eq!(c.hub.sessions["codex:x"]["status"], "running");
    assert_eq!(
        c.hub.sessions["codex:x"]["permissionChecks"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        c.hub.sessions["codex:x"]["permissionChecks"][0]["id"],
        "perm:a"
    );
    assert!(c.hub.sessions["codex:x"]["permissionChecks"][0]["ts"].is_number());
    assert!(c.hub.snapshot()["events"].as_array().unwrap().is_empty());
    c.ingest_hook(&json!({"session_id":"x","turn_id":"r","hook_event_name":"PreToolUse","tool_name":"request_user_input","tool_use_id":"q"}));
    c.ingest_hook(&json!({"session_id":"x","turn_id":"r","hook_event_name":"PostToolUse","tool_name":"Bash","tool_use_id":"a"}));
    assert_eq!(c.hub.sessions["codex:x"]["status"], "wait");
    assert_eq!(c.hub.sessions["codex:x"]["permissionChecks"], json!([]));
    assert_eq!(c.hub.snapshot()["events"].as_array().unwrap().len(), 1);
}

#[test]
fn internal_codex_tasks_never_reappear_on_late_hooks() {
    let home = Home::new();
    let mut c = home.collector("codex");
    let prompts: Vec<String> = serde_json::from_str(include_str!(
        "../../../../companion-ui/src/monitor/codex-internal-prompts.json"
    ))
    .unwrap();
    for (i, prompt) in prompts.iter().enumerate() {
        let id = format!("internal-{i}");
        c.ingest_hook(&json!({"session_id":id,"hook_event_name":"SessionStart"}));
        assert!(!c.ingest_hook(
            &json!({"session_id":id,"hook_event_name":"UserPromptSubmit","prompt":prompt})
        ));
        for event in ["PreToolUse", "PostToolUse", "Stop", "SessionStart"] {
            assert!(!c.ingest_hook(&json!({"session_id":id,"hook_event_name":event})));
        }
        assert!(!c.hub.sessions.contains_key(&format!("codex:{id}")));
    }
    assert!(c.ingest_hook(&json!({"session_id":"user","hook_event_name":"UserPromptSubmit","prompt":"帮我实现个性化建议和 memory consolidation"})));
    assert!(c.hub.sessions.contains_key("codex:user"));
}

#[test]
fn codex_async_questions_wait_and_clear_by_reply_prompt_or_end() {
    let home = Home::new();
    let mut c = home.collector("codex");
    let mut ts = now();
    let mut hook = |c: &mut Collector, p: serde_json::Value| {
        ts += 1;
        c.ingest_hook(&companion_core::merge(
            json!({"session_id":"x","turn_id":"r","timestamp":ts}),
            p,
        ));
    };
    let reply = |tool: &str| {
        json!({"hook_event_name":"UserPromptSubmit","prompt":format!(
            "<send_user_message_question_reply>\n{}\n</send_user_message_question_reply>",
            json!([{"questionItemId":json!([tool, tool, 0]).to_string(),"answer":"甲"}]).to_string())})
    };
    hook(
        &mut c,
        json!({"hook_event_name":"UserPromptSubmit","prompt":"Start"}),
    );
    // Async questions now notify like synchronous ones, and their own completion
    // must not clear them.
    for tool in [
        "request_user_input_async",
        "functions.request_user_input_async",
        "mcp__codex__request_user_input_async",
    ] {
        hook(
            &mut c,
            json!({
                "hook_event_name":"PreToolUse","tool_name":tool,"tool_use_id":tool,
                "tool_input":{"questions":[{"title":"偏好","options":[{"label":"A","description":"甲"}]}]}
            }),
        );
        assert_eq!(c.hub.sessions["codex:x"]["status"], "wait");
        let pending = c.hub.sessions["codex:x"]["pending"]
            .as_array()
            .unwrap()
            .clone();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0]["id"], tool);
        assert_eq!(pending[0]["optional"], json!(true));
        assert_eq!(pending[0]["text"], "偏好");
        assert_eq!(
            pending[0]["questions"][0]["options"][0]["description"],
            "甲"
        );
        hook(
            &mut c,
            json!({"hook_event_name":"PostToolUse","tool_name":tool,"tool_use_id":tool}),
        );
        assert_eq!(
            c.hub.sessions["codex:x"]["pending"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        // The answer envelope clears exactly that call and never titles the session.
        hook(&mut c, reply(tool));
        assert_eq!(c.hub.sessions["codex:x"]["pending"], json!([]));
        assert_eq!(c.hub.sessions["codex:x"]["status"], "running");
        assert_eq!(c.hub.sessions["codex:x"]["title"], "Start");
    }
    // A synchronous question survives an async completion; only a real prompt
    // clears the unanswered async item, and only it is optional.
    hook(
        &mut c,
        json!({"hook_event_name":"PreToolUse","tool_name":"functions.request_user_input","tool_use_id":"sync"}),
    );
    hook(
        &mut c,
        json!({"hook_event_name":"PreToolUse","tool_name":"functions.request_user_input_async","tool_use_id":"waiting"}),
    );
    hook(
        &mut c,
        json!({"hook_event_name":"PostToolUse","tool_name":"functions.request_user_input_async","tool_use_id":"waiting"}),
    );
    let pending = c.hub.sessions["codex:x"]["pending"]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(pending.len(), 2);
    assert_eq!(pending[0]["id"], "sync");
    assert_eq!(pending[0]["optional"], json!(null));
    assert_eq!(pending[1]["id"], "waiting");
    assert_eq!(pending[1]["optional"], json!(true));
    // Another internal envelope is neither a title nor an answer.
    hook(
        &mut c,
        json!({"hook_event_name":"UserPromptSubmit","prompt":"<in-app-browser-context url=\"x\">ctx</in-app-browser-context>"}),
    );
    assert_eq!(
        c.hub.sessions["codex:x"]["pending"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(c.hub.sessions["codex:x"]["title"], "Start");
    hook(
        &mut c,
        json!({"hook_event_name":"UserPromptSubmit","prompt":"Continue"}),
    );
    let pending = c.hub.sessions["codex:x"]["pending"]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0]["id"], "sync");
    assert_eq!(c.hub.sessions["codex:x"]["title"], "Continue");
    // Missing tool_input falls back to the generic wait copy.
    hook(
        &mut c,
        json!({"hook_event_name":"PreToolUse","tool_name":"functions.request_user_input_async","tool_use_id":"bare"}),
    );
    assert_eq!(
        c.hub.sessions["codex:x"]["pending"][1]["text"],
        "等待用户输入"
    );
    // An empty prompt is not a real message: it neither titles nor answers.
    hook(
        &mut c,
        json!({"hook_event_name":"UserPromptSubmit","prompt":"   "}),
    );
    assert_eq!(
        c.hub.sessions["codex:x"]["pending"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(c.hub.sessions["codex:x"]["title"], "Continue");
    // One envelope can clear several call ids at once.
    hook(
        &mut c,
        json!({"hook_event_name":"PreToolUse","tool_name":"functions.request_user_input_async","tool_use_id":"second"}),
    );
    let multi = json!([
        {"questionItemId": json!(["functions.request_user_input_async","bare",0]).to_string(), "answer":"A"},
        {"questionItemId": json!(["functions.request_user_input_async","second",1]).to_string(), "answer":"B"}
    ]);
    hook(
        &mut c,
        json!({"hook_event_name":"UserPromptSubmit","prompt":format!(
            "<send_user_message_question_reply>\n{multi}\n</send_user_message_question_reply>"
        )}),
    );
    let pending = c.hub.sessions["codex:x"]["pending"]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0]["id"], "sync");
    // Evicting call bookkeeping must not keep an unanswered async wait.
    hook(
        &mut c,
        json!({"hook_event_name":"PreToolUse","tool_name":"functions.request_user_input_async","tool_use_id":"gone"}),
    );
    c.live.get_mut("x").unwrap()["calls"] = json!({});
    hook(
        &mut c,
        json!({"hook_event_name":"UserPromptSubmit","prompt":"Still here"}),
    );
    let pending = c.hub.sessions["codex:x"]["pending"]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0]["id"], "sync");
    assert_eq!(c.hub.sessions["codex:x"]["title"], "Still here");
    // The round end clears what is left.
    hook(&mut c, json!({"hook_event_name":"Stop"}));
    assert_eq!(c.hub.sessions["codex:x"]["status"], "done");
    assert_eq!(c.hub.sessions["codex:x"]["pending"], json!([]));
}

#[test]
fn optional_waits_expire_lazily_without_clearing_synchronous_items() {
    let mut h = Hub::new();
    let t = now();
    for ev in [
        json!({"type":"start","roundId":"r","ts":t}),
        json!({"type":"wait","callId":"expired","tool":"ask","text":"旧问题","optional":true,"roundId":"r","ts":t-61_000}),
        json!({"type":"wait","callId":"live","tool":"ask","text":"新问题","optional":true,"roundId":"r","ts":t-30_000}),
        json!({"type":"wait","callId":"sync","tool":"ask","text":"同步问题","roundId":"r","ts":t-90_000}),
    ] {
        h.ingest(companion_core::merge(
            json!({"source":"codex","sessionId":"x"}),
            ev,
        ));
    }
    let s = h.snapshot()["sessions"][0].clone();
    assert_eq!(s["status"], "wait");
    assert_eq!(s["pending"].as_array().unwrap().len(), 2);
    assert_eq!(s["pending"][0]["id"], "live");
    assert_eq!(s["pending"][0]["optional"], json!(true));
    assert_eq!(s["pending"][1]["id"], "sync");
    assert_eq!(s["pending"][1]["optional"], json!(null));
    // Expiry is decided on read, so the stored round still holds the old item.
    assert_eq!(
        h.sessions["codex:x"]["pending"].as_array().unwrap().len(),
        3
    );
    let mut only = Hub::new();
    for ev in [
        json!({"type":"start","roundId":"r","ts":t}),
        json!({"type":"wait","callId":"expired","tool":"ask","text":"旧问题","optional":true,"roundId":"r","ts":t-61_000}),
    ] {
        only.ingest(companion_core::merge(
            json!({"source":"codex","sessionId":"y"}),
            ev,
        ));
    }
    let expired = only.snapshot()["sessions"][0].clone();
    assert_eq!(expired["status"], "running");
    assert_eq!(expired["pending"], json!([]));
    assert_eq!(
        only.sessions["codex:y"]["pending"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

const OBSERVED_MEMORY_PROMPT: &str = "Consolidate the supplied rollout summaries into `memory_summary.md` so another agent understands the user, finds relevant prior work, and continues correctly.";

#[test]
fn observed_memory_prompt_is_suppressed_with_whitespace_and_late_hooks() {
    let home = Home::new();
    let mut c = home.collector("codex");
    let prompt = format!(
        "  {}\nAdditional summaries follow.",
        OBSERVED_MEMORY_PROMPT.replace(' ', "\n\t")
    );
    assert!(!c.ingest_hook(
        &json!({"session_id":"memory","hook_event_name":"UserPromptSubmit","prompt":prompt})
    ));
    for event in [
        "PreToolUse",
        "PostToolUse",
        "PermissionRequest",
        "Stop",
        "Interrupt",
        "SessionEnd",
        "SessionStart",
    ] {
        assert!(!c.ingest_hook(&json!({"session_id":"memory","hook_event_name":event,"tool_name":"request_user_input","tool_use_id":"late"})));
    }
    assert!(c.hub.sessions.is_empty());
    assert!(c.hub.snapshot()["events"].as_array().unwrap().is_empty());
    for (i, prompt) in [
        "帮我整理 memory_summary.md".to_owned(),
        format!("Explain this: {OBSERVED_MEMORY_PROMPT}"),
        "Consolidate the supplied rollout summaries for my project".to_owned(),
    ]
    .iter()
    .enumerate()
    {
        assert!(c.ingest_hook(&json!({"session_id":format!("user-{i}"),"hook_event_name":"UserPromptSubmit","prompt":prompt})));
    }
    assert_eq!(c.hub.sessions.len(), 3);
}

#[test]
fn legacy_memory_recovery_is_hidden_before_late_hooks() {
    let home = Home::new();
    let mut c = home.collector("codex");
    let timestamp = now();
    // Build a valid old-runtime recovery record without passing its prompt
    // through the updated ingestion filter.
    for sid in ["memory", "user"] {
        c.ingest_hook(&json!({"session_id":sid,"turn_id":"one","hook_event_name":"UserPromptSubmit","prompt":"ordinary user task","timestamp":timestamp}));
        c.ingest_hook(&json!({"session_id":sid,"turn_id":"one","hook_event_name":"Stop","timestamp":timestamp+1}));
    }
    let store = home
        .0
        .join(".workbuddy-shift-companion/codex-recovery-v1.json");
    let mut data: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&store).unwrap()).unwrap();
    data["sessions"]["memory"]["session"]["title"] = json!(OBSERVED_MEMORY_PROMPT);
    data["sessions"]["user"]["session"]["title"] =
        json!(format!("Explain this: {OBSERVED_MEMORY_PROMPT}"));
    atomic_json(&store, &data).unwrap();
    drop(c);
    let mut restored = Collector::new(home.0.clone()).unwrap();
    assert!(!restored.hub.sessions.contains_key("codex:memory"));
    assert!(restored.hub.sessions.contains_key("codex:user"));
    assert_eq!(restored.live["memory"]["internal"], true);
    for event in [
        "PreToolUse",
        "PostToolUse",
        "PermissionRequest",
        "Stop",
        "SessionStart",
    ] {
        assert!(!restored.ingest_hook(&json!({"session_id":"memory","hook_event_name":event,"tool_name":"request_user_input","tool_use_id":"late"})));
        assert!(!restored.hub.sessions.contains_key("codex:memory"));
    }
    assert_eq!(restored.hub.sessions.len(), 1);
    assert!(restored.hub.snapshot()["events"]
        .as_array()
        .unwrap()
        .is_empty());
    drop(restored);
    let mut again = Collector::new(home.0.clone()).unwrap();
    assert!(again.hub.sessions.contains_key("codex:user"));
    assert!(!again.ingest_hook(&json!({"session_id":"memory","hook_event_name":"PreToolUse","tool_name":"request_user_input","tool_use_id":"later"})));
    assert!(!again.ingest_hook(&json!({"session_id":"memory","hook_event_name":"Stop"})));
    assert!(!again.hub.sessions.contains_key("codex:memory"));
    // Recognition evidence retains the original bounded expiry time.
    let after: serde_json::Value = serde_json::from_slice(&std::fs::read(&store).unwrap()).unwrap();
    assert_eq!(after["sessions"]["memory"], data["sessions"]["memory"]);
}
