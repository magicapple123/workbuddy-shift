//! WorkBuddy 运行日志兜底监听：沙箱审批等待/结算。

use super::*;

// 扫描节流：WorkBuddy 会话日志本身有 10–25 秒的应用层缓冲（实测），
// 端到端延迟由它支配；本侧节流放宽到 10 秒以降低目录扫描开销。
pub(crate) const LOG_SCAN_INTERVAL_MS: i64 = 10_000;

pub(crate) const LOG_ACTIVE_WINDOW_MS: i64 = 10 * 60_000;

#[derive(Debug, PartialEq)]
pub enum ApprovalEvent {
    Request {
        tool: String,
        call_id: String,
        session: String,
    },
    Settled {
        call_id: String,
    },
}

/// 解析一行 WorkBuddy 运行日志；不是审批行则返回 None（纯函数，便于测试）。
/// 超时行（Sandbox approval timed out）不解析：超时后工具以失败结束，
/// 真实的 PostToolUseFailure hook 会兜底清理 pending。
pub fn parse_approval_line(line: &str) -> Option<ApprovalEvent> {
    if let Some(rest) = line
        .split_once("[enqueueSandboxApproval] Enqueued sandbox approval: ")
        .map(|(_, rest)| rest)
    {
        let tool = field(rest, "tool=")?;
        let call_id = field(rest, ", id=")?;
        let session = field(rest, ", session=")?;
        if tool.is_empty() || call_id.is_empty() || session.is_empty() {
            return None;
        }
        return Some(ApprovalEvent::Request {
            tool,
            call_id,
            session,
        });
    }
    for marker in [
        "[Approve] User approved tool: ",
        "[Reject] User rejected tool: ",
    ] {
        if let Some(rest) = line.split_once(marker).map(|(_, rest)| rest) {
            let (tool, tail) = rest.split_once(", id: ")?;
            let call_id = tail.split(',').next().unwrap_or("").trim().to_owned();
            if tool.trim().is_empty() || call_id.is_empty() {
                return None;
            }
            return Some(ApprovalEvent::Settled { call_id });
        }
    }
    None
}

pub(crate) fn field(rest: &str, key: &str) -> Option<String> {
    Some(rest.split_once(key)?.1.split(',').next()?.trim().to_owned())
}

/// 增量扫描状态：文件 offset、审批 callId→会话 映射、节流与首次扫描标记。
#[derive(Default)]
pub struct LogWatch {
    pub offsets: HashMap<PathBuf, u64>,
    pub call_sessions: HashMap<String, String>,
    pub primed: bool,
    pub last_scan: i64,
}

pub(crate) fn collect_log_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Ok(inner) = std::fs::read_dir(&path) {
                for entry in inner.flatten() {
                    let file = entry.path();
                    if is_log_file(&file) {
                        out.push(file);
                    }
                }
            }
        } else if is_log_file(&path) {
            out.push(path);
        }
    }
}

pub(crate) fn is_log_file(path: &Path) -> bool {
    path.is_file() && path.extension().map(|e| e == "log").unwrap_or(false)
}

impl Collector {
    pub fn poll_workbuddy(&mut self) -> Result<(), String> {
        self.poll_workbuddy_log_watch();
        if self.workbuddy_presence.observe() == crate::host_process::Presence::Gone {
            crate::host_process::end_host_sessions(&mut self.hub, "workbuddy", None);
            self.hub.health(
                "workbuddy",
                "exited",
                "WorkBuddy 已退出，未完成的任务已标记中止",
            );
            return Ok(());
        }
        self.hub.health(
            "workbuddy",
            "ok",
            if self.workbuddy_hook_count == 0 {
                "等待新的 WorkBuddy Hook；不恢复历史会话"
            } else {
                "已连接 WorkBuddy Hook 与审批日志（不读取会话文件）"
            },
        );
        Ok(())
    }
}

impl Collector {
    pub fn poll_workbuddy_log_watch(&mut self) {
        if self.settings["sources"]["workbuddy"]["logWatch"] == false {
            return;
        }
        // 审批只会发生在回合进行中的会话上；没有活跃会话时不必扫日志
        // （此时候选注入目标也不存在，扫了也无处可用）。
        let active = self.hub.sessions.values().any(|s| {
            s["source"] == "workbuddy" && matches!(text(&s["status"]).as_str(), "running" | "wait")
        });
        if !active {
            return;
        }
        let time = now();
        if self.workbuddy_log_watch.primed
            && time - self.workbuddy_log_watch.last_scan < LOG_SCAN_INTERVAL_MS
        {
            return;
        }
        self.workbuddy_log_watch.last_scan = time;
        let mut files = Vec::new();
        for root in [
            self.home.join(".workbuddy-ai").join("logs"),
            self.home.join(".workbuddy").join("logs"),
        ] {
            collect_log_files(&root, &mut files);
        }
        let prime_only = !self.workbuddy_log_watch.primed;
        for path in files {
            self.consume_approval_log(&path, time, prime_only);
        }
        self.workbuddy_log_watch.primed = true;
    }
}

impl Collector {
    pub(crate) fn consume_approval_log(&mut self, path: &Path, time: i64, prime_only: bool) {
        let Ok(meta) = std::fs::metadata(path) else {
            return;
        };
        let size = meta.len();
        if prime_only {
            // 首次扫描只登记 offset：历史内容一律不看，只跟踪此后的新行。
            self.workbuddy_log_watch
                .offsets
                .insert(path.to_path_buf(), size);
            return;
        }
        let offset = self
            .workbuddy_log_watch
            .offsets
            .get(path)
            .copied()
            .unwrap_or(0);
        let offset = if size < offset { 0 } else { offset };
        if size <= offset {
            self.workbuddy_log_watch
                .offsets
                .insert(path.to_path_buf(), offset);
            return;
        }
        let active = meta
            .modified()
            .ok()
            .and_then(|m| m.duration_since(SystemTime::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as i64 > time - LOG_ACTIVE_WINDOW_MS)
            .unwrap_or(false);
        if !active {
            self.workbuddy_log_watch
                .offsets
                .insert(path.to_path_buf(), offset);
            return;
        }
        let Ok(mut file) = std::fs::File::open(path) else {
            return;
        };
        if file.seek(SeekFrom::Start(offset)).is_err() {
            return;
        }
        let mut buffer = Vec::new();
        if file.take(1 << 20).read_to_end(&mut buffer).is_err() {
            return;
        }
        self.workbuddy_log_watch
            .offsets
            .insert(path.to_path_buf(), offset + buffer.len() as u64);
        for line in String::from_utf8_lossy(&buffer).lines() {
            match parse_approval_line(line) {
                Some(ApprovalEvent::Request {
                    tool,
                    call_id,
                    session,
                }) => {
                    self.workbuddy_log_watch
                        .call_sessions
                        .insert(call_id.clone(), session.clone());
                    self.apply_approval_wait(&session, &tool, &call_id, time);
                }
                Some(ApprovalEvent::Settled { call_id }) => {
                    if let Some(session) = self.workbuddy_log_watch.call_sessions.remove(&call_id) {
                        self.apply_approval_settled(&session, &call_id, time);
                    }
                }
                None => {}
            }
        }
    }
}

impl Collector {
    pub(crate) fn apply_approval_wait(
        &mut self,
        session: &str,
        tool: &str,
        call_id: &str,
        ts: i64,
    ) {
        if !self
            .hub
            .sessions
            .contains_key(&format!("workbuddy:{session}"))
        {
            return; // 只为已跟踪的会话补充信号，不创建幽灵会话
        }
        self.hub.ingest(json!({
            "source":"workbuddy",
            "sessionId":session,
            "type":"wait",
            "callId":call_id,
            "tool":tool,
            "text":format!("WorkBuddy 沙箱审批待确认：{tool}"),
            "ts":ts
        }));
    }
}

impl Collector {
    pub(crate) fn apply_approval_settled(&mut self, session: &str, call_id: &str, ts: i64) {
        if !self
            .hub
            .sessions
            .contains_key(&format!("workbuddy:{session}"))
        {
            return;
        }
        self.hub.ingest(json!({
            "source":"workbuddy",
            "sessionId":session,
            "type":"resolve",
            "callId":call_id,
            "ts":ts
        }));
    }
}
