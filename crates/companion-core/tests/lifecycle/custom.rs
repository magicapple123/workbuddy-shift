use super::*;
use companion_core::{now, tail::Tail};

#[test]
fn tail_preserves_utf8_partial_records_and_rotation() {
    use std::io::Write;
    let home = Home::new();
    let p = home.0.join("a.jsonl");
    let bytes = "{\"text\":\"中文\"}\n".as_bytes();
    std::fs::write(&p, &bytes[..11]).unwrap();
    let mut tail = Tail::default();
    assert!(tail.pump(&p, 512).unwrap().1.is_empty());
    std::fs::OpenOptions::new()
        .append(true)
        .open(&p)
        .unwrap()
        .write_all(&bytes[11..])
        .unwrap();
    assert_eq!(tail.pump(&p, 512).unwrap().1[0]["text"], "中文");
    std::fs::write(&p, b"{}\n").unwrap();
    assert!(tail.pump(&p, 512).unwrap().0);
}
#[test]
fn settings_reject_invalid_paths_without_mutating() {
    let mut s = settings::defaults();
    s["sources"]["codex"]["path"] = json!("relative/path");
    assert!(settings::validate(&s).is_err());
    s = settings::defaults();
    s["schedule"]["end"] = json!("08:00");
    assert!(settings::validate(&s).is_err());
}

#[test]
fn session_monitor_close_targets_only_selected_source_session_and_round() {
    let home = Home::new();
    let mut collector = home.collector("workbuddy");
    for sid in ["selected", "other"] {
        assert!(collector.ingest_workbuddy_hook(&json!({
            "session_id":sid,"hook_event_name":"UserPromptSubmit","turn_id":"round-one"
        })));
    }
    collector.hub.ingest(json!({"source":"codebuddy-ide","sessionId":"selected","type":"start","roundId":"round-one","ts":now()}));
    assert_eq!(
        collector
            .request(
                "session_monitor_close",
                &json!({"source":"codebuddy-ide","sessionId":"selected","roundId":"wrong"})
            )
            .unwrap(),
        json!({"closed":false})
    );
    assert_eq!(
        collector
            .request(
                "session_monitor_close",
                &json!({"source":"workbuddy","sessionId":"selected","roundId":"round-one"})
            )
            .unwrap(),
        json!({"closed":true})
    );
    assert!(!collector.hub.sessions.contains_key("workbuddy:selected"));
    assert!(!collector.workbuddy_live.contains_key("selected"));
    assert!(collector.hub.sessions.contains_key("workbuddy:other"));
    assert!(collector
        .hub
        .sessions
        .contains_key("codebuddy-ide:selected"));
    assert!(!collector.ingest_workbuddy_hook(
        &json!({"session_id":"selected","hook_event_name":"Stop","turn_id":"round-one"})
    ));
    assert!(!collector.hub.sessions.contains_key("workbuddy:selected"));
    assert!(collector.ingest_workbuddy_hook(
        &json!({"session_id":"selected","hook_event_name":"UserPromptSubmit","turn_id":"round-two"})
    ));
    assert_eq!(
        collector.hub.sessions["workbuddy:selected"]["roundId"],
        "round-two"
    );
    assert_eq!(
        collector
            .request(
                "session_monitor_close",
                &json!({"source":"codebuddy-ide","sessionId":"selected","roundId":"round-one"})
            )
            .unwrap(),
        json!({"closed":true})
    );
    assert!(!collector
        .hub
        .sessions
        .contains_key("codebuddy-ide:selected"));
}

#[test]
fn session_monitor_close_handles_codeg_and_custom_without_disabling_sources() {
    let home = Home::new();
    let mut collector = home.collector("codeg");
    collector.hub.ingest(
        json!({"source":"codeg","sessionId":"one","type":"start","roundId":"r1","ts":now()}),
    );
    collector.hub.ingest(
        json!({"source":"codeg","sessionId":"two","type":"start","roundId":"r2","ts":now()}),
    );
    assert_eq!(
        collector
            .request(
                "session_monitor_close",
                &json!({"source":"codeg","sessionId":"one","roundId":"r1"})
            )
            .unwrap(),
        json!({"closed":true})
    );
    assert!(!collector.hub.sessions.contains_key("codeg:one"));
    assert!(collector.hub.sessions.contains_key("codeg:two"));
    assert_eq!(collector.settings["sources"]["codeg"]["enabled"], true);

    collector.hub.ingest(json!({"source":"custom:sample","sessionId":"one","type":"start","roundId":"r1","ts":now()}));
    assert_eq!(
        collector
            .request(
                "session_monitor_close",
                &json!({"source":"custom:sample","sessionId":"one","roundId":"r1"})
            )
            .unwrap(),
        json!({"closed":true})
    );
    assert!(!collector.hub.sessions.contains_key("custom:sample:one"));
    assert!(collector
        .request(
            "session_monitor_close",
            &json!({"source":"custom:INVALID","sessionId":"one","roundId":"r1"})
        )
        .is_err());
}
