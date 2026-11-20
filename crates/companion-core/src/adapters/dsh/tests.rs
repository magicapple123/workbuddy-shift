//! DSH 适配器的单测：帧切分、增量读取、首扫例外、子代理/未知版本过滤、
//! 关闭后不复活。fixture 用 `zstd::stream::encode_all` 现造多帧文件，形状与
//! DSH 实际写出的日志一致（每帧以换行结束、从完整行开始）。
use super::*;
use serde_json::json;
use std::io::Write;

/// 把若干段文本各压成一个 zstd 帧 —— 与 DSH 的写入方式一致（多帧拼接）。
fn frames(parts: &[&str]) -> Vec<u8> {
    let mut out = Vec::new();
    for part in parts {
        out.extend(zstd::stream::encode_all(part.as_bytes(), 3).unwrap());
    }
    out
}

fn header_line(id: &str, parent: Option<&str>) -> String {
    let mut value = json!({
        "type":"session","version":4,"id":id,"createdAt":1,"cwd":"D:\\workbuddy-shift",
        "isSeeded":false,"delegationDepth":0
    });
    if let Some(parent) = parent {
        value["parentSession"] = json!(parent);
        value["origin"] = json!("subagent");
    }
    format!("{value}\n")
}

struct Fixture {
    home: PathBuf,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let home = std::env::temp_dir().join(format!(
            "dsh-{name}-{}",
            uuid_like()
        ));
        std::fs::create_dir_all(home.join(".dsh/sessions/-D-project-/session-abc")).unwrap();
        Self { home }
    }

    fn log(&self) -> PathBuf {
        self.home
            .join(".dsh/sessions/-D-project-/session-abc/session.v4.jsonl.zstd")
    }

    fn write(&self, bytes: &[u8]) {
        std::fs::write(self.log(), bytes).unwrap();
    }

    fn append(&self, bytes: &[u8]) {
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(self.log())
            .unwrap();
        file.write_all(bytes).unwrap();
    }

    fn collector(&self) -> Collector {
        let mut c = Collector::new(self.home.clone()).unwrap();
        // 只测适配器本身，别让真实 DSH_HOME 混进来。
        c.settings["sources"]["dsh"]["path"] = json!(self.home.join(".dsh"));
        c
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.home);
    }
}

fn uuid_like() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64
}

fn session<'a>(c: &'a Collector, id: &str) -> Option<&'a Value> {
    c.hub.sessions.get(&format!("dsh:{id}"))
}

#[test]
fn frame_walker_bounds_every_frame_and_waits_for_the_tail() {
    let all = frames(&["{\"a\":1}\n", "{\"b\":2}\n"]);
    let first = log_watch::frame_len(&all).expect("first frame");
    assert!(first < all.len());
    assert_eq!(all.len(), first + log_watch::frame_len(&all[first..]).unwrap());
    // 尾部半帧：不返回长度，因此 offset 不会越过它。
    assert_eq!(log_watch::frame_len(&all[..all.len() - 4]), Some(first));
    assert_eq!(log_watch::frame_len(&all[..3]), None);
    assert_eq!(log_watch::frame_len(b"not zstd"), None);
    assert_eq!(
        log_watch::decode_frame(&all[..first]).unwrap(),
        "{\"a\":1}\n"
    );
}

#[test]
fn live_turns_are_followed_incrementally_and_end_in_done() {
    let fixture = Fixture::new("incremental");
    let header = header_line("session-live", None);
    let first = frames(&[
        &header,
        "{\"type\":\"turn/start\",\"seq\":1,\"time\":1000,\"data\":{\"turn\":1}}\n",
        "{\"type\":\"step/start\",\"seq\":2,\"time\":1001,\"data\":{\"turn\":1,\"step\":1}}\n",
        "{\"type\":\"tool/call\",\"seq\":3,\"time\":1002,\"data\":{\"turn\":1,\"callId\":\"c1\",\"name\":\"run_code\"}}\n",
    ]);
    fixture.write(&first);
    let mut c = fixture.collector();
    c.poll_dsh().unwrap();
    let live = session(&c, "session-live").expect("已开始的 turn 应立刻可见");
    assert_eq!(live["status"], "running");
    assert_eq!(live["roundId"], "turn:1");
    assert_eq!(live["cwd"], "D:\\workbuddy-shift");
    assert_eq!(live["steps"][0]["label"], "run_code");
    assert_eq!(c.hub.sources["dsh"]["state"], "ok");
    let offset = c.dsh_log_watch.files[&fixture.log()].offset;
    assert_eq!(offset, first.len() as u64);

    // 第二轮：补一帧结束事件，只解析新增部分。
    fixture.append(&frames(&[
        "{\"type\":\"turn/end\",\"seq\":4,\"time\":1600,\"data\":{\"turn\":1,\"reason\":{\"kind\":\"completed\"}}}\n",
    ]));
    c.dsh_log_watch.last_scan = 0;
    c.poll_dsh().unwrap();
    let done = session(&c, "session-live").unwrap();
    assert_eq!(done["status"], "done");
    assert!(done["endedAt"].as_i64().unwrap() > 0);
    assert!(c.dsh_log_watch.files[&fixture.log()].offset > offset);
}

#[test]
fn history_is_not_imported_on_the_first_scan_but_a_running_turn_is() {
    let old = Fixture::new("history");
    old.write(&frames(&[
        &header_line("session-old", None),
        "{\"type\":\"turn/start\",\"seq\":1,\"time\":1000,\"data\":{\"turn\":1}}\n",
        "{\"type\":\"turn/end\",\"seq\":2,\"time\":1300,\"data\":{\"turn\":1,\"reason\":{\"kind\":\"completed\"}}}\n",
    ]));
    // 历史文件的 mtime 是 10 分钟前。
    let past = SystemTime::now() - std::time::Duration::from_secs(600);
    let file = std::fs::OpenOptions::new().write(true).open(old.log()).unwrap();
    file.set_modified(past).unwrap();
    let mut c = old.collector();
    c.poll_dsh().unwrap();
    assert!(c.hub.sessions.is_empty(), "首扫不得导入历史会话");
    assert_eq!(
        c.dsh_log_watch.files[&old.log()].offset,
        std::fs::metadata(old.log()).unwrap().len()
    );

    // 同一份历史文件在「运行中」的情况下（mtime 是刚刚）要能建档。
    let active = Fixture::new("active");
    active.write(&frames(&[
        &header_line("session-active", None),
        "{\"type\":\"turn/start\",\"seq\":1,\"time\":1000,\"data\":{\"turn\":2}}\n",
    ]));
    let mut c = active.collector();
    c.poll_dsh().unwrap();
    let live = session(&c, "session-active").expect("首扫例外：仍在跑的会话要可见");
    assert_eq!(live["status"], "running");
    assert_eq!(live["roundId"], "turn:2");
}

#[test]
fn subagents_unknown_versions_and_broken_lines_are_skipped() {
    let fixture = Fixture::new("skip");
    // 同一个 project 目录里三个会话目录：子代理 / 未知版本 / 坏行。
    for (dir, id, parent, version) in [
        ("child", "session-child", Some("session-root"), 4),
        ("future", "session-future", None, 9),
        ("broken", "session-broken", None, 4),
    ] {
        let path = fixture
            .home
            .join(".dsh/sessions/-D-project-")
            .join(dir)
            .join("session.v4.jsonl.zstd");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut value = json!({"type":"session","version":version,"id":id,"cwd":"/tmp/x"});
        if let Some(parent) = parent {
            value["parentSession"] = json!(parent);
        }
        let body = if dir == "broken" {
            frames(&[
                &format!("{value}\n"),
                "not json at all\n",
                "\n",
                "{\"type\":\"turn/start\",\"seq\":1,\"time\":2000,\"data\":{\"turn\":1}}\n",
            ])
        } else {
            frames(&[
                &format!("{value}\n"),
                "{\"type\":\"turn/start\",\"seq\":1,\"time\":2000,\"data\":{\"turn\":1}}\n",
            ])
        };
        std::fs::write(&path, body).unwrap();
    }
    let mut c = fixture.collector();
    c.poll_dsh().unwrap();
    assert!(session(&c, "session-child").is_none(), "子代理不建档");
    assert!(session(&c, "session-future").is_none(), "未知版本不建档");
    assert!(session(&c, "session-broken").is_some(), "坏行后面的正常行仍要解析");
    assert_eq!(c.hub.sources["dsh"]["state"], "error");
    assert!(text(&c.hub.sources["dsh"]["detail"]).contains("v9"));
}

#[test]
fn closing_a_card_keeps_it_closed_until_the_next_turn() {
    let fixture = Fixture::new("closed");
    fixture.write(&frames(&[
        &header_line("session-close", None),
        "{\"type\":\"turn/start\",\"seq\":1,\"time\":1000,\"data\":{\"turn\":1}}\n",
    ]));
    let mut c = fixture.collector();
    c.poll_dsh().unwrap();
    let closed = c
        .request(
            "session_monitor_close",
            &json!({"source":"dsh","sessionId":"session-close","roundId":"turn:1"}),
        )
        .unwrap();
    assert_eq!(closed["closed"], true);
    // 同一 turn 的后续事件不得把卡片建回来。
    fixture.append(&frames(&[
        "{\"type\":\"tool/call\",\"seq\":2,\"time\":1100,\"data\":{\"turn\":1,\"callId\":\"c9\",\"name\":\"shell\"}}\n",
        "{\"type\":\"turn/end\",\"seq\":3,\"time\":1200,\"data\":{\"turn\":1,\"reason\":{\"kind\":\"completed\"}}}\n",
    ]));
    c.dsh_log_watch.last_scan = 0;
    c.poll_dsh().unwrap();
    assert!(session(&c, "session-close").is_none(), "关掉的卡片不得复活");
    // 下一个 turn 正常出现。
    fixture.append(&frames(&[
        "{\"type\":\"turn/start\",\"seq\":4,\"time\":1300,\"data\":{\"turn\":2}}\n",
    ]));
    c.dsh_log_watch.last_scan = 0;
    c.poll_dsh().unwrap();
    let live = session(&c, "session-close").expect("下一个 turn 要重新出现");
    assert_eq!(live["roundId"], "turn:2");
    assert_eq!(live["status"], "running");
}

#[test]
fn missing_sessions_directory_degrades_to_unavailable() {
    let home = std::env::temp_dir().join(format!("dsh-missing-{}", uuid_like()));
    std::fs::create_dir_all(&home).unwrap();
    let mut c = Collector::new(home.clone()).unwrap();
    c.settings["sources"]["dsh"]["path"] = json!(home.join("nowhere"));
    c.poll_dsh().unwrap();
    assert_eq!(c.hub.sources["dsh"]["state"], "ok");
    assert!(text(&c.hub.sources["dsh"]["detail"]).contains("暂不可用"));
    assert!(c.hub.sessions.is_empty());
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn partial_tail_frame_is_retried_after_it_is_completed() {
    let fixture = Fixture::new("partial");
    let head = frames(&[
        &header_line("session-partial", None),
        "{\"type\":\"turn/start\",\"seq\":1,\"time\":1000,\"data\":{\"turn\":1}}\n",
    ]);
    let tail = frames(&[
        "{\"type\":\"turn/end\",\"seq\":2,\"time\":1500,\"data\":{\"turn\":1,\"reason\":{\"kind\":\"completed\"}}}\n",
    ]);
    // 先只写半帧。
    let mut body = head.clone();
    body.extend_from_slice(&tail[..tail.len() / 2]);
    fixture.write(&body);
    let mut c = fixture.collector();
    c.poll_dsh().unwrap();
    assert_eq!(session(&c, "session-partial").unwrap()["status"], "running");
    assert_eq!(
        c.dsh_log_watch.files[&fixture.log()].offset,
        head.len() as u64,
        "半帧不得推进 offset"
    );
    fixture.append(&tail[tail.len() / 2..]);
    c.dsh_log_watch.last_scan = 0;
    c.poll_dsh().unwrap();
    assert_eq!(session(&c, "session-partial").unwrap()["status"], "done");
}

/// 真机口径校验：需要本机真实的 `$DSH_HOME`（或 `~/.dsh`），默认 `#[ignore]`。
///
/// 跑法：`cargo test -p companion-core -- --ignored real_dsh_home_is_scanned_without_history`
///
/// 断言的是「首扫一个历史会话都不导入」「346 个文件都能读出 header」「子代理占多
/// 数」「日志里没有审批请求事件」这几条实测结论，**只读**，不写任何文件。
#[test]
#[ignore = "需要本机真实 DSH 数据，手动 --ignored 运行"]
fn real_dsh_home_is_scanned_without_history() {
    let home = std::env::temp_dir().join(format!("dsh-real-{}", uuid_like()));
    std::fs::create_dir_all(&home).unwrap();
    let user_home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .expect("无法定位用户主目录");
    let real = crate::settings::dsh_home(&user_home);
    let dir = real.join("sessions");
    let mut files = Vec::new();
    collect_session_logs(&dir, &mut files);
    if files.is_empty() {
        println!("{} 下没有会话日志，跳过（DSH 未安装或从未用过）", dir.display());
        let _ = std::fs::remove_dir_all(&home);
        return;
    }

    let mut c = Collector::new(home.clone()).unwrap();
    c.settings["sources"]["dsh"]["path"] = json!(real);
    c.poll_dsh().unwrap();
    println!(
        "files={} sessions_after_prime={} health={}",
        files.len(),
        c.hub.sessions.len(),
        c.hub.sources["dsh"]["detail"]
    );
    // 历史会话一个都不能进浮窗（本机 346 个会话的 mtime 都远早于 2 分钟窗口）。
    assert!(c.hub.sessions.is_empty(), "首扫不得导入历史会话");
    assert_eq!(c.dsh_log_watch.files.len(), files.len(), "每个文件都要登记偏移");

    let mut parents = 0;
    for path in &files {
        let state = &c.dsh_log_watch.files[path];
        assert!(state.unsupported.is_none(), "{}: 未知格式", path.display());
        let header = state.header.as_ref().expect("header");
        assert!(!header.id.is_empty());
        parents += usize::from(header.parent.is_some());
        assert_eq!(state.offset, std::fs::metadata(path).unwrap().len());
    }
    println!("files={} subagent={}", files.len(), parents);

    // 抽样全帧解码最小的几个文件：每一帧都要能解开、每一行都要是 JSON。
    let mut sample: Vec<&PathBuf> = files.iter().collect();
    sample.sort_by_key(|path| std::fs::metadata(path).map(|m| m.len()).unwrap_or(u64::MAX));
    let mut events = 0usize;
    for path in sample.iter().take(5) {
        let size = std::fs::metadata(path).unwrap().len();
        let mut offset = 0u64;
        let mut frames = 0usize;
        while let Some(buf) = log_watch::read_chunk(path, offset, size) {
            if buf.is_empty() {
                break;
            }
            let mut pos = 0usize;
            while let Some(len) = log_watch::frame_len(&buf[pos..]) {
                let text = log_watch::decode_frame(&buf[pos..pos + len]).expect("frame decodes");
                assert!(!text.contains("approval/asked"), "v4 日志不应出现审批请求");
                events += text.lines().filter(|line| !line.trim().is_empty()).count();
                for line in text.lines().filter(|line| !line.trim().is_empty()) {
                    serde_json::from_str::<Value>(line).expect("每行都是 JSON");
                }
                pos += len;
                frames += 1;
            }
            offset += pos as u64;
            if pos == 0 {
                break;
            }
        }
        assert_eq!(offset, size, "{}: 尾部还剩无法切帧的数据", path.display());
        println!("{}: frames={frames}", path.display());
    }
    println!("sampled_events={events}");
    let _ = std::fs::remove_dir_all(&home);
}
