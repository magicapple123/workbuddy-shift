//! 脚本写入者的「一事件一文件」落盘收件箱（issue #20 第 2 轮回归：并发写入）。
//!
//! ## 为什么不是「追加到同一个 jsonl」
//!
//! hook 脚本没有文件锁（`flock` 在 Git for Windows 里没有，`mkdir` 自旋锁会在 hook 被
//! 强杀时留下死锁 —— 那正是 #20 的「全员超时」）。而 shell 的 `printf … >> file` **不是
//! 一次 `write()`**：payload 超过 stdio 缓冲就会分多次写，两个 hook 同时写就会交错，
//! 把 B 的指令行插进 A 的 payload 中间（本机实测 16 并发 8KB payload，5 轮里必有一轮丢
//! 2–6 条事件）。
//!
//! 所以脚本**不再往共享文件里写**：每个 hook 进程把一帧写进自己的私有文件
//! （写多少次都无所谓，没人跟它抢同一份字节），再用一次 `mv` 原子改名成 `.evt`
//! 发布到本目录。daemon 侧只读 `.evt`：
//!
//! - 不会读到写了一半的文件（改名是原子的，读者看到的永远是完整帧）；
//! - 不会被并发写入交错（一帧一文件，物理上不可能交叉）；
//! - 不需要任何锁，因此**不存在 hook 被强杀后留下死锁**的路径；
//! - 崩溃安全：写完没来得及 `mv` 的 `.part` 由 [`EventInbox::sweep_stale`] 回收。
//!
//! 旧的单文件队列（`companion-hook-spool.jsonl`）继续存在：v0.4.2 的 exe 命令与
//! 遗留事件都在那里，两条链路都接在 [`crate::HookSpool::drain`] 上。

use crate::hook_frame::parse_frame;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// 收件箱目录名（相对 `~/.workbuddy-shift-companion`）。
pub const INBOX_DIR_NAME: &str = "hook-inbox.d";
/// 已发布、待消费的事件文件后缀。
const READY_SUFFIX: &str = "evt";
/// 写入中（尚未 `mv`）的私有文件后缀。
const PARTIAL_SUFFIX: &str = "part";
/// `.part` 留这么久还没改名，说明写它的进程被强杀了：回收掉，避免无限堆积。
const STALE_PARTIAL: Duration = Duration::from_secs(300);

/// 收件箱目录。
pub fn inbox_dir(home: &Path) -> PathBuf {
    home.join(".workbuddy-shift-companion").join(INBOX_DIR_NAME)
}

/// 收件箱的增量消费者：每轮把 `.evt` 读成事件并删除。
pub struct EventInbox {
    dir: PathBuf,
}

impl EventInbox {
    pub fn new(home: &Path) -> Self {
        Self {
            dir: inbox_dir(home),
        }
    }

    /// 消费当前全部 `.evt`。按修改时间排序（同一轮并发写入时尽量保持事件先后），
    /// 逐条投递后删除；解析失败的文件直接丢弃并记一条诊断（重试也不会变好）。
    ///
    /// 投递回调 panic 时不删除文件 ⇒ 下次重启会重投（at-least-once，与单文件队列一致）。
    pub fn drain(&self, mut consume: impl FnMut(Value)) -> Result<usize, String> {
        let entries = match std::fs::read_dir(&self.dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
            Err(e) => return Err(format!("读取 {} 失败：{e}", self.dir.display())),
        };
        let mut ready: Vec<(SystemTime, PathBuf)> = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == READY_SUFFIX) {
                let at = entry
                    .metadata()
                    .and_then(|meta| meta.modified())
                    .unwrap_or(SystemTime::UNIX_EPOCH);
                ready.push((at, path));
            }
        }
        ready.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
        let mut count = 0;
        for (_, path) in ready {
            let content = match std::fs::read(&path) {
                Ok(bytes) => bytes,
                // 被别的消费者（或用户）抢先删掉了：不是错误。
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Err(format!("读取 {} 失败：{e}", path.display())),
            };
            match parse_frame(&String::from_utf8_lossy(&content)) {
                Some(value) => {
                    consume(value);
                    count += 1;
                }
                None => eprintln!("忽略损坏的 hook 事件文件：{}", path.display()),
            }
            // 只有投递成功（或确认损坏）才删除：投递回调 panic 时文件留着，下次重启重投。
            if let Err(e) = std::fs::remove_file(&path) {
                if e.kind() != std::io::ErrorKind::NotFound {
                    eprintln!("删除 {} 失败：{e}", path.display());
                }
            }
        }
        self.sweep_stale();
        Ok(count)
    }

    /// 回收「写完没来得及改名」的残骸（进程被强杀 / 断电）。正常路径不会命中。
    fn sweep_stale(&self) {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return;
        };
        let now = SystemTime::now();
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.extension().is_some_and(|ext| ext == PARTIAL_SUFFIX) {
                continue;
            }
            let stale = entry
                .metadata()
                .and_then(|meta| meta.modified())
                .ok()
                .and_then(|at| now.duration_since(at).ok())
                .is_some_and(|age| age > STALE_PARTIAL);
            if stale {
                let _ = std::fs::remove_file(&path);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hook_frame::SOURCE_DIRECTIVE;
    use serde_json::json;

    fn temp_home(tag: &str) -> PathBuf {
        let home = std::env::temp_dir().join(format!("hook-inbox-{tag}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(inbox_dir(&home)).unwrap();
        home
    }

    fn publish(home: &Path, name: &str, body: &str) -> PathBuf {
        let path = inbox_dir(home).join(format!("{name}.evt"));
        std::fs::write(&path, body).unwrap();
        path
    }

    #[test]
    fn published_events_are_delivered_once_and_removed() {
        let home = temp_home("once");
        let first = publish(
            &home,
            "100",
            &format!("{SOURCE_DIRECTIVE} workbuddy domestic\n{{\"session_id\":\"a\"}}\n"),
        );
        publish(
            &home,
            "101",
            &format!("{SOURCE_DIRECTIVE} codebuddy-ide international\n{{\"session_id\":\"b\"}}\n"),
        );
        let inbox = EventInbox::new(&home);
        let mut events = Vec::new();
        assert_eq!(inbox.drain(|v| events.push(v)).unwrap(), 2);
        assert_eq!(events[0]["agent_source"], "workbuddy");
        assert_eq!(events[0]["agent_edition"], "domestic");
        assert_eq!(events[1]["agent_source"], "codebuddy-ide");
        assert!(!first.exists(), "投递后必须删除，避免重复消费");
        assert_eq!(inbox.drain(|_| panic!("不得重复投递")).unwrap(), 0);
        std::fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn a_missing_inbox_is_not_an_error_and_creates_nothing() {
        let home = std::env::temp_dir().join(format!("hook-inbox-absent-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&home).unwrap();
        let inbox = EventInbox::new(&home);
        assert_eq!(inbox.drain(|_| panic!("不得投递")).unwrap(), 0);
        assert!(!inbox_dir(&home).exists(), "消费者不得创建目录");
        std::fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn a_partial_file_is_never_delivered_and_stale_ones_are_swept() {
        let home = temp_home("partial");
        // 写入中（没有 mv）的文件对消费者不可见。
        let partial = inbox_dir(&home).join("200.part");
        std::fs::write(
            &partial,
            format!("{SOURCE_DIRECTIVE} workbuddy domestic\n{{"),
        )
        .unwrap();
        let inbox = EventInbox::new(&home);
        assert_eq!(inbox.drain(|_| panic!("不得投递半成品")).unwrap(), 0);
        assert!(partial.exists(), "未过期的 .part 不能被回收");
        // 模拟被强杀留下的残骸：把 mtime 改成 10 分钟前。
        let old = SystemTime::now() - Duration::from_secs(600);
        let file = std::fs::OpenOptions::new()
            .write(true)
            .open(&partial)
            .unwrap();
        file.set_modified(old).unwrap();
        drop(file);
        assert_eq!(inbox.drain(|_| {}).unwrap(), 0);
        assert!(!partial.exists(), "过期 .part 必须被回收");
        std::fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn a_corrupt_file_is_dropped_without_blocking_others() {
        let home = temp_home("corrupt");
        publish(&home, "300", "not json at all\n");
        publish(
            &home,
            "301",
            &format!(
                "{SOURCE_DIRECTIVE} workbuddy domestic\n{}",
                json!({"session_id":"ok"})
            ),
        );
        let inbox = EventInbox::new(&home);
        let mut events = Vec::new();
        assert_eq!(inbox.drain(|v| events.push(v)).unwrap(), 1);
        assert_eq!(events[0]["session_id"], "ok");
        std::fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn concurrent_publishers_never_lose_an_event() {
        // 每个线程写自己的私有文件再改名 —— 与脚本的行为一致。
        let home = temp_home("concurrent");
        let mut handles = Vec::new();
        for id in 0..32u32 {
            let dir = inbox_dir(&home);
            handles.push(std::thread::spawn(move || {
                let part = dir.join(format!("{id}.part"));
                std::fs::write(
                    &part,
                    format!("{SOURCE_DIRECTIVE} workbuddy domestic\n{{\"session_id\":\"s-{id}\"}}"),
                )
                .unwrap();
                std::fs::rename(&part, dir.join(format!("{id}.evt"))).unwrap();
            }));
        }
        for handle in handles {
            handle.join().unwrap();
        }
        let inbox = EventInbox::new(&home);
        let mut ids: Vec<u32> = Vec::new();
        inbox
            .drain(|v| {
                ids.push(
                    v["session_id"]
                        .as_str()
                        .and_then(|id| id.trim_start_matches("s-").parse().ok())
                        .unwrap_or(u32::MAX),
                )
            })
            .unwrap();
        ids.sort_unstable();
        assert_eq!(ids, (0..32).collect::<Vec<u32>>(), "并发发布不得丢事件");
        std::fs::remove_dir_all(&home).unwrap();
    }
}
