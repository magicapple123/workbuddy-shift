//! DSH（DeepSeek Harness）监听适配器：**只读**轮询会话日志。
//!
//! 与其它四个来源的区别：DSH 没有可装的 hook（它的 hook 只覆盖 5 个事件，
//! `PermissionRequest` 会被静默丢弃，而且要写用户的 `profiles/*/cordis.patch.yml`），
//! 所以这里只读 `$DSH_HOME/sessions/<project>/<session-dir>/session*.jsonl.zstd`，
//! 不写 `$DSH_HOME` 下的任何文件，也不读内部缓存 `storages/session_projcache`。
//!
//! 状态只有三态：进行中（`running`）/ 已完成（`done`）/ 空闲（`unknown`，45 分钟
//! 无事件后由 `hub` 自动转过去）。**没有「等待确认」**：v4 日志里不存在审批请求
//! 事件，详见 `events::parse_approval_signal` 的说明。
use super::*;
use crate::settings;
use std::{
    path::{Path, PathBuf},
    time::SystemTime,
};

mod events;
mod log_watch;

pub use events::{DshEvent, Header, HeaderError};
pub use log_watch::{collect_session_logs, is_session_log, LogWatch};

impl Collector {
    /// DSH 的 `sessions` 目录（日志根）。
    pub fn dsh_sessions_dir(&self) -> PathBuf {
        let root = self
            .paths("dsh")
            .into_iter()
            .next()
            .unwrap_or_else(|| settings::dsh_home(&self.home));
        root.join("sessions")
    }

    pub fn poll_dsh(&mut self) -> Result<(), String> {
        let dir = self.dsh_sessions_dir();
        if std::fs::read_dir(&dir).is_err() {
            // 未安装 / 没有会话目录：显示「暂不可用」，不报错、不弹错。
            self.dsh_log_watch = Default::default();
            self.hub.sessions.retain(|_, s| s["source"] != "dsh");
            self.hub.health(
                "dsh",
                "ok",
                "暂不可用：未找到 DSH 会话目录（$DSH_HOME/sessions）",
            );
            return Ok(());
        }
        if !self.integration_automatic("dsh") {
            self.hub
                .health("dsh", "disabled", "已断开 DSH 只读监听（接入页开关）");
            return Ok(());
        }
        let time = now();
        if self.dsh_log_watch.primed && time - self.dsh_log_watch.last_scan < log_watch::SCAN_INTERVAL_MS
        {
            return Ok(());
        }
        self.dsh_log_watch.last_scan = time;
        let priming = !self.dsh_log_watch.primed;
        let mut files = Vec::new();
        log_watch::collect_session_logs(&dir, &mut files);
        for path in files {
            self.consume_dsh_log(&path, time, priming);
        }
        self.dsh_log_watch.primed = true;
        self.dsh_health();
        Ok(())
    }

    /// 未知格式版本优先于「已连接」的提示：用户看到「请升级」才知道为什么没会话。
    fn dsh_health(&mut self) {
        let unsupported = self
            .dsh_log_watch
            .files
            .values()
            .filter_map(|state| state.unsupported)
            .max();
        match unsupported {
            Some(version) => self.hub.health(
                "dsh",
                "error",
                &HeaderError::Unsupported(version).message(),
            ),
            None => self.hub.health(
                "dsh",
                "ok",
                "已连接 DSH 会话日志（只读；子代理会话不显示）",
            ),
        }
    }

    /// 关掉一个会话：清空追踪状态与已建档会话。
    pub fn reset_dsh(&mut self) {
        self.dsh_log_watch = Default::default();
        self.hub.sessions.retain(|_, s| s["source"] != "dsh");
        self.hub
            .events
            .retain(|event| !text(&event["sessionId"]).starts_with("dsh:"));
    }

    fn consume_dsh_log(&mut self, path: &Path, time: i64, priming: bool) {
        let Ok(meta) = std::fs::metadata(path) else {
            return;
        };
        let size = meta.len();
        let mtime = meta
            .modified()
            .ok()
            .and_then(|m| m.duration_since(SystemTime::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        if !self.dsh_log_watch.files.contains_key(path) {
            let active = mtime > time - log_watch::ACTIVE_WINDOW_MS;
            // 首扫例外：mtime 在 2 分钟内说明这个会话还在跑，完整解析一次。
            if let Some(state) = log_watch::register(path, size, mtime, priming, active) {
                self.dsh_log_watch.files.insert(path.to_path_buf(), state);
            }
        }
        let Some(state) = self.dsh_log_watch.files.get(path) else {
            return;
        };
        if state.hidden || state.unsupported.is_some() {
            return;
        }
        let Some(header) = state.header.clone() else {
            return;
        };
        let mut offset = state.offset;
        if size == offset && mtime == state.mtime {
            return; // 一点没变，连 open 都省掉
        }
        if size < offset {
            // 文件被重写或轮转：从头再读一遍，重复事件在 hub 侧按 eventId 去重。
            offset = 0;
            self.dsh_log_watch.rest.remove(path);
        }
        if let Some(buf) = log_watch::read_chunk(path, offset, size) {
            let consumed = self.consume_dsh_frames(path, &header, &buf);
            offset += consumed as u64;
        }
        if let Some(state) = self.dsh_log_watch.files.get_mut(path) {
            state.offset = offset;
            state.mtime = mtime;
        }
    }

    /// 逐帧解码。返回推进了多少字节。
    ///
    /// 只有**整帧**在缓冲区里才会被切出来（`frame_len` 返回 `None` 就是尾部半帧，
    /// 此时不推进，下一轮重试）。帧边界已知但解压失败的帧直接跳过：卡在坏帧上会
    /// 让该文件的后续事件永久丢失，比丢一帧更糟。
    fn consume_dsh_frames(&mut self, path: &Path, header: &Header, buf: &[u8]) -> usize {
        let mut pos = 0usize;
        while pos < buf.len() {
            let Some(len) = log_watch::frame_len(&buf[pos..]) else {
                break;
            };
            // 帧边界已知但解不开：跳过这一帧继续往后，别把文件永久卡住。
            if let Ok(text) = log_watch::decode_frame(&buf[pos..pos + len]) {
                self.consume_dsh_text(path, header, &text);
            }
            pos += len;
        }
        pos
    }

    /// 按行切分并把余下的半行留在 `rest` 里（跨轮拼接）。
    fn consume_dsh_text(&mut self, path: &Path, header: &Header, text: &str) {
        let mut pending = self.dsh_log_watch.rest.remove(path).unwrap_or_default();
        pending.push_str(text);
        // 最后一行没有换行符时先不扔：能解析成 JSON 就是完整行（DSH 正常会补
        // 换行，但不要因为少一个 `\n` 就把 `turn/end` 丢掉）。
        let tail_at = pending.rfind('\n').map(|i| i + 1).unwrap_or(0);
        let (complete, tail) = pending.split_at(tail_at);
        for line in complete.split('\n') {
            self.handle_dsh_line(header, line);
        }
        if !tail.trim().is_empty() {
            if serde_json::from_str::<Value>(tail).is_ok() {
                self.handle_dsh_line(header, tail);
            } else {
                self.dsh_log_watch
                    .rest
                    .insert(path.to_path_buf(), tail.to_owned());
            }
        }
    }

    fn handle_dsh_line(&mut self, header: &Header, line: &str) {
        let line = line.trim();
        if line.is_empty() {
            return;
        }
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            return; // 坏行 / 空行跳过，不 panic
        };
        let Some(event) = events::map_event(&value) else {
            return;
        };
        // 用户点过关闭的会话：同一个 turn 的后续事件不再建档，下一个 turn 正常出现。
        if let Some(closed) = self.dsh_log_watch.closed.get(&header.id).cloned() {
            match &event {
                DshEvent::Start { round } if round != &closed => {
                    self.dsh_log_watch.closed.remove(&header.id);
                }
                _ => return,
            }
        }
        let ts = value["time"]
            .as_i64()
            .filter(|n| *n > 0)
            .unwrap_or_else(now);
        let sid = header.id.clone();
        let cwd = header.cwd.clone();
        match event {
            DshEvent::Title(title) => self.hub.ingest(json!({
                "source":"dsh","sessionId":sid,"cwd":cwd,"type":"meta","title":title,"ts":ts
            })),
            DshEvent::Start { round } => self.hub.ingest(json!({
                "source":"dsh","sessionId":sid,"cwd":cwd,"roundId":round,"type":"start","ts":ts
            })),
            DshEvent::Step {
                round,
                call_id,
                label,
            } => self.hub.ingest(json!({
                "source":"dsh","sessionId":sid,"cwd":cwd,"roundId":round,"type":"step",
                "eventId":call_id,"label":label,"ts":ts
            })),
            DshEvent::End {
                round,
                status,
                ended_by,
            } => self.hub.ingest(json!({
                "source":"dsh","sessionId":sid,"cwd":cwd,"roundId":round,"type":"end",
                "status":status,"endedBy":ended_by,"ts":ts
            })),
        }
    }
}

#[cfg(test)]
mod tests;
